# Known differences from the reference

Where the engine plays differently from the reference, on purpose or for now, each to be
ruled on. "Existing profile" is the project's earlier FM plugin profile, whose measured laws this
engine reuses; its deliberate departures are not adopted silently: each is either replaced by the
reference's behaviour or listed here. The per-law accuracy is in `docs/coverage.md`.

## Open: for a ruling

| What | The reference | The engine (M4) | Why |
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
| Sound LFO tempo sync | 1/16 to 64 beats a cycle at the host's tempo | not played: the LFO runs at its speed (reported as not covered) | the core has no tempo input |
| Sound LFO random speed | its law neither measured nor documented as numbers | not played (reported as not covered) | no law |
| Sound LFO sample and hold 2 | its held values include many zeros (not resolved) | plays as sample and hold 1, a uniform value a cycle | as the existing profile |
| Sound LFO key-on reset off | free-running, its phase from the time since the performance loaded (not compared) | free-running from the engine's clock; a first key starts the envelope (delay, fades, hold) again | as the existing profile, unmeasured |
| Sound LFO envelope below about 100 ms | the fades and hold measured from 48 up | the fades and hold take the delay's times below that | as the existing profile, unmeasured |
| Sound LFO between its top and bottom | every destination measured with a square wave only | values between the top and bottom straight (in cents, dB, steps cut towards 0); the depth ratio rule on level, detune and form unmeasured; frequency and detune do not reach fixed-mode operators (unmeasured) | as the existing profile |
| Sound LFO level at depth 127 | silent at the bottom | 74.8 dB down at the bottom | the measured 0.589 dB a step, straight |
| Sound LFO pan, cutoff and resonance destinations | played | parsed and reported as not covered | M5 (pan and the filter) |
| Sound LFO destinations 0-69 | the insertion effects' parameters | not played (reported as not covered) | the engine has no effects |
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
