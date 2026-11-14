# Known differences from the reference

Where the engine plays differently from the reference, on purpose or for now, each to be
ruled on. "Existing profile" is the project's earlier FM plugin profile, whose measured laws this
engine reuses; its deliberate departures are not adopted silently: each is either replaced by the
reference's behaviour or listed here. The per-law accuracy is in `docs/coverage.md`.

## Open: for a ruling

| What | The reference | The engine (M2) | Why |
|---|---|---|---|
| Output clipping | not measured whether the part's output clips | no clip (the platform's mixer clips) | the existing profile's choice, unmeasured either way |
| Envelope resolution | per sample (as far as measured) | every 16 frames (0.33 ms at 48 kHz), exact at the steps, gains ramped linearly between; a segment shorter than a step becomes one ramp; the hold in whole steps | the design's control rate; the fastest decay (time 0) moves 5.2 dB a step, the fastest attack about 18 dB |
| Key-on phase reset off | a free-running phase | the voice slot's phase runs on from its previous note | the reference's free-running phase is not modelled (as the existing profile) |
| Note start | frame-exact | at the start of the block the note-on precedes (up to 2.7 ms late at 128 frames, 48 kHz) | splitting a block at events is not built yet (the shell's milestone) |
| Voice stealing | not measured | a 3 ms linear fade, then the voice ends; the platform's voice-budget order | the platform's convention |
| Polyphony | not measured per part | 128 voices an instance, shared by its sounds | provisional decision |
| Coarse 31 with fine 101-127 | not stable | the documented fixed ratio (6.66) | as the existing profile |
| Resonant form centred at or above Nyquist | a tone near Nyquist (its law not measured) | silent while the centre harmonic is at or above Nyquist (checked every control step) | the existing profile's choice; the reference's tone is not known well enough to play |
| Aliasing level of the forms | not band-limited; at the top key an odd form's aliases about 14 dB under its harmonics (as measured at the reference's rate) | not band-limited either, tables of the measured harmonics (up to the 168th) read with linear interpolation at the engine's rate: alias levels not compared | the alias pattern depends on the sample rate and the reference's table, neither matched |
| Silence floor | not measured | envelopes stop at about -180 dB | inaudible either way |

## Adopted from the reference where the existing profile departs from it

| What | Existing profile | The engine |
|---|---|---|
| Fixed-mode frequencies | no ceiling | held at 22.04 kHz, as the reference plays coarse and fine values past it |
| Note shift range | widened to +-127 | the reference's +-48 |
| Part velocity fractions | rounded to whole velocities | played between the neighbouring velocities' levels, as the reference |
| Operator forms | band-limited (every harmonic below Nyquist up to the 16th, nothing above), versions chosen at note-on | not band-limited, as the reference: one table per form and spread at every pitch |

## Inherited inaccuracies (the measured laws' own misses)

- Detune: 5 of 20 measured cases one grid step (1/1024 octave) off, up to 1.18 cents.
- Level velocity sensitivity: 56 of 64 cases within the gate; worst 1.5 dB at velocities 2-3.
- Feedback: three measured cases off by 2.5, 3.8 and 20 dB.
- Envelope times: the reference's fastest segments read up to 15 % slower than the tables; time key
  follow 90 of 151 cases within 3 %.
- The carrier envelope-level table is checked only against the renders it was fitted to.
