# Oscairn FM: plan

## 1. Status

Version 0.6.0. The coverage below was measured with version 0.3.1; the commits since then change the plugin id, its
shown name, the version and the README only, so the engine's coverage should be the same, but that is unverified.
Implementation language today: Rust (a platform-free core and a plugin shell). Everything in Oscairn's plugin set is to end up in Rust: the engines port after their coverage work, and new effect types are written in Rust from the start.

## 2. Supported

What the engine plays of the reference's FM engine today, in short. Parameter by parameter:
[coverage.md](coverage.md); design and milestones: [design.md](design.md); the plugin: [plugin.md](plugin.md); the
sound format: [sound-format.md](sound-format.md); differences from the reference:
[known-differences.md](known-differences.md).

| Area | Played today | Not yet |
|---|---|---|
| Operators | eight operators, the seven operator forms (sine, all harmonics, odd harmonics, resonant, with spread and formant), the 88 routings, feedback, level and envelope laws, level scaling | the level controller sensitivity |
| Pitch | ratio and fixed frequencies, the operator and common pitch envelopes, key follow, velocity, random pitch, detune, bend, portamento, mono and key assign | micro tuning |
| Filter | the filter types with the filter envelope | the output high-pass |
| LFOs | the voice LFO; the sound LFO with all waves, destinations and the user wave | the sound LFO's tempo sync (not measured) and random speed |
| Modulation | the seven macros; the channel controllers and note volume; controller boxes on the operator level and frequency, cutoff, resonance, the voice LFO's speed and depths, and the pitch | the other box destinations; Key Controller Sets |
| Voices and output | random, alternate and scaling pan; key-on delay time and note length (tempo sync at the host's tempo); stereo output; part envelope offsets | |

## 3. Coverage

Measured with FM 0.3.1 on 2026-11-17. Two sets of Performances of the reference were converted: its factory (stock) set of 3631 Performances and a personal library of 87 Performances. "Fully covered": the engine's own coverage report lists nothing for that part that the engine does not play; it says nothing about effects, controls, scenes or sequences. "Best effort": the part plays, with at least one item reported. Neither set has a Performance with no sound-affecting drop yet (0 of 3631, 0 of 87): every Performance still needs work somewhere, in this plugin or elsewhere.

| Parts on this engine | Total | Fully covered | Best effort | Muted |
|---|---|---|---|---|
| Stock set | 1457 | 64 | 1393 | 0 |
| Personal library | 24 | 0 | 24 | 0 |

In the personal library, the FM parts' mod-wheel controller boxes: 16 carried in the engine's sound, 6 dropped.

### Gaps

50 gaps of the stock set belong to this engine (operator parameters per operator, `opN` = operator N). "Changes the sound" is a classification of the gap, not a listening test: a stored value away from its default that is not carried counts as sound-affecting whether or not it is heard in a given Performance. Counts are Performances and parts of the stock set that carry the gap; parts switched off or muted are not counted against their other gaps.

| Gap | Performances (stock) | Parts (stock) | Changes the sound | Status |
|---|---|---|---|---|
| `key_on_delay_length`: not played | 552 | 1259 | yes | done (0.6.2): played with tempo sync; with tempo sync off (all but 3 of these parts) it changes nothing, and is no longer reported |
| `boxN_dst`: not played | 407 | 781 | yes | partly done (0.6.3, 0.6.4): 52, 53, 56-59, 88, 92, 95-98, 104, 107-115, 117 played (measured); counted again with the engine's report on the stock set's 1450 FM parts read through the converter, the parts with an unplayed destination went from 651 to 33 (Performances 350 to 29); left: 100 and 101 the filter envelope's times (15), 116 the operators' pitch envelope attack (5), 93, 94 and 99 (7), and 377-379 (not the engine's: 13) |
| `op5.level_ctrl_sens`: not played | 41 | 64 | yes | done (0.6.3): the operator level offset boxes' share (measured) |
| `op7.level_ctrl_sens`: not played | 40 | 62 | yes | done (0.6.3): the operator level offset boxes' share (measured) |
| `op3.level_ctrl_sens`: not played | 37 | 59 | yes | done (0.6.3): the operator level offset boxes' share (measured) |
| `op2.level_ctrl_sens`: not played | 35 | 44 | yes | done (0.6.3): the operator level offset boxes' share (measured) |
| `op1.level_ctrl_sens`: not played | 31 | 49 | yes | done (0.6.3): the operator level offset boxes' share (measured) |
| `op4.level_ctrl_sens`: not played | 30 | 50 | yes | done (0.6.3): the operator level offset boxes' share (measured) |
| `op6.level_ctrl_sens`: not played | 25 | 41 | yes | done (0.6.3): the operator level offset boxes' share (measured) |
| `slfo_dest1`: not played | 24 | 41 | yes | open |
| `op8.level_ctrl_sens`: not played | 15 | 18 | yes | done (0.6.3): the operator level offset boxes' share (measured) |
| `op1.key_ctrl_set1`: not played | 11 | 26 | yes | open |
| `op3.key_ctrl_set1`: not played | 11 | 26 | yes | open |
| `op7.key_ctrl_set1`: not played | 11 | 23 | yes | open |
| `slfo_tempo_speed`: not played | 11 | 18 | yes | open; since 0.6.2 reported only with `slfo_tempo_sync` on (off, it changes nothing) |
| `op2.key_ctrl_set2`: not played | 10 | 29 | yes | open |
| `op5.key_ctrl_set2`: not played | 10 | 27 | yes | open |
| `op8.key_ctrl_set2`: not played | 10 | 27 | yes | open |
| `op6.key_ctrl_set2`: not played | 9 | 23 | yes | open |
| `op8.key_ctrl_set1`: not played | 9 | 23 | yes | open |
| `op5.key_ctrl_set1`: not played | 9 | 22 | yes | open |
| `slfo_tempo_sync`: not played | 9 | 19 | yes | open |
| `slfo_random_speed`: not played | 9 | 13 | yes | open |
| `op7.key_ctrl_set2`: not played | 8 | 23 | yes | open |
| `op1.key_ctrl_set2`: not played | 8 | 21 | yes | open |
| `op4.key_ctrl_set1`: not played | 8 | 21 | yes | open |
| `op2.key_ctrl_set1`: not played | 8 | 18 | yes | open |
| `op4.key_ctrl_set2`: not played | 7 | 19 | yes | open |
| `op3.key_ctrl_set2`: not played | 7 | 16 | yes | open |
| `op6.key_ctrl_set1`: not played | 7 | 16 | yes | open |
| `slfo_dest2`: not played | 6 | 11 | yes | open |
| `op1.key_ctrl_set3`: not played | 5 | 16 | yes | open |
| `op7.key_ctrl_set3`: not played | 5 | 16 | yes | open |
| `op8.key_ctrl_set3`: not played | 5 | 16 | yes | open |
| `op4.key_ctrl_set3`: not played | 4 | 14 | yes | open |
| `op2.key_ctrl_set3`: not played | 4 | 12 | yes | open |
| `op2.key_ctrl_set4`: not played | 4 | 11 | yes | open |
| `op6.key_ctrl_set3`: not played | 3 | 10 | yes | open |
| `op1.key_ctrl_set4`: not played | 3 | 9 | yes | open |
| `op3.key_ctrl_set4`: not played | 3 | 9 | yes | open |
| `op4.key_ctrl_set4`: not played | 3 | 9 | yes | open |
| `op5.key_ctrl_set4`: not played | 3 | 9 | yes | open |
| `op6.key_ctrl_set4`: not played | 3 | 9 | yes | open |
| `op7.key_ctrl_set4`: not played | 3 | 9 | yes | open |
| `op5.key_ctrl_set3`: not played | 2 | 6 | yes | open |
| `op3.key_ctrl_set3`: not played | 1 | 4 | yes | open |
| `op8.key_ctrl_set4`: not played | 1 | 4 | yes | open |
| `key_on_delay_sync`: not played | 1 | 3 | yes | done (0.6.2): the note length at the host's tempo (the sample engine's law) |
| `slfo_dest3`: not played | 1 | 3 | yes | open |
| `tuning_scale`: not played | 1 | 2 | yes | open |

## 4. Port to Rust

Not applicable: the engine is written in Rust.

## 5. Next

This engine's package is ninth of seventeen in the cross-engine order (605 stock Performances touched). In build
order:

1. Done in 0.6.2: the key-on delay's note length at the host's tempo (`key_on_delay_length`, 552 Performances). The
   sound LFO's tempo sync and tempo speed wait for their law.
2. The controller box destinations not played yet (`boxN_dst`, 407; 92, 95-97, 104, 107-109 and 111-115 done in 0.6.3; 52,
   53, 56-59, 88, 98, 110 and 117 in 0.6.4).
3. The operator level controller sensitivities (`opN.level_ctrl_sens`, 15 to 41 per operator).
4. The key controller sets (`opN.key_ctrl_setN`) and the sound LFO's other destinations.
