// SPDX-License-Identifier: AGPL-3.0-or-later
//! What the engine plays of the model: for each parameter, played, or the milestone that brings
//! it. `docs/coverage.md` is the same table with each law's accuracy, and a test ties the two.
//! A sound's report lists its parameters at non-default values that the engine does not play.

use crate::params::{Param, Part, BOX_BASE, BOX_FIELDS, PARAMS};
use crate::sound::Sound;

/// Played, or why not (the milestone that brings it).
pub const PLAYED: &str = "played";

/// Tempo sync: the core has no tempo input.
const NO_TEMPO: &str = "not covered (no tempo input)";

/// A stored filter type value that is none of the model's types (9, 18, 20).
const NO_TYPE: &str = "not covered (no filter type: played as thru)";

/// Why a sound LFO destination (the reference's number) with depth is not played, or `None`.
pub fn slfo_destination(dest: i16) -> Option<&'static str> {
    match dest {
        70..=78 => None,
        _ => Some("not covered (an effect's parameter, not the engine's)"),
    }
}

/// The coverage of one parameter.
pub fn status(p: &Param) -> &'static str {
    let n = p.name;
    let op = p.op.is_some();
    let played_op = [
        "key_on_reset",
        "mode",
        "coarse",
        "fine",
        "detune",
        "form",
        "spread",
        "formant",
        "eg_level_attack",
        "eg_level_decay1",
        "eg_level_decay2",
        "eg_level_release",
        "eg_time_attack",
        "eg_time_decay1",
        "eg_time_decay2",
        "eg_time_release",
        "eg_time_hold",
        "eg_time_key_follow",
        "level",
        "level_velocity",
        "scaling_break",
        "scaling_low_depth",
        "scaling_high_depth",
        "scaling_low_curve",
        "scaling_high_curve",
        "lfo_pitch_offset",
        "lfo_amp_offset",
    ];
    let played_part = [
        "algorithm",
        "feedback",
        "note_shift",
        "velocity_depth",
        "velocity_offset",
        "macro_depth",
        "macro_ratio",
        "macro_attack",
        "macro_decay",
        "macro_sustain",
        "macro_release",
        "macro_texture",
        "amp_eg_attack_offset",
        "amp_eg_decay_offset",
        "amp_eg_sustain_offset",
        "amp_eg_release_offset",
        "lfo_wave",
        "lfo_speed",
        "lfo_phase",
        "lfo_delay",
        "lfo_key_on_reset",
        "lfo_pitch_depth",
        "lfo_amp_depth",
        "lfo_speed_range",
        "lfo_extended_speed",
    ];
    let starts = |prefixes: &[&str]| prefixes.iter().any(|x| n.starts_with(x));
    if (op && played_op.contains(&n)) || (!op && played_part.contains(&n)) {
        PLAYED
    } else if starts(&["slfo_tempo"]) {
        NO_TEMPO
    } else if n == "slfo_random_speed" {
        "M7 (unmeasured)"
    } else if starts(&["slfo_"]) {
        // M4: the sound LFO, its destinations (those not played: `slfo_destination`), boxes and
        // ratios
        PLAYED
    } else if (op && starts(&["ctrl_set"])) || (!op && starts(&["box", "knob", "ribbon", "switch"])) {
        // M7: the controller boxes, their sources and the operators' switches for them (a box the
        // engine does not play: `crate::boxes::not_played`)
        PLAYED
    } else if n == "pitch_ctrl_sens" {
        // M7: the operator frequency boxes' share
        PLAYED
    } else if n == "level_ctrl_sens" {
        "M7 (unmeasured)"
    } else if starts(&["key_ctrl_set", "tuning_"]) {
        "M7 (controllers, tuning)"
    } else if starts(&[
        "pitch_",
        "random_pitch",
        "portamento",
        "bend_",
        "mono_poly",
        "key_assign",
        "fixed_key",
    ]) || (!op && n == "detune")
    {
        // M3: both pitch envelopes, key follow, velocity, random pitch, detune, bend,
        // portamento, mono and key assign
        PLAYED
    } else if starts(&["key_on_delay_"]) {
        // tempo sync and its note length
        NO_TEMPO
    } else if starts(&[
        "key_on_delay",
        "filter_",
        "cutoff",
        "resonance",
        "hpf_",
        "distance",
        "random_pan",
        "alternate_pan",
        "scaling_pan",
        "lfo_filter_depth",
    ]) {
        // M5: key-on delay, the filter, its envelope and pan
        PLAYED
    } else {
        // the LFOs, macros, level scaling, part envelope offsets
        "M4 (modulators)"
    }
}

