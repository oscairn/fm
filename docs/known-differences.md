# Known differences from the reference

Where the engine plays differently from the reference, on purpose or for now, each to be
ruled on. "Existing profile" is the project's earlier FM plugin profile, whose measured laws this
engine reuses; its deliberate departures are not adopted silently: each is either replaced by the
reference's behaviour or listed here. The per-law accuracy is in `docs/coverage.md`.

## Open: for a ruling

| What | The reference | The engine (M3) | Why |
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
