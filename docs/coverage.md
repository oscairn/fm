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

At M2: the laws below are played on every routing, the operator forms included; nothing else.

| Parameter | Ids | Status | Range (default) | Law and accuracy |
|---|---|---|---|---|
| `random_pan` | 0 | M5 (filter and output) | 0 to 127 (0) |  |
| `alternate_pan` | 1 | M5 (filter and output) | -64 to 63 (0) |  |
| `scaling_pan` | 2 | M5 (filter and output) | -64 to 63 (0) |  |
| `key_on_delay` | 3 | M5 (filter and output) | 0 to 127 (0) |  |
| `key_on_delay_sync` | 4 | M5 (filter and output) | 0 to 1 (0) |  |
| `key_on_delay_length` | 5 | M5 (filter and output) | 5 to 21 (14) |  |
| `pitch_velocity` | 6 | M3 (pitch) | -64 to 63 (0) |  |
| `random_pitch` | 7 | M3 (pitch) | 0 to 127 (0) |  |
| `pitch_key_follow` | 8 | M3 (pitch) | 0 to 127 (96) |  |
| `pitch_key_centre` | 9 | M3 (pitch) | 0 to 127 (60) |  |
| `pitch_eg_level_initial` | 10 | M3 (pitch) | -50 to 50 (0) |  |
| `pitch_eg_level_attack` | 11 | M3 (pitch) | -50 to 50 (0) |  |
| `pitch_eg_level_decay1` | 12 | M3 (pitch) | -50 to 50 (0) |  |
| `pitch_eg_level_decay2` | 13 | M3 (pitch) | -50 to 50 (0) |  |
| `pitch_eg_level_release` | 14 | M3 (pitch) | -50 to 50 (0) |  |
| `pitch_eg_time_attack` | 15 | M3 (pitch) | 0 to 99 (0) |  |
| `pitch_eg_time_decay1` | 16 | M3 (pitch) | 0 to 99 (0) |  |
| `pitch_eg_time_decay2` | 17 | M3 (pitch) | 0 to 99 (0) |  |
| `pitch_eg_time_release` | 18 | M3 (pitch) | 0 to 99 (0) |  |
| `pitch_eg_depth_velocity` | 19 | M3 (pitch) | 0 to 7 (0) |  |
| `pitch_eg_depth` | 20 | M3 (pitch) | 0 to 3 (0) |  |
| `pitch_eg_time_key_follow` | 21 | M3 (pitch) | 0 to 7 (0) |  |
| `lfo_wave` | 22 | M4 (modulators) | 0 to 5 (0) |  |
| `lfo_speed` | 23 | M4 (modulators) | 0 to 99 (30) |  |
| `lfo_phase` | 24 | M4 (modulators) | 0 to 3 (0) |  |
| `lfo_delay` | 25 | M4 (modulators) | 0 to 99 (0) |  |
| `lfo_key_on_reset` | 26 | M4 (modulators) | 0 to 1 (0) |  |
| `lfo_pitch_depth` | 27 | M4 (modulators) | 0 to 99 (0) |  |
| `lfo_amp_depth` | 28 | M4 (modulators) | 0 to 99 (0) |  |
| `lfo_filter_depth` | 29 | M4 (modulators) | 0 to 99 (0) |  |
| `algorithm` | 30 | played | 1 to 88 (69) | the 88 routings as data, checked against the model's documented chart (routing 70 as the reference plays it: 1 and 2 both modulate 3); the carriers' output level per routing measured, 0 to +13.55 dB |
| `feedback` | 31 | played | 0 to 7 (0) | a modulator feeds back its own output: 2^(f - 9) cycles of phase per unit of the sum of its last two outputs; a carrier 0.18 of that, of its signal before the routing's level; existing profile: carrier within 0.4 dB, modulator harmonics 2-6 within 0.09 dB, 3 cases off by 2.5, 3.8 and 20 dB |
| `lfo_speed_range` | 32 | M4 (modulators) | 0 to 1 (1) |  |
| `lfo_extended_speed` | 33 | M4 (modulators) | 0 to 415 (60) |  |
| `macro_depth` | 34 | M4 (modulators) | -99 to 99 (0) |  |
| `macro_ratio` | 35 | M4 (modulators) | -99 to 99 (0) |  |
| `macro_attack` | 36 | M4 (modulators) | -99 to 99 (0) |  |
| `macro_decay` | 37 | M4 (modulators) | -99 to 99 (0) |  |
| `macro_sustain` | 38 | M4 (modulators) | -99 to 99 (0) |  |
| `macro_release` | 39 | M4 (modulators) | -99 to 99 (0) |  |
| `macro_texture` | 40 | M4 (modulators) | -99 to 99 (0) |  |
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
| `mono_poly` | 200 | M3 (pitch) | 0 to 1 (1) |  |
| `key_assign` | 201 | M3 (pitch) | 0 to 1 (1) |  |
| `portamento` | 202 | M3 (pitch) | 0 to 1 (0) |  |
| `portamento_time` | 203 | M3 (pitch) | 0 to 127 (64) |  |
| `portamento_mode` | 204 | M3 (pitch) | 0 to 1 (1) |  |
| `portamento_time_mode` | 205 | M3 (pitch) | 0 to 3 (0) |  |
| `bend_up` | 206 | M3 (pitch) | -48 to 24 (2) |  |
| `bend_down` | 207 | M3 (pitch) | -48 to 24 (-2) |  |
| `detune` | 208 | M3 (pitch) | -128 to 127 (0) |  |
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
| `amp_eg_attack_offset` | 249 | M4 (modulators) | -64 to 63 (0) |  |
| `amp_eg_decay_offset` | 250 | M4 (modulators) | -64 to 63 (0) |  |
| `amp_eg_sustain_offset` | 251 | M4 (modulators) | -64 to 63 (0) |  |
| `amp_eg_release_offset` | 252 | M4 (modulators) | -64 to 63 (0) |  |
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
| `opN.fixed_key_follow` | 1000 + 100 op + 45 | M3 (pitch) | 0 to 99 (0) |  |
| `opN.pitch_velocity` | 1000 + 100 op + 46 | M3 (pitch) | -7 to 7 (0) |  |
| `opN.form` | 1000 + 100 op + 47 | played | 0 to 6 (0) | sine; all harmonics 1 and 2 (one table); odd harmonics 1 and 2 (E(2t + m) sin(t + m)); resonant 1 and 2 (window(t + m) sin(centre t + m)); tables from the existing profile's measured amplitudes and phases, not band-limited (as the reference); the engine: every form and spread within 0.06 dB of the measured levels of its first 16 harmonics relative to a sine and its loudness within 0.05 dB of the measured (worst 0.009 and 0.034 dB), the tables' phases within 0.002 rad; modulated forms within 1 dB of three of the reference's renders (tests); existing profile: the four harmonic forms within 0.06 dB of the reference (first 16 harmonics), as modulators 100 of 111 cases within 1 dB; a resonant form whose centre is at or above Nyquist is silent (the reference plays a tone near Nyquist: docs/known-differences.md); a non-sine modulator's spectrum is not checked against the reference here |
| `opN.spread` | 1000 + 100 op + 48 | played | 0 to 7 (0) | a table per form and spread; as form; existing profile, resonant forms: spread 7 meets 10 of 24 formant cases (3 to over 100 dB off), resonant 2 at formant 0 off by 2-7 dB at spreads 4-5, low centres at spreads 4-7 off by up to 13 dB (inherited) |
| `opN.formant` | 1000 + 100 op + 49 | played | 0 to 99 (0) | the centre harmonic formant + 1; the engine: the strongest harmonic is the centre at formants 9-99 (spreads 0 and 3) and 18-99 (spread 7), and resonant 2 at spread 5, formant 0 plays harmonic 15 within 1.5 dB of the reference's -75.75 dB (tests); existing profile: spreads 0-3 48 of 48 (worst 0.58 dB), spread 7 as spread |
| `opN.pitch_eg_level_initial` | 1000 + 100 op + 50 | M3 (pitch) | -50 to 50 (0) |  |
| `opN.pitch_eg_level_attack` | 1000 + 100 op + 51 | M3 (pitch) | -50 to 50 (0) |  |
| `opN.pitch_eg_time_attack` | 1000 + 100 op + 52 | M3 (pitch) | 0 to 99 (0) |  |
| `opN.pitch_eg_time_decay` | 1000 + 100 op + 53 | M3 (pitch) | 0 to 99 (0) |  |
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
| `opN.scaling_break` | 1000 + 100 op + 65 | M4 (modulators) | 0 to 99 (39) |  |
| `opN.scaling_low_depth` | 1000 + 100 op + 66 | M4 (modulators) | 0 to 99 (0) |  |
| `opN.scaling_high_depth` | 1000 + 100 op + 67 | M4 (modulators) | 0 to 99 (0) |  |
| `opN.scaling_low_curve` | 1000 + 100 op + 68 | M4 (modulators) | 0 to 3 (0) |  |
| `opN.scaling_high_curve` | 1000 + 100 op + 69 | M4 (modulators) | 0 to 3 (0) |  |
| `opN.level_velocity` | 1000 + 100 op + 70 | played | -7 to 7 (0) | measured table at 17 velocities, fractional velocities between; existing profile: 56 of 64 (worst 1.5 dB at velocities 2-3) |
| `opN.lfo_pitch_offset` | 1000 + 100 op + 71 | M4 (modulators) | 0 to 7 (3) |  |
| `opN.lfo_amp_offset` | 1000 + 100 op + 72 | M4 (modulators) | 0 to 7 (3) |  |
| `opN.pitch_ctrl_sens` | 1000 + 100 op + 73 | M7 (controllers, tuning) | -7 to 7 (0) |  |
| `opN.level_ctrl_sens` | 1000 + 100 op + 74 | M7 (controllers, tuning) | -7 to 7 (0) |  |
| `opN.slfo_ratio1` | 1000 + 100 op + 75 | M4 (modulators) | 0 to 127 (127) |  |
| `opN.slfo_ratio2` | 1000 + 100 op + 76 | M4 (modulators) | 0 to 127 (127) |  |
| `opN.slfo_ratio3` | 1000 + 100 op + 77 | M4 (modulators) | 0 to 127 (127) |  |
