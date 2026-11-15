// SPDX-License-Identifier: AGPL-3.0-or-later
//! The cost cases' sounds, shared by fmbench and fmhost.

/// Operator `o`'s (1-8) form, spread and formant in a cost case: `mixed` puts the eight on eight
/// different tables (the worst case for the cache); `res` is the costliest kernel.
fn case_form(forms: &str, o: usize) -> (usize, usize, usize) {
    const MIXED: [(usize, usize); 8] = [(1, 0), (2, 3), (3, 5), (4, 7), (5, 2), (6, 6), (1, 7), (4, 1)];
    match forms {
        "all" => (1, 3, 0),
        "odd" => (3, 3, 0),
        "res" => (5, 7, 3),
        "mixed" => (MIXED[o - 1].0, MIXED[o - 1].1, 4 + 3 * o),
        _ => (0, 0, 0),
    }
}

/// A sound like the parts of a converted multi-sound setting (`n` varies it a little, as such
/// parts vary): eight operators on routing 13, two of them on a non-sine form, decaying
/// envelopes, level scaling and velocity, the 24 dB low-pass with its envelope, and two controller
/// boxes at rest values: the mod wheel at 0 on the voice LFO's amplitude depth (moving nothing)
/// and knob 3 a little off centre on the cutoff (moving it a little).
pub fn shared_text(n: usize) -> String {
    let mut t = format!(
        "fm-sound/1\nalgorithm=13 feedback=6 lfo_speed={} lfo_key_on_reset=1 lfo_pitch_depth=1 random_pitch=3 \
         filter_type=0 cutoff_velocity=5 resonance=5 filter_eg_time_decay1=75 filter_eg_level_attack=127 \
         filter_eg_level_decay1=80 filter_eg_level_decay2=80 filter_eg_depth=50 detune={}\n\
         knob3=514 box1_dst=91 box1_ratio=40 box2_src=10 box2_dst=102 box2_ratio=127 box2_pol=1\n",
        24 + n,
        n as i32 - 2
    );
    for o in 1..=8usize {
        let (level, decay) = (
            [60, 70, 85, 99, 85, 99, 90, 99][o - 1],
            [60, 60, 58, 70, 64, 60, 56, 56][o - 1],
        );
        t += &format!(
            "op{o}.level={level} op{o}.detune={} op{o}.eg_time_decay1={decay} op{o}.eg_level_decay1=0 \
             op{o}.eg_time_decay2={decay} op{o}.eg_level_decay2=0 op{o}.eg_time_key_follow=4 \
             op{o}.level_velocity=2 op{o}.scaling_break={} op{o}.scaling_high_depth=30\n",
            o as i32 % 7 - 3,
            if o > 4 { 15 } else { 0 }
        );
        if o <= 2 {
            t += &format!("op{o}.form=1 op{o}.spread=3\n");
        }
    }
    t
}

/// A cost case's sound, in the text form (fmbench's `case_sound` says what each case is).
pub fn case_text(routing: u32, ops: u32, forms: &str) -> String {
    // FORMS+fT: filter type T (the stored value) on the case, its envelope moving; FORMS+fx: the
    // dual band-stop with the envelope, the voice LFO on the cutoff and the sound LFO on cutoff,
    // resonance and pan (the costliest filter); FORMS+pan: alternate and random pan; a last +ctl:
    // every controller box active (30 on the operators' levels, one on the cutoff, one on the
    // resonance, from the mod wheel, foot controller 1 and knob 1)
    let (forms, post) = forms.split_once('+').unwrap_or((forms, ""));
    let (post, ctl) = match post.strip_suffix("ctl") {
        Some(p) => (p.trim_end_matches('+'), true),
        None => (post, false),
    };
    let pitch = forms == "pitch";
    let mut t = format!(
        "fm-sound/1\nalgorithm={routing} feedback=7 portamento={}\n",
        u8::from(pitch)
    );
    let peg = "pitch_eg_level_initial=-50 pitch_eg_level_attack=50 pitch_eg_time_attack=60";
    if pitch {
        t += &format!("portamento_time=100 {peg}\n");
    }
    let m = "scaling_low_depth=40 scaling_high_depth=40 scaling_high_curve=2 lfo_pitch_offset=5 lfo_amp_offset=5";
    if forms == "slfo" {
        t += "slfo_wave=0 slfo_speed=40 slfo_delay=5 slfo_key_on_reset=1 slfo_dest1=75 slfo_depth1=20 \
              slfo_dest2=78 slfo_depth2=30 slfo_dest3=77 slfo_depth3=16\n";
    }
    if forms == "mod" {
        t += "lfo_wave=4 lfo_speed_range=0 lfo_speed=40 lfo_delay=5 lfo_pitch_depth=20 lfo_amp_depth=30\n";
        for o in 1..=8 {
            t += &format!("op{o}.{}\n", m.replace(' ', &format!(" op{o}.")));
        }
    }
    if let Some(f) = post.strip_prefix('f') {
        let all = f == "x";
        if all {
            t += "lfo_wave=0 lfo_speed=40 lfo_filter_depth=30\n";
        }
        let (kind, gain) = match f {
            "x" => ("17", 255),
            "gain" => ("21", 200),
            k => (k, 255),
        };
        t += &format!(
            "filter_type={kind} cutoff=600 resonance=60 hpf_cutoff=200 distance=24 filter_gain={gain} \
             filter_eg_time_attack=100 filter_eg_level_attack=40 filter_eg_depth=40\n"
        );
        if all {
            t += "slfo_wave=0 slfo_speed=40 slfo_dest1=70 slfo_depth1=40 slfo_dest2=72 slfo_depth2=10 \
                  slfo_dest3=73 slfo_depth3=20\n";
        }
    }
    if post == "pan" {
        t += "random_pan=64 alternate_pan=32\n";
    }
    if ctl {
        for n in 1..=32 {
            let dst = match n {
                31 => 102,
                32 => 103,
                _ => 106,
            };
            let (src, ratio) = ([1, 3, 8][n % 3], if n % 2 == 0 { 6 } else { -6 });
            t += &format!(
                "box{n}_src={src} box{n}_dst={dst} box{n}_ratio={ratio} box{n}_pol={}\n",
                n % 4 / 3
            );
        }
        t += "knob1=700\n";
    }
    for o in 1..=8 {
        let level = if o > 8 - ops { 99 } else { 0 };
        let (f, s, r) = case_form(forms, o as usize);
        t += &format!(
            "op{o}.level={level} op{o}.coarse={} op{o}.eg_time_decay1=60 op{o}.eg_level_decay1=0 \
             op{o}.form={f} op{o}.spread={s} op{o}.formant={r}\n",
            o % 4
        );
        if pitch {
            t += &format!("op{o}.{}\n", peg.replace(' ', &format!(" op{o}.")));
        }
    }
    t
}
