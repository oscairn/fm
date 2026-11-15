# Coverage

What the engine plays of the model, parameter by parameter: "played", or the milestone that
brings it (docs/design.md, section 7). `crates/fm-core/src/coverage.rs` holds the same status and a
test checks that every parameter has exactly one row here with that status. For a sound,
`fmbench coverage SOUND` lists its parameters at non-default values that are not played
("not covered: ID=VALUE: why"); the default of every parameter is played.

Operator parameters are listed once, as `opN.name` (operators 1-8, ids 1000 + 100 op + k with op
from 0). Range is the raw range and (the default). "Existing profile" figures are the existing
FM plugin profile's measured accuracy against the reference (docs/design.md, section 1), which
the laws here reuse; "the engine" figures are this repository's tests. Known differences from the
reference: docs/known-differences.md.

At M3: the laws below are played on every routing, the operator forms and pitch included; nothing
else. Pitch that moves (both pitch envelopes, portamento, bend) is evaluated every 16 frames, the
increments ramped linearly across a step while an envelope moves them (docs/design.md, 2.3); the
pitch a note keeps is on the grid of 1/2048 octave, what moves is not.

| Parameter | Ids | Status | Range (default) | Law and accuracy |
|---|---|---|---|---|
| `random_pan` | 0 | M5 (filter and output) | 0 to 127 (0) |  |
| `alternate_pan` | 1 | M5 (filter and output) | -64 to 63 (0) |  |
| `scaling_pan` | 2 | M5 (filter and output) | -64 to 63 (0) |  |
| `key_on_delay` | 3 | M5 (filter and output) | 0 to 127 (0) |  |
| `key_on_delay_sync` | 4 | M5 (filter and output) | 0 to 1 (0) |  |
| `key_on_delay_length` | 5 | M5 (filter and output) | 5 to 21 (14) |  |
| `pitch_velocity` | 6 | played | -64 to 63 (0) | measured depth table times a measured velocity curve, cents, every operator, on the grid; existing profile: worst 1.12 cents; the engine plays the law on the grid within 0.05 cent (test) |
| `random_pitch` | 7 | played | 0 to 127 (0) | whole steps of 18.75 cents, uniform within depth / 4 steps either way, cut towards 0, ratio-mode operators, a draw a note from the sound's own source (started again when the sound is installed); existing profile: the reference's extra step down at depths 32-127 not modelled (inherited); the engine: on the 18.75-cent steps within 0.6 cent, within 31 steps (test) |
| `pitch_key_follow` | 8 | played | 0 to 127 (96) | measured percent table (96 is 100 %), straight in cents about the centre key, ratio-mode operators; existing profile: 157 of 157 within 1 cent; the engine within 0.05 cent of the law on the grid (test) |
| `pitch_key_centre` | 9 | played | 0 to 127 (60) | as pitch_key_follow |
| `pitch_eg_level_initial` | 10 | played | -50 to 50 (0) | the common pitch envelope (ratio-mode operators): 37.5-cent steps at depth 8 octaves (wider beyond +-35), straight in cents between levels; existing profile: 101 of 101, worst 0.31 cents; the engine within 0.2 cent of the law at every block's end (test) |
| `pitch_eg_level_attack` | 11 | played | -50 to 50 (0) | as pitch_eg_level_initial |
| `pitch_eg_level_decay1` | 12 | played | -50 to 50 (0) | as pitch_eg_level_initial |
| `pitch_eg_level_decay2` | 13 | played | -50 to 50 (0) | as pitch_eg_level_initial; held while the key is down |
| `pitch_eg_level_release` | 14 | played | -50 to 50 (0) | as pitch_eg_level_initial; from the key's release |
| `pitch_eg_time_attack` | 15 | played | 0 to 99 (0) | a constant rate, a whole multiple of 0.05619 cents/ms (time 0 instant); existing profile: within 0.05 % of the multiples, worst 0.33 %; the engine within 0.2 cent of the law (test) |
| `pitch_eg_time_decay1` | 16 | played | 0 to 99 (0) | as pitch_eg_time_attack |
| `pitch_eg_time_decay2` | 17 | played | 0 to 99 (0) | as pitch_eg_time_attack |
| `pitch_eg_time_release` | 18 | played | 0 to 99 (0) | as pitch_eg_time_attack |
| `pitch_eg_depth_velocity` | 19 | played | 0 to 7 (0) | levels and rates times the measured share left at the velocity (times unchanged); existing profile: met, worst 0.22 cents; the engine within 0.2 cent (test) |
| `pitch_eg_depth` | 20 | played | 0 to 3 (0) | 1, 1/4, 1/8, 1/16 of the widest (8, 2, 1, 0.5 octaves); existing profile: met; the engine within 0.2 cent (test) |
| `pitch_eg_time_key_follow` | 21 | played | 0 to 7 (0) | rates times 1 + s (key - 3) / 33; existing profile: within 0.6 %; the engine within 0.2 cent (test) |
| `lfo_wave` | 22 | played | 0 to 5 (0) | the voice LFO, one a voice, evaluated every 16 frames (docs/design.md, 2.3): triangle, saw down, saw up, square, sine, sample and hold (a new value each cycle), from phase 0 the triangle and saw down at their top, saw up and square at their bottom (as measured); existing profile: starts and shapes 6.3-14.2 cents (95th percentile), the square 61 and the sine 18.9; the engine's pitch route within 0.2 cent of the per-sample law (test) |
| `lfo_speed` | 23 | played | 0 to 99 (30) | the normal range's measured rate table (speed range normal only); existing profile: 100 of 100 within 1 %, speed 0 unmeasured |
| `lfo_phase` | 24 | played | 0 to 3 (0) | starts a quarter of a cycle further per value; as lfo_wave |
| `lfo_delay` | 25 | played | 0 to 99 (0) | the six-operator model's hold, then a fade in as long (straight, at control rate); existing profile: 11 of 12, delay 99 reaching 10 % 2.5 % early; the engine: nothing in the hold, the full swing after the fade (test) |
| `lfo_key_on_reset` | 26 | played | 0 to 1 (0) | on: each note's LFO from its start phase; off: a note takes the sound's free-running phase (from the engine's clock at the note's block; chosen: the reference's is not modelled) |
| `lfo_pitch_depth` | 27 | played | 0 to 99 (0) | twice the six-operator depth (depth × 165 / 64 steps, 99 as 256) times each operator's offset, ratio-mode operators; existing profile: 30 of 31 within 1 cent (depth 80 +1.3 cents); the engine within 0.2 cent of the law (test) |
| `lfo_amp_depth` | 28 | played | 0 to 99 (0) | each operator loses, straight in dB from the LFO's top, the measured offset-7 swing times its offset's eighths, through the carrier envelope's bend below about 45 dB; existing profile: within 0.16 dB on the renders it was fitted to, depth 99 at offsets 3-4 0.8 and 2.6 dB short; the engine: 6 dB at depth 30 within 0.1 dB (test) |
| `lfo_filter_depth` | 29 | M5 (filter and output) | 0 to 99 (0) |  |
| `algorithm` | 30 | played | 1 to 88 (69) | the 88 routings as data, checked against the model's documented chart (routing 70 as the reference plays it: 1 and 2 both modulate 3); the carriers' output level per routing measured, 0 to +13.55 dB |
| `feedback` | 31 | played | 0 to 7 (0) | a modulator feeds back its own output: 2^(f - 9) cycles of phase per unit of the sum of its last two outputs; a carrier 0.18 of that, of its signal before the routing's level; existing profile: carrier within 0.4 dB, modulator harmonics 2-6 within 0.09 dB, 3 cases off by 2.5, 3.8 and 20 dB |
| `lfo_speed_range` | 32 | played | 0 to 1 (1) | normal or extended; as lfo_speed and lfo_extended_speed |
| `lfo_extended_speed` | 33 | played | 0 to 415 (60) | the extended range's measured rate table, every fifth value, straight between (0.002 to 1357 Hz); existing profile: every fifth value checked; evaluated every 16 frames, so above about 150 Hz the LFO is under-sampled (docs/known-differences.md) |
| `macro_depth` | 34 | played | -99 to 99 (0) | added to modulators' levels, divided by 1 + feedback (cut towards 0) on the operator that feeds back, per sound; existing profile: 12 of 12 without feedback, 0.02-1.25 dB with; the engine plays the law (test) |
| `macro_ratio` | 35 | played | -99 to 99 (0) | ratio-mode modulators: a measured table at 9 ratios and 79 values, straight between both, at least 0.5, then on the grid; existing profile: up to 5.7 cents between measured points; fixed-mode operators unmeasured (not moved); the engine plays the law (test) |
| `macro_attack` | 36 | played | -99 to 99 (0) | added to modulators' attack time values (0-99); existing profile: 44 of 48 simple probes, 4 of 24 on the wave probe (the envelope macros together); the engine plays the law (test) |
| `macro_decay` | 37 | played | -99 to 99 (0) | added to modulators' decay 1 and decay 2 times; as macro_attack |
| `macro_sustain` | 38 | played | -99 to 99 (0) | added to modulators' decay 2 levels; as macro_attack |
| `macro_release` | 39 | played | -99 to 99 (0) | added to modulators' release times; as macro_attack |
| `macro_texture` | 40 | played | -99 to 99 (0) | moves the forms of the carriers (below 0) or the modulators (above) along the measured path of form places, coming round past +-99, per sound; existing profile: extremes off by 16-25 dB, -99 by 22.6 dB, the wrap unmeasured (inherited); the engine plays the law (test) |
| `filter_type` | 100 | M5 (filter and output) | 0 to 21 (21) |  |
| `cutoff` | 101 | M5 (filter and output) | 0 to 1023 (1023) |  |
| `cutoff_velocity` | 102 | M5 (filter and output) | -64 to 63 (0) |  |
| `resonance` | 103 | M5 (filter and output) | 0 to 127 (10) |  |
| `resonance_velocity` | 104 | M5 (filter and output) | -64 to 63 (0) |  |
| `hpf_cutoff` | 105 | M5 (filter and output) | 0 to 1023 (0) |  |
| `distance` | 106 | M5 (filter and output) | -128 to 127 (0) |  |
| `filter_gain` | 107 | M5 (filter and output) | 0 to 255 (255) |  |
| `filter_eg_time_hold` | 108 | M5 (filter and output) | 0 to 127 (0) |  |
| `filter_eg_time_attack` | 109 | M5 (filter and output) | 0 to 127 (0) |  |
| `filter_eg_time_decay1` | 110 | M5 (filter and output) | 0 to 127 (0) |  |
| `filter_eg_time_decay2` | 111 | M5 (filter and output) | 0 to 127 (0) |  |
| `filter_eg_time_release` | 112 | M5 (filter and output) | 0 to 127 (0) |  |
| `filter_eg_level_hold` | 113 | M5 (filter and output) | -128 to 127 (0) |  |
| `filter_eg_level_attack` | 114 | M5 (filter and output) | -128 to 127 (0) |  |
| `filter_eg_level_decay1` | 115 | M5 (filter and output) | -128 to 127 (0) |  |
| `filter_eg_level_decay2` | 116 | M5 (filter and output) | -128 to 127 (0) |  |
| `filter_eg_level_release` | 117 | M5 (filter and output) | -128 to 127 (0) |  |
| `filter_eg_depth` | 118 | M5 (filter and output) | -64 to 63 (40) |  |
| `filter_eg_time_velocity_segment` | 119 | M5 (filter and output) | 0 to 4 (4) |  |
| `filter_eg_time_velocity` | 120 | M5 (filter and output) | -64 to 63 (0) |  |
| `filter_eg_depth_velocity` | 121 | M5 (filter and output) | -64 to 63 (0) |  |
| `filter_eg_depth_velocity_curve` | 122 | M5 (filter and output) | 0 to 4 (2) |  |
| `filter_eg_time_key_follow` | 123 | M5 (filter and output) | -64 to 63 (0) |  |
| `filter_eg_time_key_centre` | 124 | M5 (filter and output) | 0 to 127 (24) |  |
| `cutoff_key_follow` | 125 | M5 (filter and output) | -64 to 63 (10) |  |
| `cutoff_break1` | 126 | M5 (filter and output) | 0 to 124 (36) |  |
| `cutoff_break2` | 127 | M5 (filter and output) | 1 to 125 (48) |  |
| `cutoff_break3` | 128 | M5 (filter and output) | 2 to 126 (60) |  |
| `cutoff_break4` | 129 | M5 (filter and output) | 3 to 127 (72) |  |
| `cutoff_offset1` | 130 | M5 (filter and output) | -128 to 127 (0) |  |
| `cutoff_offset2` | 131 | M5 (filter and output) | -128 to 127 (0) |  |
| `cutoff_offset3` | 132 | M5 (filter and output) | -128 to 127 (0) |  |
| `cutoff_offset4` | 133 | M5 (filter and output) | -128 to 127 (0) |  |
| `hpf_key_follow` | 134 | M5 (filter and output) | -64 to 63 (0) |  |
| `mono_poly` | 200 | played | 0 to 1 (1) | mono: one voice, last-note priority; with portamento a note whose predecessor's key is down takes over its voice (legato: amplitude envelopes, common pitch envelope and phases go on; the operator pitch envelopes start again), otherwise the sounding note fades out as a retrigger (13.3 ms, gain the square of the share left) while the new one starts; releasing the sounding key returns to the newest key still down (the same two ways); a held mono key is never shed; existing profile: 18 of 18 and 12 of 13, the fade about 1 ms ahead (inherited); the engine: every note ends exactly once (tests, fuzz) |
| `key_assign` | 201 | played | 0 to 1 (1) | single (poly): a key played again retriggers its sounding note (the same fade, unmeasured); multi: both sound (test) |
| `portamento` | 202 | played | 0 to 1 (1) | on (the documented default): from the pitch the sound's latest note has now, ratio-mode operators only; fingered glides only when another key of the sound is down |
| `portamento_time` | 203 | played | 0 to 127 (64) | measured rate table every fourth value, straight in log rate between, straight in semitones, in steps of 9.977 ms of a clock shared by every sound (the reference's period; its phase against a note is not known); existing profile: 22 of 29 and 27 of 28 within 2 % (fast times slower, inherited); the engine: the steps and the glide's time within two steps (test) |
| `portamento_mode` | 204 | played | 0 to 1 (1) | fingered, full-time; as portamento (test) |
| `portamento_time_mode` | 205 | played | 0 to 3 (0) | rate 1 keeps the rate; time 1 takes as long for any distance as rate 1 for an octave; rate 2 and time 2 start at most an octave away; existing profile: measured at 5, 12, 19, 24 semitones; the engine (test) |
| `bend_up` | 206 | played | -48 to 24 (2) | the pitch at full bend up (+8191), whole cents then whole steps, each cut towards 0, every operator, from the event's frame; documented, not measured (the existing profile widened the range to +-127: docs/known-differences.md); the engine (test) |
| `bend_down` | 207 | played | -48 to 24 (-2) | the pitch at full bend down (-8192), as bend_up |
| `detune` | 208 | played | -128 to 127 (0) | cents, not hertz (the same at every key): measured every eighth value, nothing within 2 of 0, straight between, ratio-mode operators; existing profile: 37 of 49 within 1 cent (inherited); the engine within 0.05 cent of the law on the grid (test) |
| `note_shift` | 209 | played | -48 to 48 (0) | whole semitones before the pitch grid; the reference's range (+-48) |
| `tuning_scale` | 210 | M7 (controllers, tuning) | 0 to 212 (0) |  |
| `tuning_root` | 211 | M7 (controllers, tuning) | 0 to 11 (0) |  |
| `velocity_depth` | 212 | played | 0 to 127 (64) | v d / 64 + 2 (o - 64), at most 127; 0 or below plays no note (it ends at once); fractions go on to the level velocity law; existing profile: 40 combinations measured |
| `velocity_offset` | 213 | played | 0 to 127 (64) | as velocity_depth |
| `slfo_phase` | 214 | M4 (modulators) | 0 to 5 (0) |  |
| `slfo_wave` | 215 | M4 (modulators) | 0 to 12 (0) |  |
| `slfo_speed` | 216 | M4 (modulators) | 0 to 63 (32) |  |
| `slfo_tempo_sync` | 217 | M4 (modulators) | 0 to 1 (0) |  |
| `slfo_tempo_speed` | 218 | M4 (modulators) | 5 to 24 (11) |  |
| `slfo_delay` | 219 | M4 (modulators) | 0 to 127 (0) |  |
| `slfo_fade_in` | 220 | M4 (modulators) | 0 to 127 (0) |  |
| `slfo_hold` | 221 | M4 (modulators) | 0 to 127 (127) |  |
| `slfo_fade_out` | 222 | M4 (modulators) | 0 to 127 (64) |  |
| `slfo_key_on_reset` | 223 | M4 (modulators) | 0 to 2 (2) |  |
| `slfo_dest1` | 224 | M4 (modulators) | 0 to 69 (2) |  |
| `slfo_depth1` | 225 | M4 (modulators) | 0 to 127 (0) |  |
| `slfo_dest2` | 226 | M4 (modulators) | 0 to 69 (4) |  |
| `slfo_depth2` | 227 | M4 (modulators) | 0 to 127 (0) |  |
| `slfo_dest3` | 228 | M4 (modulators) | 0 to 69 (4) |  |
| `slfo_depth3` | 229 | M4 (modulators) | 0 to 127 (0) |  |
| `slfo_user_cycle` | 230 | M4 (modulators) | 0 to 6 (6) |  |
| `slfo_user_slope` | 231 | M4 (modulators) | 0 to 3 (0) |  |
| `slfo_step1` | 232 | M4 (modulators) | -64 to 63 (0) |  |
| `slfo_step2` | 233 | M4 (modulators) | -64 to 63 (0) |  |
| `slfo_step3` | 234 | M4 (modulators) | -64 to 63 (0) |  |
| `slfo_step4` | 235 | M4 (modulators) | -64 to 63 (0) |  |
| `slfo_step5` | 236 | M4 (modulators) | -64 to 63 (0) |  |
| `slfo_step6` | 237 | M4 (modulators) | -64 to 63 (0) |  |
| `slfo_step7` | 238 | M4 (modulators) | -64 to 63 (0) |  |
| `slfo_step8` | 239 | M4 (modulators) | -64 to 63 (0) |  |
| `slfo_step9` | 240 | M4 (modulators) | -64 to 63 (0) |  |
| `slfo_step10` | 241 | M4 (modulators) | -64 to 63 (0) |  |
| `slfo_step11` | 242 | M4 (modulators) | -64 to 63 (0) |  |
| `slfo_step12` | 243 | M4 (modulators) | -64 to 63 (0) |  |
| `slfo_step13` | 244 | M4 (modulators) | -64 to 63 (0) |  |
| `slfo_step14` | 245 | M4 (modulators) | -64 to 63 (0) |  |
| `slfo_step15` | 246 | M4 (modulators) | -64 to 63 (0) |  |
| `slfo_step16` | 247 | M4 (modulators) | -64 to 63 (0) |  |
| `slfo_random_speed` | 248 | M4 (modulators) | 0 to 127 (0) |  |
| `amp_eg_attack_offset` | 249 | played | -64 to 63 (0) | every operator's attack time times 2^(offset / 4), held within attack times 0-99's, a positive offset on carriers only; existing profile: within one time value from -12 to +26, faster below -12, sooner at +28 and +30 (up to 5.6 dB); the engine plays the law (test) |
| `amp_eg_decay_offset` | 250 | played | -64 to 63 (0) | 1.5 time values a step (rounded) added to every operator's decay 1 and decay 2 times; existing profile: 18 of 18 offsets within 0.3 dB (with release); the engine plays the law (test) |
| `amp_eg_sustain_offset` | 251 | played | -64 to 63 (0) | decay 2 level steps from a measured table at even offsets -22 to +32 (odd ones halfway, 1.5 a step beyond, unmeasured); existing profile: 1.2-2.3 dB off; the engine plays the law (test) |
| `amp_eg_release_offset` | 252 | played | -64 to 63 (0) | as amp_eg_decay_offset, on release times; existing profile: -8 off by 1.0 dB |
| `filter_eg_depth_offset` | 253 | M5 (filter and output) | -64 to 63 (0) |  |
| `cutoff_offset` | 254 | M5 (filter and output) | -64 to 63 (0) |  |
| `resonance_offset` | 255 | M5 (filter and output) | -64 to 63 (0) |  |
| `opN.ctrl_set1` | 1000 + 100 op + 0 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.ctrl_set2` | 1000 + 100 op + 1 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.ctrl_set3` | 1000 + 100 op + 2 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.ctrl_set4` | 1000 + 100 op + 3 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.ctrl_set5` | 1000 + 100 op + 4 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.ctrl_set6` | 1000 + 100 op + 5 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.ctrl_set7` | 1000 + 100 op + 6 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.ctrl_set8` | 1000 + 100 op + 7 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.ctrl_set9` | 1000 + 100 op + 8 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.ctrl_set10` | 1000 + 100 op + 9 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.ctrl_set11` | 1000 + 100 op + 10 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.ctrl_set12` | 1000 + 100 op + 11 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.ctrl_set13` | 1000 + 100 op + 12 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.ctrl_set14` | 1000 + 100 op + 13 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.ctrl_set15` | 1000 + 100 op + 14 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.ctrl_set16` | 1000 + 100 op + 15 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.ctrl_set17` | 1000 + 100 op + 16 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.ctrl_set18` | 1000 + 100 op + 17 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.ctrl_set19` | 1000 + 100 op + 18 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.ctrl_set20` | 1000 + 100 op + 19 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.ctrl_set21` | 1000 + 100 op + 20 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.ctrl_set22` | 1000 + 100 op + 21 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.ctrl_set23` | 1000 + 100 op + 22 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.ctrl_set24` | 1000 + 100 op + 23 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.ctrl_set25` | 1000 + 100 op + 24 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.ctrl_set26` | 1000 + 100 op + 25 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.ctrl_set27` | 1000 + 100 op + 26 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.ctrl_set28` | 1000 + 100 op + 27 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.ctrl_set29` | 1000 + 100 op + 28 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.ctrl_set30` | 1000 + 100 op + 29 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.ctrl_set31` | 1000 + 100 op + 30 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.ctrl_set32` | 1000 + 100 op + 31 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.key_ctrl_set1` | 1000 + 100 op + 32 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.key_ctrl_set2` | 1000 + 100 op + 33 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.key_ctrl_set3` | 1000 + 100 op + 34 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.key_ctrl_set4` | 1000 + 100 op + 35 | M7 (controllers, tuning) | 0 to 1 (1) |  |
| `opN.slfo_box1` | 1000 + 100 op + 36 | M4 (modulators) | 0 to 1 (1) |  |
| `opN.slfo_box2` | 1000 + 100 op + 37 | M4 (modulators) | 0 to 1 (1) |  |
| `opN.slfo_box3` | 1000 + 100 op + 38 | M4 (modulators) | 0 to 1 (1) |  |
| `opN.key_on_reset` | 1000 + 100 op + 40 | played | 0 to 1 (1) | on: phase 0 at key-on; off: the slot's phase runs on from its last note (the reference's free-running phase not modelled) |
| `opN.mode` | 1000 + 100 op + 41 | played | 0 to 1 (0) | ratio or fixed, as below |
| `opN.coarse` | 1000 + 100 op + 42 | played | 0 to 31 (1) | the pitch grid of 1/2048 octave: keys rounded up to the 1/1024-octave grid from A, ratios to their nearest 1/1024-octave step (coarse 0-31 and fine 0-99 measured tables, fine 100 and above carries; past coarse 31 0.5, then a fixed ratio), fixed frequencies on 1/128-octave steps from A4's step (440.169 Hz), held at 22.04 kHz; existing profile: 1 cent met; the engine plays the grid within 0.05 cent (test) |
| `opN.fine` | 1000 + 100 op + 43 | played | 0 to 127 (0) | as coarse |
| `opN.detune` | 1000 + 100 op + 44 | played | -15 to 15 (0) | measured table by key (cents), on 1/1024-octave steps; fixed-mode operators too; existing profile: 5 of 20 one grid step off, up to 1.18 cents |
| `opN.fixed_key_follow` | 1000 + 100 op + 45 | played | 0 to 99 (0) | fixed-mode operators: measured cents a key about key 60, on the grid; existing profile: worst 1.77 cents; the engine within 0.05 cent of the law (test) |
| `opN.pitch_velocity` | 1000 + 100 op + 46 | played | -7 to 7 (0) | fixed-mode operators: the measured cents at velocity 127, straight in velocity from 64 (4 and 6 halfway, unmeasured); ratio-mode operators do not move (measured); the engine within 0.05 cent of the law (test) |
| `opN.form` | 1000 + 100 op + 47 | played | 0 to 6 (0) | sine; all harmonics 1 and 2 (one table); odd harmonics 1 and 2 (E(2t + m) sin(t + m)); resonant 1 and 2 (window(t + m) sin(centre t + m)); tables from the existing profile's measured amplitudes and phases, not band-limited (as the reference); the engine: every form and spread within 0.06 dB of the measured levels of its first 16 harmonics relative to a sine and its loudness within 0.05 dB of the measured (worst 0.009 and 0.034 dB), the tables' phases within 0.002 rad; modulated forms within 1 dB of three of the reference's renders (tests); existing profile: the four harmonic forms within 0.06 dB of the reference (first 16 harmonics), as modulators 100 of 111 cases within 1 dB; a resonant form whose centre is at or above Nyquist is silent (the reference plays a tone near Nyquist: docs/known-differences.md); a non-sine modulator's spectrum is not checked against the reference here |
| `opN.spread` | 1000 + 100 op + 48 | played | 0 to 7 (0) | a table per form and spread; as form; existing profile, resonant forms: spread 7 meets 10 of 24 formant cases (3 to over 100 dB off), resonant 2 at formant 0 off by 2-7 dB at spreads 4-5, low centres at spreads 4-7 off by up to 13 dB (inherited) |
| `opN.formant` | 1000 + 100 op + 49 | played | 0 to 99 (0) | the centre harmonic formant + 1; the engine: the strongest harmonic is the centre at formants 9-99 (spreads 0 and 3) and 18-99 (spread 7), and resonant 2 at spread 5, formant 0 plays harmonic 15 within 1.5 dB of the reference's -75.75 dB (tests); existing profile: spreads 0-3 48 of 48 (worst 0.58 dB), spread 7 as spread |
| `opN.pitch_eg_level_initial` | 1000 + 100 op + 50 | played | -50 to 50 (0) | the operator's pitch envelope (fixed-mode operators too): from the initial level to the attack level, then to 0, straight in cents, measured level table; existing profile: 101 of 101; the engine within 0.2 cent of the law at every block's end, its phase within 0.0022 cycles of the per-sample law at the fastest time (tests) |
| `opN.pitch_eg_level_attack` | 1000 + 100 op + 51 | played | -50 to 50 (0) | as pitch_eg_level_initial |
| `opN.pitch_eg_time_attack` | 1000 + 100 op + 52 | played | 0 to 99 (0) | a constant rate in cents/ms, measured table; existing profile: within 2 %, times 90-99 unmeasured (inherited); as pitch_eg_level_initial |
| `opN.pitch_eg_time_decay` | 1000 + 100 op + 53 | played | 0 to 99 (0) | as pitch_eg_time_attack |
| `opN.eg_level_attack` | 1000 + 100 op + 54 | played | 0 to 99 (99) | carrier curve (from 20 up in steps of two) and modulator curve (0.75 dB a step, in steps of two); the carrier table checked only against the renders it was fitted to |
| `opN.eg_level_decay1` | 1000 + 100 op + 55 | played | 0 to 99 (99) | as eg_level_attack |
| `opN.eg_level_decay2` | 1000 + 100 op + 56 | played | 0 to 99 (99) | as eg_level_attack |
| `opN.eg_level_release` | 1000 + 100 op + 57 | played | 0 to 99 (0) | as eg_level_attack; also the level a note starts and holds at |
| `opN.eg_time_attack` | 1000 + 100 op + 58 | played | 0 to 99 (0) | fitted rate table; a rising segment approaches 1.52 dB above full level exponentially in dB, the attack starting at most 56.3 dB below full; existing profile: within 2 %, the reference's fast segments read up to 15 % slower; the engine evaluates envelopes every 16 frames (exact at the steps, linear gain ramps between) |
| `opN.eg_time_decay1` | 1000 + 100 op + 59 | played | 0 to 99 (0) | fitted time table, straight in dB; existing profile within 2 %; the engine within 2 % of the table at times 30, 50, 70 (test) |
| `opN.eg_time_decay2` | 1000 + 100 op + 60 | played | 0 to 99 (0) | as eg_time_decay1 |
| `opN.eg_time_release` | 1000 + 100 op + 61 | played | 0 to 99 (40) | as eg_time_decay1 |
| `opN.eg_time_hold` | 1000 + 100 op + 62 | played | 0 to 99 (0) | fitted time table; in whole 16-frame control steps (0.33 ms at 48 kHz); the engine within 2 % at times 30, 50, 70 (test) |
| `opN.eg_time_key_follow` | 1000 + 100 op + 63 | played | 0 to 7 (0) | 8 steps a rate doubling, keys in groups of three around keys 64-69, every segment but the hold; existing profile: 90 of 151 within 3 % (the rest the fast-segment reading); the engine within 2 % at sensitivity 7, key 105 (test) |
| `opN.level` | 1000 + 100 op + 64 | played | 0 to 99 (0) | carrier curve; modulator curve, 0.75 dB a step, modulation index 4 pi at 0 dB; existing profile: index within 0.01 dB at 32 levels; the engine's J1/J0 within 1 % at levels 40, 60, 75 and a lone carrier within 0.02 dB of -17.64 dBFS (tests) |
| `opN.scaling_break` | 1000 + 100 op + 65 | played | 0 to 99 (39) | documented, not measured: the six-operator model's level scaling (break point A-1 to C8, keys in groups of three, linear and exponential curves in its 0.7526 dB output level steps), added in dB at note-on at the key after note shift, raising a level to level 99's at most (as the existing profile, also unmeasured); the engine plays the documented values (test) |
| `opN.scaling_low_depth` | 1000 + 100 op + 66 | played | 0 to 99 (0) | as opN.scaling_break, below it |
| `opN.scaling_high_depth` | 1000 + 100 op + 67 | played | 0 to 99 (0) | as opN.scaling_break, above it |
| `opN.scaling_low_curve` | 1000 + 100 op + 68 | played | 0 to 3 (0) | as opN.scaling_break: - linear, - exponential, + exponential, + linear |
| `opN.scaling_high_curve` | 1000 + 100 op + 69 | played | 0 to 3 (0) | as opN.scaling_low_curve |
| `opN.level_velocity` | 1000 + 100 op + 70 | played | -7 to 7 (0) | measured table at 17 velocities, fractional velocities between; existing profile: 56 of 64 (worst 1.5 dB at velocities 2-3) |
| `opN.lfo_pitch_offset` | 1000 + 100 op + 71 | played | 0 to 7 (3) | the operator's share of the voice LFO's pitch depth: 0, 10, 20, 33, 56, 96, 160, 256 steps (measured at depths 30 and 99); fixed-mode operators do not follow (as the existing profile); as lfo_pitch_depth |
| `opN.lfo_amp_offset` | 1000 + 100 op + 72 | played | 0 to 7 (3) | the operator's share of the voice LFO's amplitude loss: 0, 1, 2, 4, 5, 6, 7, 8 eighths (measured at depths 16, 30, 70 and 99); as lfo_amp_depth |
| `opN.pitch_ctrl_sens` | 1000 + 100 op + 73 | M7 (controllers, tuning) | -7 to 7 (0) |  |
| `opN.level_ctrl_sens` | 1000 + 100 op + 74 | M7 (controllers, tuning) | -7 to 7 (0) |  |
| `opN.slfo_ratio1` | 1000 + 100 op + 75 | M4 (modulators) | 0 to 127 (127) |  |
| `opN.slfo_ratio2` | 1000 + 100 op + 76 | M4 (modulators) | 0 to 127 (127) |  |
| `opN.slfo_ratio3` | 1000 + 100 op + 77 | M4 (modulators) | 0 to 127 (127) |  |
