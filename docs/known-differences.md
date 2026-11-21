# Known differences from the reference

Where the engine plays differently from the reference, on purpose or for now, each to be
ruled on. "Existing profile" is the project's earlier FM plugin profile, whose measured laws this
engine reuses; its deliberate departures are not adopted silently: each is either replaced by the
reference's behaviour or listed here. The per-law accuracy is in `docs/coverage.md`.

## Open: for a ruling

| What | The reference | The engine (M5) | Why |
|---|---|---|---|
| Output clipping | not measured whether the part's output clips | no clip (the platform's mixer clips) | the existing profile's choice, unmeasured either way |
| Envelope resolution | per sample (as far as measured) | every 16 frames (0.33 ms at 48 kHz; a step is cut short where an event of the voice's sound falls), exact at the steps, gains ramped linearly between; a segment shorter than a step becomes one ramp; the hold in whole steps | the design's control rate; the fastest decay (time 0) moves 5.2 dB a step, the fastest attack about 18 dB |
| Key-on phase reset off | a free-running phase | the voice slot's phase runs on from its previous note | the reference's free-running phase is not modelled (as the existing profile) |
| Voice stealing | not measured | a 3 ms linear fade, then the voice ends; the platform's voice-budget order | the platform's convention |
| Polyphony | not measured per part | 128 voices an instance, shared by its sounds | provisional decision |
| Coarse 31 with fine 101-127 | not stable | the documented fixed ratio (6.66) | as the existing profile |
| Resonant form centred at or above Nyquist | a tone near Nyquist (its law not measured) | silent while the centre harmonic is at or above Nyquist (checked every control step) | the existing profile's choice; the reference's tone is not known well enough to play |
| Aliasing level of the forms | not band-limited; at the top key an odd form's aliases about 14 dB under its harmonics (as measured at the reference's rate) | not band-limited either, tables of the measured harmonics (up to the 168th) read with linear interpolation at the engine's rate: alias levels not compared | the alias pattern depends on the sample rate and the reference's table, neither matched |
| Silence floor | not measured | envelopes stop at about -180 dB | inaudible either way |
| Pitch that moves | per sample (the existing profile steps pitch per sample on the grid) | both pitch envelopes every 16 frames, the increments ramped linearly across each step; portamento and bend change at a step's start; not on the grid of 1/2048 octave (at most 0.59 cent from it) | the design's control rate: the ramp's frequency is within 1.2 cents of the per-sample law at the fastest operator pitch envelope times (0-2), within 0.65 cent from time 3; the fastest sweep (4800 cents in 12 ms) leaves the phase 0.0022 cycles off at key 93 (0.26 cycles without the ramp) |
| Portamento's two switches | glides only when the part's switch and the performance's (default off) are both on | one switch, the part's, at its documented default (on): a sound that sets nothing glides, full-time, at time 64 | the engine has no performance level: a converter writes portamento off unless both are on |
| Portamento's clock | steps every 440 samples at 44.1 kHz at the start of 64-sample blocks; its phase against a note not known | steps every 9.977 ms at the start of the 16-frame step it falls in, one clock for every sound, counted from the engine's start | finer than the reference's blocks; the phase is unmeasured either way |
| Pitch bend | the documented ranges, -48 to +24 semitones; not measured | the documented ranges; whole cents then whole steps; every operator, fixed-mode ones too | documented, not measured (the recorder cannot bend); whether fixed-mode operators follow is not known |
| Mono legato | measured with portamento: the envelopes go on | the amplitude envelopes, the common pitch envelope and the phases go on; the operator pitch envelopes start again; the new note's velocity sets its pitch velocity, not its levels; a key returned to draws a new random pitch | the existing profile's choices, unmeasured |
| Mono keys kept | not measured | the 16 newest keys still down are returned to; an older one's note ends when a seventeenth is kept | a fixed size |
| Key assign single | not measured | a key played again fades its sounding note as a mono retrigger does (13.3 ms) | as the existing profile, unmeasured |
| Voice LFO resolution | per sample (as far as measured) | every 16 frames: the increments and the gains ramp linearly across each step to the LFO's value at its end; a square or sample-and-hold edge becomes a 16-frame ramp; past about 150 Hz (extended speeds above about 320) the LFO is under-sampled and aliases | the design's control rate; an audio-rate LFO for the fastest speeds (design 2.3) is not built |
| Voice LFO key-on reset off | each note a phase of its own, no pattern found | a note takes the sound's free-running phase, counted from the engine's clock at the note's block | the reference's is not modelled (as the existing profile) |
| Voice LFO on fixed-mode operators | not measured | no pitch modulation on fixed-mode operators | as the existing profile, unmeasured |
| Voice LFO amplitude bend | measured at envelope level 99 on a lone carrier | the bend acts on the LFO's loss alone, on modulators as on carriers | as the existing profile, unmeasured |
| Level scaling | not measured | the six-operator model's documented curves in dB, a level raised to level 99's at most | documented, not measured (as the existing profile) |
| Ratio macro on fixed-mode operators | not measured | not moved | as the existing profile, unmeasured |
| Sound LFO resolution | per sample (as far as measured) | every 16 frames, its value at each step's end ramped across the step (pitch through the increments, level through the gains); a square, sample-and-hold or user-wave edge becomes a 16-frame ramp; a triangle's corner is cut by at most 29 cents for one step at the fastest speed and deepest depth (1.8 cents at depth 8); the stepped destinations (detune, form, feedback, the voice LFO's speed) change at a step's start | the design's control rate; the sound LFO's fastest speed (11.86 Hz) is 253 steps a cycle |
| Sound LFO tempo sync | 1/16 to 64 beats a cycle at the host's tempo | not played: the LFO runs at its speed (reported as not covered; the note length only with tempo sync on) | the cycle's length and its phase against the beat are not measured on any engine |
| Sound LFO random speed | its law neither measured nor documented as numbers | not played (reported as not covered) | no law |
| Sound LFO sample and hold 2 | its held values include many zeros (not resolved) | plays as sample and hold 1, a uniform value a cycle | as the existing profile |
| Sound LFO key-on reset off | free-running, its phase from the time since the performance loaded (not compared) | free-running from the engine's clock; a first key starts the envelope (delay, fades, hold) again | as the existing profile, unmeasured |
| Sound LFO envelope below about 100 ms | the fades and hold measured from 48 up | the fades and hold take the delay's times below that | as the existing profile, unmeasured |
| Sound LFO between its top and bottom | every destination measured with a square wave only | values between the top and bottom straight (in cents, dB, steps cut towards 0); the depth ratio rule on level, detune and form unmeasured; frequency and detune do not reach fixed-mode operators (unmeasured) | as the existing profile |
| Sound LFO level at depth 127 | silent at the bottom | 74.8 dB down at the bottom | the measured 0.589 dB a step, straight |
| Sound LFO pan, cutoff and resonance destinations | played; measured with a square wave at a few depths | played at the LFO's value at each step's end, ramped; the wide band-pass's width does not move with the resonance destination | as the existing profile (the width unmeasured) |
| Filter overload | the 24 dB low-pass, 24 dB high-pass and 12 dB band-pass overload at resonance 96-127 (8-51 dB of the existing profile's cases, harmonics added) | not modelled: the sections' states are held within +-8 (18 dB over full scale), so every output stays bounded; below that the sections are linear | the existing profile's choice (deliberate); the overload's law is not measured |
| Filter resolution | per sample (as far as measured; the reference's envelope segments arrive about 2 ms after the existing profile's) | the envelope, the LFOs' moves and the coefficients every 16 frames, F, damping and the one-pole's coefficient ramped linearly a frame; an instant segment becomes one step's ramp | the design's control rate (the existing profile: every 64 samples) |
| Filter at other rates | a 44.1 kHz filter: the section's shape near a sixth of its rate is part of the measured response | the same cutoffs in Hz at the engine's rate, so above about 3 kHz the section's shape follows the rate (at 48 kHz F at the top is 0.94, not 1); below 44.1 kHz the top is a sixth of the rate and the damping is held where the section turns unstable | a filter's shape near its rate cannot match at every rate; not compared at 48 kHz |
| Filter envelope segments | bend slightly (a 1200-cent decay a quarter of the way at 0.19 of its time, half at 0.51, three quarters at 0.78) | straight in cents | as the existing profile (up to about 90 cents off mid-segment) |
| Filter types 9, 18, 20 | no type of the model's documented list | played as thru, reported as not covered | no law |
| Low-pass 12 + band-pass 6 | its exact structure not resolved | a low-pass section plus a band-pass section with 1.13 times its Q (fitted, 13 of 21 cases within 1 dB) | as the existing profile |
| Dual types' second section above F = 1 | measured only at the 18 dB "s" type, resonance 64 | up to F = 1.75, damping limited only where it would turn unstable | as the existing profile (the dual band-pass at distance +127 is 8.1 dB off) |
| Filter envelope time velocity | positive measured from +8 to +63; negative instant from velocity 16 | the same; negative instant from velocity 2 (2-15 unmeasured); decay 2 and the release by the segment value's names | as the existing profile |
| Filter envelope time key follow below 0 | shorter for -64 too | the positive rule mirrored (longer) | as the existing profile, unmeasured |
| Voice LFO filter depth | measured at depth 25 only (its modulation shrank by about 15 % a second in that render, unexplained) | 68 cents a depth step at the peak, straight in depth, at each step's end | as the existing profile |
| Pan resolution | the existing profile moved the sound LFO's pan once per 64 samples | every 16 frames, the gains ramped a frame; a note's own position is fixed for its life | the design's control rate |
| Alternate pan's alternation | could not be measured (one note per performance load) | the side changes with every note of the sound (its first note at the set side), counted from the sound's install | as the existing profile, unmeasured |
| Pan of a mono legato note | not measured | the voice keeps its pan | unmeasured |
| Key-on delay | whole 64-sample blocks at 44.1 kHz | the same time, to the frame at every rate; at most 32 notes an instance waiting (more end at once); in mono and legato playing the note is handled when it starts | finer than the reference's blocks; mono and legato with a delay unmeasured |
| Key-on delay tempo sync | 1/16 to 8 beats at the host's tempo (measured on the sample engine: within 12.3 ms of the exact note length at 60 to 300 a minute, its own takes spreading up to 7.3 ms; the host's tempo, not the Performance's; 120 without one) | the exact note length at the host's tempo at the note-on, to the frame; a tempo change while a note waits does not move it; the transport's tempo read at each block's start (a change within a block waits for the next block) | law taken from the sample engine's measurement, not yet measured for this engine; a tempo change during the delay unmeasured |
| Sound LFO destinations 0-69 | the insertion effects' parameters | not played (reported as not covered) | the engine has no effects |
| Parameter changes inside a block (the plugin) | not measured | a block's values for a sound take effect together at the start of its stretch (at most 128 frames), not at their frames | one preparation a stretch instead of one an event (about 17,200 instructions each) |
| Controller box destinations | every destination of the part type | the operator level and frequency, the cutoff and the resonance, the voice LFO's speed, pitch depth and amplitude depth, the sound's pitch; the others reported as not covered (the voice LFO's filter depth, the envelopes, feedback, the forms, pan, part volume, the sound LFO's speed and depths, and the effects' parameters) | no law measured for them |
| Controller box limit and spans on this part type | measured: the operator level's and the cutoff's steps a ratio unit (ratios up to 64); the voice LFO's speed and depths up to a full box (100 steps, held within 0-99); the sound's pitch and an operator's frequency up to a quarter of a full box | the output's limit at 8192 (a full box: 96 level steps, 1024 cutoff values, 8192 cents of the sound's pitch, 3600 of an operator's at +7) and the resonance's span 128 taken from a sibling part's measurements | the limits on the pitches and the resonance not measured on this part type |
| Controller box curves | every preset curve and its parameters, user and library curves | the linear curve and eight measured preset curves; any other plays as linear (reported) | not measured |
| Voice LFO speed box in the extended range | moves the extended speed | not played (reported as not covered) | measured in the normal range only |
| Operator frequency boxes on fixed-mode operators | not measured | not moved | not measured |
| Operator frequency boxes by sensitivity | sens / 7 of 3600 cents a full box | the same, from whole cents of the +7 move; up to 1.3 cents from the reference at +3 and 0.8 at -7 (measured at 450 and 900 cents) | the reference's own rounding not found |
| Level controller sensitivity | leaves the level boxes as they are (measured at -7, 0, +3, +7); what it moves was not found | not played | no law |
| Level boxes and level scaling | a box moves the stored level; scaling's raise stops at level 99's | the operator is prepared at the top its boxes reach and lowered from there, so level scaling's raise stops where it would at that top level (less raise than the reference's while a box holds the level below its top) | the raise of a note's held levels is not re-computed while a box moves |
| Operators raised from silence by a box | sound when the box raises them | the same; an operator a box can raise is rendered (costed) even while the box holds it silent | the plan is fixed while notes sound |
| Expression (CC 11) | received; its law not measured | the output times (v / 127)^2, as the part volume's measured law (about 40 log10(v / 127) dB) | assumed |
| Foot controller 2 | assigned by default to a performance-wide knob, not a controller number | CC 4 is foot controller 2 (box source 4) | the host forwards CC 4, not the reference's assignment |
| Aftertouch | channel and key pressure (key pressure per key: not measured) | one value a sound, the latest of channel pressure, key pressure and the pressure expression | as the sibling engines; per-voice boxes would cost a box evaluation a voice |
| Sostenuto pedal | received | holds the notes whose keys are down when it goes down (not those the sustain pedal holds then); later notes not held | the reference's rule not measured |
| Soft pedal (CC 67), portamento switch (CC 65) | the soft pedal not received; CC 65 switches portamento | both ignored | CC 65 waits for a ruling (the sound's switch is a stored parameter) |
| Box sources the host cannot send | the foot switch, the stepped modulation lanes, the envelope followers | not played (reported) | the host sends neither |
| Knob, ribbon and switch changes (the plugin) | at their time | sound parameters: at the start of the stretch after their event (a preparation, about 17,200 instructions) | one route for every parameter |
| Pedal and bend after a sound change (the plugin) | per part | sent to the sound the channel selects now; notes left sounding on the previous sound keep its last values | the channel addresses one sound at a time |
| Note-on velocity 0 (the plugin) | MIDI velocity 0 is a note off (as here) | a CLAP note on with velocity 0 plays at velocity 1 | a CLAP note on is always a note |
| A parameter changed while a sound plays | not measured | from the next control step for what moves, the next note for what a note keeps; a change of the operators heard, their forms, the feedback path, mono or key assign waits until the sound is silent | a voice's state stays valid |

## Adopted from the reference where the existing profile departs from it

| What | Existing profile | The engine |
|---|---|---|
| Fixed-mode frequencies | no ceiling | held at 22.04 kHz, as the reference plays coarse and fine values past it |
| Note shift range | widened to +-127 | the reference's +-48 |
| Pitch bend ranges | widened to +-127 | the reference's -48 to +24 |
| Portamento's default | off (one switch for the two) | the documented default, on (see above) |
| Part velocity fractions | rounded to whole velocities | played between the neighbouring velocities' levels, as the reference |
| Operator forms | band-limited (every harmonic below Nyquist up to the 16th, nothing above), versions chosen at note-on | not band-limited, as the reference: one table per form and spread at every pitch |

## Inherited inaccuracies (the measured laws' own misses)

- Detune: 5 of 20 measured cases one grid step (1/1024 octave) off, up to 1.18 cents.
- Level velocity sensitivity: 56 of 64 cases within the gate; worst 1.5 dB at velocities 2-3.
- Feedback: three measured cases off by 2.5, 3.8 and 20 dB.
- Envelope times: the reference's fastest segments read up to 15 % slower than the tables; time key
  follow 90 of 151 cases within 3 %.
- The carrier envelope-level table is checked only against the renders it was fitted to.
- Random pitch: the reference reaches one step further down than up at depths 32-127.
- Portamento: 22 of 29 and 27 of 28 measured times within 2 % (fast times read slower).
- Part detune: 37 of 49 measured values within 1 cent.
- Retrigger fade: about 1 ms ahead of the reference's in its first 10 ms.
- Operator pitch envelope times 90-99 unmeasured; fixed-mode pitch velocity sensitivities 4 and 6
  halfway between their neighbours.
- Filter: 676 of 697 measured cases within 1 dB; the wide band-pass 14 of 16 (1.1-1.3 dB at cutoff
  256); the dual band-pass at distance +127 8.1 dB off; low-pass 12 + band-pass 6 13 of 21. Cutoff
  key follow up to 16 cents off at fractional octaves. Filter envelope decay 42 2.0 ms off; holds
  above 96 unmeasured; depth velocity curves 0, 1, 3, 4 measured at two velocities.
- Pan: scaling pan +63 at key 30 (the reference's far side at -64.7 dB, the law's -45.6); random
  pan's narrower spread at depth 127 and its left lean.