/// Calls `f` with each parameter of `s` at a non-default value that the engine does not play,
/// its value and why, and each sound LFO destination with depth that it does not play.
pub fn not_covered(s: &Sound, mut f: impl FnMut(&'static Param, i16, &'static str)) {
    for (p, v) in s.changed() {
        let why = status(p);
        if why != PLAYED {
            f(p, v, why);
        } else if p.name == "filter_type" && !crate::filter::is_type(v) {
            f(p, v, NO_TYPE);
        }
    }
    for d in 0..3 {
        let i = Part::SlfoDest as usize + 2 * d;
        let (dest, depth) = (s.get(i), s.get(i + 1));
        if let (true, Some(why), Some(p)) = (depth > 0, slfo_destination(dest), PARAMS.get(i)) {
            f(p, dest, why);
        }
    }
    // each controller box not played, by its destination
    for n in 0..crate::boxes::BOXES {
        let i = BOX_BASE + BOX_FIELDS * n + 1;
        if let (Some(why), Some(p)) = (crate::boxes::not_played(s, n), PARAMS.get(i)) {
            f(p, s.get(i), why);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::PARAMS;

    /// docs/coverage.md has one row per parameter (operator parameters once, as `opN.name`) whose
    /// third column is this module's status.
    #[test]
    fn the_coverage_table_matches_the_code() {
        let doc = include_str!("../../../docs/coverage.md");
        let rows: Vec<Vec<&str>> = doc
            .lines()
            .filter(|l| l.starts_with("| `"))
            .map(|l| l.trim_matches('|').split(" | ").map(str::trim).collect())
            .collect();
        let mut seen = std::collections::HashSet::new();
        for p in PARAMS.iter() {
            // operator parameters once as `opN.name`, the boxes' fields once as `boxN_field`
            let name = match (p.op, p.name.strip_prefix("box").and_then(|r| r.split_once('_'))) {
                (Some(_), _) => format!("`opN.{}`", p.name),
                (None, Some((_, field))) => format!("`boxN_{field}`"),
                _ => format!("`{}`", p.name),
            };
            if !seen.insert(name.clone()) {
                continue;
            }
            let row = rows
                .iter()
                .find(|r| r[0] == name)
                .unwrap_or_else(|| panic!("{name} missing from docs/coverage.md"));
            assert_eq!(row[2], status(p), "{name}");
        }
        assert_eq!(
            rows.len(),
            seen.len(),
            "docs/coverage.md has rows for parameters that do not exist"
        );
    }

    #[test]
    fn report() {
        let s = Sound::from_text(
            "fm-sound/1\nrandom_pan=3 key_on_delay=4 key_on_delay_sync=1 algorithm=5 filter_type=18 slfo_depth1=3 slfo_dest2=72 \
             slfo_depth2=9 slfo_dest3=75 slfo_depth3=1 slfo_tempo_sync=1 box1_dst=106 box1_ratio=9 box2_dst=92 \
             box2_ratio=9\nop2.level=50 op2.pitch_velocity=3 op2.scaling_low_depth=3\n",
        )
        .unwrap();
        let mut got = Vec::new();
        not_covered(&s, |p, v, why| got.push((p.id, v, why)));
        let effect = "not covered (an effect's parameter, not the engine's)";
        assert_eq!(
            got,
            vec![
                (4, 1, NO_TEMPO),
                (100, 18, NO_TYPE),
                (217, 1, NO_TEMPO),
                (224, 2, effect),
                (
                    309,
                    92,
                    "not covered (a destination without a law, or not the engine's)"
                )
            ]
        );
        let mut n = 0;
        not_covered(&Sound::default(), |_, _, _| n += 1);
        assert_eq!(n, 0);
    }
}
