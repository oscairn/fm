# Design

The engine reproduces the reference's eight-operator FM part engine, every parameter, as a
first-party Rust implementation from laws and measurements. M0 held the design, the scaffold and a
measured voice-kernel spike (`crates/fm-core/src/spike.rs`); M1 (section 3.1) is the voice: the
sound format, the coverage report, plans, the routings, the engine with sine operators and the
measured level, envelope, feedback and pitch laws. Figures marked "measured" were seen in this
repository's tools; everything else is a plan or an estimate and says so.

Contents: 1 the model and the gap list; 2 architecture; 3 the spike; 4 audio-thread safety;
5 toolchain and targets; 6 tests and gates; 7 milestones; 8 open questions.

## 1. The model and the gap list

What one part of the reference's FM engine holds, in neutral names, with every parameter and its
range, and for each the state of the project's existing implementation: the eight-operator profile
of its six-operator FM plugin (the "existing profile" below). Its laws were measured from the
reference's audio output (renders at 44.1 kHz) and its accuracy is what its own gates recorded. That
plugin's code is not used here; its laws, test vectors and measured numbers are acceptance data.

Columns: **Ex** = the existing profile implements it: yes, part (partly or with a known miss), no.
"Unmeasured" means implemented without a check against the reference.

### 1.1 Part common (41 parameters)

| Parameter | Range | Ex | Existing law; accuracy against the reference |
|---|---|---|---|
| Random pan depth | 0–127 | part | uniform over 64·(d/127)² positions; the reference's left bias and narrower spread at 127 not modelled |
| Alternate pan depth | L64–C–R63 | yes | measured far-side pan table, near side at full level; 5 of 5 sides |
| Scaling pan depth | −64–+63 | yes | d/32 positions a key outside keys 60–64; 75 of 76 within 0.5 dB |
| Key-on delay time | 0–127 | yes | whole 64-sample blocks at 44.1 kHz, 0–8.08 s; 128 of 128, worst 0.29 ms |
| Key-on delay tempo sync | off, on | yes | host tempo; checked by its own test only |
| Key-on delay note length | 17 values, sixteenth to 8 quarters | yes | as above |
| Pitch velocity sensitivity | −64–+63 | yes | measured table on a 1/2048-octave grid; worst 1.12 cents |
| Random pitch depth | 0–127 | part | uniform steps of 18.75 cents, reach about d/4 steps; spread within a step, sequence not reproducible |
| Pitch key follow | −200 %–+200 % | yes | whole-percent steps, straight in cents; 157 of 157, worst 0.97 cents |
| Pitch key follow centre key | C-2–G8 | yes | as above |
| Pitch envelope levels: initial, attack, decay 1, decay 2, release | −50–+50 each | yes | 37.5-cent steps (wider beyond ±32), times the depth; 101 of 101, worst 0.31 cents |
| Pitch envelope times: attack, decay 1, decay 2, release | 0–99 each | yes | constant rate in cents/ms, multiples of 0.0562; worst 0.33 % |
| Pitch envelope depth velocity sensitivity | 0–7 | yes | removes measured steps; met (worst 0.22 cents) |
| Pitch envelope depth | 8, 2, 1, 0.5 octaves | yes | 1, 1/4, 1/8, 1/16 of the widest; met |
| Pitch envelope time key follow | 0–7 | yes | rate × (1 + s(n − 3)/33); met |
| Voice LFO wave | triangle, saw down, saw up, square, sine, sample and hold | yes | measured shapes |
| Voice LFO speed | 0–99 | yes | measured rate table; 100 of 100 within 1 %; speed 0 unmeasured |
| Voice LFO phase | 0°, 90°, 180°, 270° | yes | start phases 6–14 cents off (square 61, sine 19) |
| Voice LFO delay | 0–99 | yes | a hold, then a fade-in of equal length; delay 99 2.5 % early |
| Voice LFO key-on reset | off, on | yes | off: a free-running phase (chosen; the reference's not modelled) |
| Voice LFO pitch depth | 0–99 | yes | twice the six-operator depth steps; 30 of 31 (depth 80 +1.3 cents) |
| Voice LFO amplitude depth | 0–99 | yes | straight in dB; within 0.16 dB, checked only against the renders it was fitted to |
| Voice LFO filter depth | 0–99 | yes | 68 cents a step at the peak; measured at depth 25 only |
| Algorithm (routing) | 1–88 | yes | a compiled graph, ascending order; all 88 checked at feedback 5 |
| Feedback level | 0–7 | yes | a carrier feeds back 0.18 of a modulator's amount; carrier within 0.4 dB, modulator harmonics 2–6 within 0.09 dB, 3 cases off by 2.5, 3.8 and 20 dB |
| LFO speed range | normal, extended | yes | |
| LFO extended speed | 0–415 | yes | measured rate table, every fifth value checked |
| Macro: modulation depth | −99–+99 | yes | added to modulator levels, divided by (1 + feedback) on the operator that feeds back; 12 of 12 without feedback, 0.02–1.25 dB with |
| Macro: ratio shift | −99–+99 | yes | table at 9 ratios × 79 values, linear between; modulators only; up to 5.7 cents between points |
| Macros: attack, decay, sustain, release | −99–+99 each | yes | added to modulator envelope times and the decay-2 level; 44 of 48 simple probes, 4 of 24 on the wave probe |
| Macro: texture | −99–+99 | part | a path through wave "places"; extremes off by 16–25 dB, −99 by 22.6 dB; the wrap past ±99 unmeasured |

### 1.2 Filter and filter envelope (35 parameters)

| Parameter | Range | Ex | Existing law; accuracy |
|---|---|---|---|
| Filter type | 19: low-pass 24 dB (two kinds), 18 dB (two kinds), low-pass 12 + high-pass 12, low-pass 6 + high-pass 12, high-pass 24 and 12, band-pass 12, wide and 6, band-stop 12 and 6, four dual types (low, high, band, stop), low-pass 12 + band-pass 6, through | part | a biquad structure per type; 676 of 697 within 1 dB; low-pass 12 + band-pass 6 approximate; overload at high resonance not modelled (deliberate) |
| Cutoff, high-pass cutoff | 0–1023 each | yes | measured Hz table, capped at 7.35 kHz |
| Resonance / width | 0–127 | part | measured Q; three types at resonance 96–127 off by 8–51 dB (the reference overloads) |
| Cutoff velocity sensitivity, resonance velocity sensitivity | −64–+63 each | yes | measured velocity curves |
| Distance (dual types) | −128–+127 | yes | second section F·2^(d/16); above F = 1 unmeasured |
| Filter gain | 0–255 | yes | 0.375 dB a step |
| Envelope times: hold, attack, decay 1, decay 2, release | 0–127 each | yes | one duration table; 91 of 92 within 2 %; 0–20 instant |
| Envelope levels: hold, attack, decay 1, decay 2, release | −128–+127 (±9600 cents) each | yes | 75 cents a step; within 15 cents; segments straight in cents (the reference bends up to about 90 cents) |
| Envelope depth | −64–+63 | yes | a measured share |
| Envelope time velocity segment | attack, attack + decay, decay, attack + release, all | yes | partly chosen where unmeasured |
| Envelope time velocity sensitivity | −64–+63 | yes | positive shifts the time value, negative makes the segment instant |
| Envelope depth velocity sensitivity | −64–+63 | yes | measured per curve |
| Envelope depth velocity curve | 0–4 | yes | curves 0, 1, 3 and 4 checked at two velocities only |
| Envelope time key follow, centre key | −64–+63; C-2–G8 | yes | the negative side mirrors the positive (unmeasured) |
| Cutoff key follow, high-pass key follow | −200 %–+200 % each | yes | 3.125 % steps; up to 16 cents at fractional octaves |
| Cutoff scaling break points 1–4 | keys, ascending | yes | 37.5 cents a step, linear by key |
| Cutoff scaling offsets 1–4 | −128–+127 each | yes | as above |

### 1.3 Per operator, eight operators (8 × 39 switches, 8 × 38 parameters, 8 × 2 read-outs)

Controller switches (39 a operator):

| Parameter | Range | Ex | Existing law; accuracy |
|---|---|---|---|
| Controller assignment set 1–32 switches | off, on each | no | not in the parameter list |
| Operator key-controller set 1–4 switches | off, on each | no | |
| LFO destination box 1–3 switches | off, on each | yes | gates the sound LFO's destinations per operator |

Operator (38 a operator, and two read-outs):

| Parameter | Range | Ex | Existing law; accuracy |
|---|---|---|---|
| Phase reset at key-on | off, on | yes | off: free-running phase |
| Frequency mode | ratio, fixed | yes | |
| Coarse, fine | 0–31; 0–127 | yes | 1/2048-octave grid (key and ratio rounded to 1/1024 octave); fixed mode on a 1/128-octave grid; 1 cent met; coarse 31 with fine above 100 plays the documented ratio (the reference is not stable there) |
| Detune | −15–+15 | yes | measured table; 5 of 20 one grid step off, up to 1.18 cents |
| Fixed-mode key follow | 0–99 | yes | measured; worst 1.77 cents |
| Pitch velocity sensitivity | −7–+7 | yes | fixed-frequency operators only |
| Wave (spectral form) | sine and six forms: two all-harmonic, two odd-harmonic, two resonant | yes | single-cycle tables; the four harmonic forms at every spread within 0.06 dB (first 16 harmonics) |
| Spread (skirt) | 0–7 | part | per-spread tables; resonant forms at spread 7 meet 10 of 24 (3 to over 100 dB off); one resonant form at spreads 4–5 off by 2–7 dB |
| Formant (resonance) | 0–99 | part | a window times a sine at harmonic formant + 1; spreads 0–3 48 of 48 (worst 0.58 dB) |
| Pitch envelope initial and attack levels | −50–+50 each | yes | 101 of 101 |
| Pitch envelope attack and decay times | 0–99 each | yes | constant cents rate; within 2 %, attack 90–99 unmeasured |
| Amplitude envelope levels: attack, decay 1, decay 2, release (hold) | 0–99 each | yes | separate carrier and modulator curves; carrier table checked against its own renders only |
| Amplitude envelope times: attack, decay 1, decay 2, release, hold | 0–99 each | yes | fitted time table; rising segments exponential in dB towards +1.52 dB; within 2 %; the reference's fast segments read up to 15 % slower |
| Amplitude envelope time key follow | 0–7 | yes | 8 steps a rate doubling; 90 of 151 within 3 % (the rest the fast-segment reading) |
| Operator level | 0–99 | yes | modulator 0.75 dB a step, carrier curve, an output level per routing; modulation index within 0.01 dB at 32 levels |
| Level scaling break point, low and high depths, low and high curves | key A-1–C8; 0–99; 4 curves | part | the six-operator curves, unmeasured |
| Level velocity sensitivity | −7–+7 | yes | measured, fractional velocity; 56 of 64 (worst 1.5 dB at velocities 2–3) |
| Voice LFO pitch and amplitude depth offsets | 0–7 each | yes | measured tables |
| Pitch and level controller sensitivities | −7–+7 each | no | |
| Sound LFO destination depth ratios 1–3 | 0–127 each | yes | depth × ratio/128 (127 counts as 128); within 0.4 cents |
| Ratio and frequency read-outs | two words | no (not state) | a display of the result, not a parameter |

### 1.4 Part parameters that act on the engine (55; 56 ids in the sound format, which adds the sound LFO's random speed)

| Parameter | Range | Ex | Existing law; accuracy |
|---|---|---|---|
| Mono / poly, key assign (single, multi) | | yes | last-note priority; single uses a 13.3 ms fade, gain (t_left)²; 18 of 18 and 12 of 13, the fade 1 ms ahead |
| Portamento switch, mode (fingered, full-time), time mode (two rates, two times) | | yes | straight in semitones |
| Portamento time | 0–127 | part | rate table; 22 of 29 and 27 of 28 within 2 % (fast times slower) |
| Pitch bend up, down | −48–+24 each (the reference) | part | widened to ±127 by decision (no pitch ceiling); unmeasured |
| Detune | −12.8–+12.7 Hz | yes | cents from a table; 49 of 49 within 1 cent |
| Note shift | −48–+48 | yes | whole semitones before the grid; widened to ±127 |
| Micro tuning scale, root | built-in scales, user | no | fixed to equal temperament |
| Velocity depth, offset | 0–127 each | yes | v·d/64 + 2(o − 64) |
| Sound LFO wave | 13 incl. a user wave | part | 95th percentile 0.1–10.5 cents; one sample-and-hold wave uses the other's law |
| Sound LFO: phase, speed, tempo sync, tempo speed, delay, fade in, hold, fade out, key-on reset (off, each, first), destinations 1–3, depths 1–3, user cycle, slope, 16 steps | | yes | speed table (about ×2 per 8 values, within 0.23 % against its own renders); delay 109 of 111; depths within 0.4 cents |
| Part offsets: amplitude attack, sustain | −64–+63 each | part | attack up to 5.6 dB off, sustain 1.2–2.3 dB |
| Part offsets: amplitude decay, release, filter envelope depth, cutoff, resonance | −64–+63 each | yes | 18 of 18 within 0.3 dB; release −8 off by 1.0 dB |

### 1.5 The gap list in numbers

Counted by hand over 1.1–1.4, one per value slot (each operator's parameters eight times):

| | slots | existing: yes | part | no |
|---|---|---|---|---|
| Part common | 41 | 38 | 3 | 0 |
| Filter and filter envelope | 35 | 33 | 2 | 0 |
| Operator switches (8 × 39) | 312 | 24 | 0 | 288 |
| Operator (8 × 38) | 304 | 232 | 56 | 16 (controller sensitivities) |
| Read-outs (8 × 2, not state) | 16 | 0 | 0 | 16 |
| Part parameters | 55 | 47 | 6 | 2 (micro tuning) |
| **All** | **763** | **374** | **67** | **322** |

So "everything" beyond the existing profile is: the 288 controller-set switches and 16 controller
sensitivities (they decide which operators a controller assignment reaches: section 8 asks how they
map onto the platform's control tree), micro tuning, and the 67 partial items (resonant forms at
wide spreads, the texture extremes, the filter overload, level scaling, the fastest envelope
segments, fast portamento, part attack and sustain offsets, random pan and pitch distributions,
bend ranges unmeasured). Things the existing profile does differently by decision and this engine
must decide again: band-limited waves (the reference aliases; a formant above Nyquist is silent
there instead of a tone near Nyquist), no filter overload, no output clip (the platform's mixer
clips), the 10.3 Hz first-order output high-pass (the reference's, adopted), filter and filter
envelope once per 64 samples, and key-on delay in 64-sample blocks at every rate.

## 2. Architecture

### 2.1 Crates

| Crate | What | Constraints |
|---|---|---|
| `crates/fm-core` | the DSP core: vector type, tables, laws, plans, voices, pools, sounds | `#![no_std]`, no `alloc`, no OS, threads, files, locks, logging or formatting; the caller owns every object (fixed size, known at compile time) |
| `crates/fm-clap` | the CLAP shell, and in its own module a thin safe layer over the platform's extension bindings that the platform's SDK can adopt | std; allocation only off the audio thread |
| `tools/fmbench` | renders N voices: callgrind cost, time on any machine, the device timing binary | std |
| `tools/nopanic` | a `no_std` static library over the block path for the link-time panic check | |

The sound format, the parameter table and the laws (raw value → physical value, once per sound)
live in the core so that the shell, the converter and the tests share one definition.

### 2.2 Voices: four a lane, across voices

A lane is four voices of one sound, one voice a SIMD lane (`F4`/`U4`, section 5). Every per-voice
quantity is a `[f32; 4]` or `[u32; 4]` in a plain struct (structure of arrays): eight operators ×
{phase, increment, gain, envelope state}, the feedback history, the gate mask, an active-voice
mask. Each sound has its own pool of lanes, so a lane never mixes sounds and the routing, the
operator waves and the plan are uniform across its four lanes.

Why across voices and not across operators: within one voice an operator's input is the output of
lower operators *in the same sample*, and feedback needs the previous sample. In a chain of eight
(one routing of the 88) nothing is parallel within a sample; in the all-carrier routing everything
is. Vectorising the eight operators of one voice would need a schedule per routing, idle lanes in
deep routings, and shuffles between dependent operators. Four voices in lanes have no
dependencies at all, and the same code serves all 88 routings. The cost is a partly filled lane:
it costs as a full one (the shell packs a sound's voices into as few lanes as it can).

Within a control step (16 frames) the operators of a lane run in ascending order, which is valid
for every routing (every modulation edge runs from a lower operator to a higher). Measured
(section 3), operator-major is the layout: each operator renders the whole step into a 16-frame
buffer (`[[f32; 4]; 16]`, 256 bytes) after its modulation input has been summed from the buffers
of its sources; self-feedback is sample by sample inside that one operator. The two routings
whose feedback runs through other operators (5 → 3 around 3 > 4 > 5; 4 → 3 around 3 > 4) render
only the loop's operators interleaved sample by sample, the rest operator-major. Sample-major
everywhere was measured at 1.6 times the cost and rejected.

### 2.3 Control rate and audio rate

Audio rate: phases (32-bit integers, wrapping, exact on every back end), modulation sums,
feedback, wave reads, operator gains (a linear ramp a frame), filter states, the output.

Control rate, every 16 frames (or fewer where an event splits a block, so a note starts on its
frame), each with a linear ramp to the next step where it reaches the audio:

| Modulator | Rate and why it is safe |
|---|---|
| Amplitude envelopes (8 a voice), level scaling, velocity, voice and sound LFO amplitude, macros on levels | a target gain a step, ramped linearly a frame (implemented in the spike). 16 frames is 0.33 ms at 48 kHz: the sibling engine measured its reference with no artefact at 16/32/64-sample periods above −55 dB and its own 16-frame ramps at −68 dB on the fastest sweep. Exception to check: segments faster than a step (instant times) become one ramp; the reference's own fastest segments read slower than the existing tables, so a per-segment check is a gate of M1 |
| Operator and common pitch envelopes, key follow, bend, portamento, voice and sound LFO pitch, random pitch, detune, ratio shift | a phase increment a step. The phase is continuous, so a pitch step is a frequency step, not a waveform discontinuity; a 16-frame staircase of a 6 Hz vibrato is 500 steps a cycle. The existing profile steps operator pitch envelopes per sample; to measure at M3 whether the step is audible on an operator modulating at a high index (a pitch step there moves many sidebands) and ramp the increment if it is |
| Sound and voice LFOs | evaluated a step. At 48 kHz that is 3,000 values a second: up to about 150 Hz that is 20 a cycle. The extended speed range reaches higher; its top rate is to be read from the measured rate table at M4, and above 150 Hz the LFO moves to audio rate for that sound (a plan choice) |
| Filter coefficients and the filter envelope | a step, coefficients ramped (the existing profile: once per 64 samples, its instant envelope segments then take 1.45 ms; the reference's arrive about 2 ms after them) |
| Pan (random, alternate, scaling, sound LFO pan) | per note, and the LFO's a step, ramped |
| Feedback level, operator wave choice (band-limit version) | per step, from the increment (the existing profile chooses the wave version at note-on only, a shortcut) |
| Key-on delay | frame-exact |

### 2.4 Tables

The spike's tables are single cycles of 1024 points plus a guard point (`f32`, 4,100 bytes each),
read with linear interpolation from the top 10 bits of the phase and a 16-bit fraction, computed at
compile time (`const fn`), so they are read-only data that every instance and process shares through
the page cache, and nothing is built at run time.

The operator forms (M2, `crates/fm-core/src/forms.rs`), decided and measured:

- **Not band-limited, as the reference.** The existing profile's renders show the reference's
  harmonic forms with the same spectrum at every note (at the top key an odd form's aliases sit about
  14 dB under its harmonics) and a non-harmonic tone near Nyquist where a resonant form's centre is
  above it. The engine plays one table per form and spread at every pitch (the existing profile's 16
  band-limit versions are not needed), so aliasing is the reference's kind (its level not compared);
  a resonant form is silent while its centre is at or above Nyquist (checked every control step from
  the increment; the reference's tone is not known well enough: `docs/known-differences.md`).
- **Tables**: forms 1-2 one table each spread; forms 3-4 a table of E, the form being E(2t) sin(t),
  played E(2t + m) sin(t + m) (the measured law of a modulated odd form: two reads); forms 5-6 a
  window table each spread times a sine at the centre harmonic (formant + 1), window(t + m) sin(centre
  t + m) (two reads and an integer multiply); no table per formant value. 48 tables of 1025 points,
  computed by the build script (`forms_build.rs`; as constants they took 30 s a build) from the
  measured amplitudes and phases (`forms_data.rs`: 2,957 components, 70 KB of source).
- **Q14 (`i16`) rather than `f32`**: the forms peak near 1.01; Q14 puts every form and spread within
  0.005 dB of the measured levels of its first 16 harmonics (0.02 dB for those 40-60 dB down) and
  0.0015 rad of the phases (a test), and halves the bytes: 98,400 bytes of shared read-only data
  (196,800 as `f32`). Eight operators on eight different tables touch 16.4 KB of form tables and the
  4.1 KB sine table (37 KB with `f32`, more than the first-level cache). A read costs four
  instructions more than an `f32` read (shift and convert, twice), exact on every back end.

### 2.5 Plans

A sound's plan is data computed once per sound off the audio thread: the operator groups (single or
interleaved), each operator's wave kind (sine, one table, window × sine) and table references, the
carrier mask, the feedback descriptor, the filter type, and which control stages are present (an
LFO with zero depth is not in the loop). The block path matches on the wave kind once per operator
per step (outside the sample loop); the kernels are generic over the vector type and monomorphised.
A plan also gives the voice's static cost (2.8).

At M1 (`crates/fm-core/src/plan.rs`) a plan holds the operators that can be heard (audible: level
and envelope levels not all silent; and a carrier, or modulating an operator that is heard),
their modulators among those, the feedback path's kind (none, an operator into itself, a loop
through several) and a cost estimate; the block path renders only the operators in it, so a sound
with one carrier costs about a sixth of one with eight (3,131 against 16,214 instructions a
voice-block, measured). Since M2 each operator's form (`forms::Wave`) is in the plan, and a plan
with sines only runs kernels compiled without the forms' branches (so it costs what it cost at M1).

### 2.6 Sound format and state

A sound is the model's raw parameter values, all of them, in raw units with stable numeric ids
(section 1 is the list): the format does not change when a feature is added; what the core does
not reproduce yet is reported by a `not_covered()` function tied to a coverage table, one line per
parameter. The state is append-only and versioned: a version number, then the values as id/value
pairs; a new parameter takes a new id with a default that plays as before; nothing is renumbered.
Laws turn raw values into physical ones (Hz, seconds, dB, cents) once per sound when it is
prepared, never in the block path; what depends on the key and velocity (frequency, levels,
envelope rates with time key follow) once per note. The format is in `docs/sound-format.md`
(binary with a CRC, and a text form for tests; 748 ids).

### 2.7 Sound contexts and sharing

As in the sibling engines: up to 8 sounds an instance (`oscairn.sound-snapshots/draft-4`, table
`oscairn_plugin_sound_snapshots`), each with its own channel, its own pool and its own random
source, rendered on its own and split only at its own channel's events, so a part plays sample for
sample as it would alone. Nine stereo output ports with stable ids (0 main, 1–8 sound bus n); a
sound's output bus parameter (0–8, the output-bus role) picks its port; with
`clap.audio-ports-activation` a port plays only once activated, and a sound whose port is not
active plays on main. Sounds are prepared on the main thread into a buffer per context and
installed by the audio thread by a copy, with no lock.

### 2.8 Voice budget, shedding and the end rule

- Each note is one voice. The pool is fixed (lanes × 4 per sound), sized at instance creation.
- Static cost: every plan reports its voice cost (instructions a voice-block from the measured
  cost table by wave kind and group, section 3) through `oscairn.voice-budget` before the first
  note.
- Overload: when a budget or the pool would be exceeded, whole notes are shed in the same block, in
  the platform's order (released before held, lower priority, quieter, older; never a held mono key
  or a sound's reserve), each with a short fade (3 ms, as the sibling engine); a note is never cut
  without its fade and never renders past the budget. No glitch is ever traded for a note.
- End rule: a released voice ends once its output gain bound is below −96 dBFS: the sum over its
  carriers of envelope × level terms, each term at its maximum over what can still move it (LFO
  amplitude at its peak, the macro and velocity terms as set), with every wave's peak at most 1 (a
  table test). The spike implements the rule on the carriers' target gains; its slot is free in
  the same control step.
- As built at M1 (`crates/fm-core/src/engine.rs`): 40 lanes an instance (the 128-voice ceiling's 32
  and a partly filled lane per sound, so a note under the ceiling always finds a slot); a lane takes
  the sound of its first note. The bound is, per carrier, the gain at the larger of its envelope
  level and its segment's target (in the release the level moves only towards the target), summed;
  only released voices (key up and not held by the pedal) end by it; a held note never does. A
  shed voice fades linearly over 3 ms (to zero on the ninth 16-frame step at 48 kHz) and then ends.
  The order is the platform's (`voice-budget.h`): never a held voice of a sound at or under its
  reserve (unless for that sound's own note); released before held; the new note's own sound
  before others; lower priority before higher; the quietest (the end rule's bound), then the
  oldest. No mono yet (M3), so its rule has nothing to protect. A note that gets no voice (every
  candidate protected, or no free slot while fades are under way) ends at once; every note-on is
  answered by exactly one end (the fuzz test). A sound is installed only while none of its voices
  sounds (`choke` fades them first).

### 2.9 Cost and memory reporting

- Cost: callgrind on aarch64 (`tools/cost/cg.sh`), instructions a voice-block per case, gated in CI
  (section 6); the device timing binary is `fmbench` (time a voice-block) built for the device.
- Memory: the instance is fixed-size structs (`size_of` is the figure, reported through
  `oscairn.memory` as instance bytes, plus per-context bytes); tables are shared read-only data,
  reported once as shared bytes. Bytes touched per voice-block are stated by `fmbench footprint`
  and checked by a unit test (section 3).

### 2.10 The shell

Not built this round: `crates/fm-clap` holds the plugin id and name, each defined once. Plan:
CLAP's own types from `clap-sys` 0.5.0; the platform's extensions from the platform's bindings
(`oscairn-clap-sys`, pre-generated, no libclang at the consumer), pinned to one commit. Those
bindings carry their own copies of the roughly 30 CLAP types the extensions use, layout-identical to
`clap-sys`'s. The preference is that the bindings use `clap-sys`'s types directly, so the shell has
one set of CLAP types and no casts at the extension boundary (fewer `unsafe` lines); until then the
shell casts at that boundary, in the safe layer only. The C++ helper headers' roles (sound contexts,
voice budget, preset discovery) are written in Rust in the safe layer. Extensions: audio ports (nine,
with activation), note ports, params (every model parameter, controller-box destinations flagged
modulatable with the modulation amount kept out of the state), state, voice info, the platform's
sound snapshots, voice budget, memory and UI hints (a role for every parameter).

## 3. The spike

What it is: eight operators, a placeholder non-sine form, an envelope per operator at control
rate with linear gain ramps, one feedback path, four voices a lane, 128-frame blocks, the end rule;
two layouts; generic over the vector type. Not the reference's laws: the table form is a
band-limited 16-harmonic wave and the window form a raised cosine times a sine at harmonic 7, with
the cost of the real forms (one table read; two table reads and an integer multiply).

Routings: the one with the most modulation edges (operators 1 and 2 each modulate all six others,
which are the carriers; 12 edges; self-feedback), the loop (5 → 3 around 3 > 4 > 5, and 6 > 7 > 8),
and the chain of eight.

**Measured** with `tools/cost/cg.sh` (callgrind 3.22, aarch64 Linux container, Rust 1.95.0,
`-C target-cpu=cortex-a53`, 32 held voices, 64 blocks, only the rendering counted; "Drw" = data
reads and writes; D1 misses from callgrind's cache simulation):

| Case (all eight operators on the form) | Instructions a voice-block | Drw a voice-block | D1 misses a block (32 voices) |
|---|---|---|---|
| at rest (no voice), per block | 622 | 166 | 1 |
| operator-major, most edges, table form | 15,243 | 4,054 | 170 |
| operator-major, most edges, window form | 21,899 | 5,079 | 1,264 |
| operator-major, most edges, sine | 15,115 | 4,054 | 176 |
| operator-major, chain, table form | 14,613 | 3,526 | 174 |
| operator-major (loop interleaved), loop routing, table form | 18,379 | 4,092 | 197 |
| operator-major (loop interleaved), loop routing, window form | **25,131** | 5,212 | 1,466 |
| sample-major, most edges, table form | 24,963 | 4,980 | 175 |
| sample-major, most edges, window form | 31,875 | 6,261 | 1,212 |
| sample-major, loop routing, window form | 31,223 | 5,868 | 1,206 |
| scalar back end, operator-major, most edges, table form | 16,183 | 4,067 | 77 |

Against the gate (28,000 a voice-block with all eight operators on a non-sine form; 2,000 a block
at rest): operator-major meets it in every case, worst 25,131 (the loop routing on the two-read
form, 10 % under); sample-major misses it on the window form. The first sample-major version kept
the operator outputs in registers and cost 30,600 (table) and 37,000 (window) before it was
changed to read the step buffers. At 8 voices operator-major costs the same per voice as at 32
(15,302 against 15,243). The scalar back end (also four lanes as arrays) is vectorised by the
compiler and comes within 6 % of NEON; the table reads (four lane extracts and four paired loads a
read) are most of an operator's cost, not the arithmetic.

Bytes touched (`fmbench footprint`, from the layout): a lane's state 692 bytes (173 a voice), read
and written once a block; per-step scratch 4,352 bytes shared by all lanes (operator buffers 2 KB,
modulation input 256 bytes, the lane's voice output 2 KB); tables 4,100 bytes (one-read forms) or
8,200 (two-read). The hot set of one lane at work is 13.2 KB with the two-read form, inside half
the 32 KB first-level cache (a unit test). Per block, 25 voices touch about 4.3 KB of state, so the
budget's cost is instructions, not the second-level cache.

Correctness, seen pass: NEON and scalar bit-identical (macOS arm64 and Linux arm64), SSE2 and
scalar bit-identical (x86_64 under translation on macOS), for every form, both layouts and the
three routings; the two layouts bit-identical to each other; one golden hash of the output the same
on all three back ends; output finite and bounded with 32 voices; every voice ends after its
release; silence in, silence out.

**Confidence that the full voice fits.** The spike's operator loop is close to the real one: the
real forms have the same read structure; the per-operator pitch envelopes, LFOs, macros, velocity
and scaling are control-rate work (the spike's control step, eight envelopes with an `exp2`, is
already inside its figures; not measured on its own). Estimated additions (not measured): filter and filter envelope about 1,000 a
voice-block (two sections at audio rate), stereo pan and mix about 500, the real envelopes and LFOs
about 1,500. With the worst case at 25,131 that is about 28,000: at the gate. The costliest
features to watch: two-read forms (resonant, odd under modulation) on many operators, the two loop
routings, the filter. Reserves not yet taken, each to be measured: one native minimum and maximum
for the modulation clamp (2 of about 60 instructions an operator-frame of a lane), the fraction by a
fixed-point conversion (1), the table index scaled in the vector before extraction (4), `i16`
tables (cache, not instructions), skipping operators at zero gain (an operator whose envelope and
level make it silent costs nothing; most sounds do not use all eight at full level).

### 3.1 The engine at M1 (measured)

Same tool and settings as above (`tools/cost/cg.sh`, callgrind 3.22 in the aarch64 container, Rust
1.95.0, Cortex-A53 tuning, 64 blocks, only the rendering counted). The engine's cases: sine
operators, every listed operator at level 99, feedback 7, each envelope in a slow decay 1 so the
segment checks run on every control step (the costlier envelope path), held notes, 32 voices.

| Case | Instructions a voice-block | Drw a voice-block |
|---|---|---|
| at rest (no voice), per block | 1,277 | 331 |
| routing 12 (feedback loop through 3 > 4 > 5, interleaved), eight operators | **20,240** (8 voices: 20,361) | 4,416 |
| routing 14 (loop through 3 > 4), eight operators | 18,806 | 4,396 |
| routing 80 (the most modulation edges), eight operators | 16,554 | 4,512 |
| routing 8, eight operators | 16,134 | 4,228 |
| routing 1 (eight carriers), eight operators | 16,214 | 4,264 |
| routing 1, one operator | 3,131 | 1,137 |
| routing 12, eight operators, scalar back end | 21,068 | 4,268 |

The worst plan with eight operators is 20,240 against the gate of 28,000; at rest 1,277 against
2,000. What the real envelopes cost: routing 80 against the spike's same graph on the sine
(15,115) is 1,439 a voice-block more (10 %), with the measured envelope laws, the segment checks,
the per-voice fade and the plan's checks; the loop routing costs more than in the spike (20,240
against 18,379 on its table form) because only its live operators are interleaved, by a filter per
sample. Rest is higher than the spike's (622) because the pool has 40 lanes to look at.

Data misses a voice-block with the device's caches (callgrind `--D1=32768,4,64
--LL=524288,16,64`), routing 12 with eight operators: 1 voice 5.9 first-level and 1.1 last-level;
8 voices 1.0 and 0.1; 25 voices 1.1 and 0.0; 64 voices 2.6 and 0.0
(`tools/cost/cache-baseline.txt`). A lane's state is under 2 KB (a unit test) and a lane at work
touches under 11 KB with its scratch and the sine table (the first-level budget test: at most 16 KB).

Correctness, seen pass (macOS arm64, Linux arm64 in the container, x86_64 under translation on
macOS): the laws against the existing profile's measured numbers (test vectors in `laws` and
renders in `engine`: a lone carrier within 0.02 dB of −17.64 dBFS, the routing gain, level and
envelope levels, the modulation index by J1/J0 within 1 %, decay slopes and time key follow within
2 % of the tables, hold times, frequencies within 0.05 cent of the grid in ratio and fixed mode,
note shift, the fixed-frequency hold at 22.04 kHz, the feedback amounts); a fuzz of random notes,
releases, pedals, limits and sound changes (finite, bounded, native and scalar bit-identical,
every note ended exactly once); a golden render hash the same on every back end; the 88 routings
against the documented chart; every single-byte corruption and truncation of a sound blob refused.

### 3.2 The forms at M2 (measured)

Same tool and settings as 3.1. Cases: eight operators at level 99 on routing 12 (the worst plan),
32 held voices unless stated.

| Case | Instructions a voice-block |
|---|---|
| sines (M1: 20,240) | 18,910 |
| all harmonics (one Q14 read) | 21,454 |
| odd harmonics (two reads) | 27,478 |
| resonant (two reads and a multiply) | **27,636** |
| eight different forms and tables | 25,288 (8 voices: 25,409) |
| resonant, routing 80; routing 1 | 22,976; 22,712 |
| one operator, routing 1 (M1: 3,131) | 2,950 |

The worst plan with all eight operators on the costliest form is 27,636 against the gate of 28,000
(1.3 % under). Sines got cheaper than at M1 because table reads now form their byte offsets in the
vector and convert the fraction in one instruction (`simd`; the same bits on every back end).

Data misses with the device's caches, routing 12, eight operators on eight different form tables and
pitches (`cache-mixed-N`, the worst case for locality) against sines: first level 129.4, 90.4, 98.2
and 143.6 a voice-block at 1, 8, 25 and 64 voices (sines 7.9, 1.2, 1.6, 2.7); last level 5.2, 0.7,
0.2 and 0.1 (sines 1.1, 0.1, 0.0, 0.0). The hot set fits by size (about 27 KB; a unit test), but not
without conflicts: the nine tables alone (eight forms and the sine) put five lines on some of the
first-level cache's 4-way sets wherever they are placed (computed), so these misses are structural
for this case; the second-level cache serves them, an estimated 2-3 µs a voice-block on the device at
its second-level latency (to be timed: `fmbench time`, case "eight mixed forms"). The counts move
with the binary's layout (an unrelated change to fmbench moved them from 81, 41, 67, 110); aligning
each table to a cache line made them worse, and was not kept.

A lane costs the same with one voice as with four (routing 12: one voice alone 72,948 a
voice-block, two 37,110, against 18,910 a voice at 32), and the device shows it too. A narrow kernel
for one and two voices (the scalar back end over the first one or two lanes, voices kept in a lane's
first slots) was measured and not kept: one voice 65,259 (routing 1: 52,299 against 57,109 four-wide),
two voices 47,311, worse than four-wide. Most of a lane's cost does not scale with its voices: the
step's operator buffers zeroed and summed four-wide, the carriers' accumulation, the modulation sums
and the control step. A lane whose cost follows its voices needs buffers and state the width of the
kernel (or voices of several sounds sharing one lane), not a narrower arithmetic type.

## 4. Audio-thread safety, and how it is proven

- **No allocation**, by construction: the core is `#![no_std]` and does not link the `alloc` crate;
  CI builds it for a bare-metal target without the standard library and checks the sources for any
  use of `alloc` or `std`. The shell's tests will run the audio thread under an aborting allocator:
  a global allocator that aborts when a thread-local flag says the thread is inside `process`.
- **No locks or logging**: the core has neither (no `std`); the shell installs sounds by a copy from
  a lock-free buffer, and never logs or formats on the audio thread.
- **No panic path**, checked at link time (`tools/check-no-panic.sh`): `tools/nopanic` is a `no_std`
  static library with one C function that drives every block-path entry (note on, note off,
  render; every routing, form and layout chosen by arguments the optimiser cannot see). Its release
  build must define no panic, bounds-check, unwrap or overflow machinery; and a build whose panic
  handler calls a symbol that does not exist must still link into a C program with dead-code
  removal, which proves no panic path survives optimisation. The release profile keeps integer
  overflow checks on, so an unproven integer overflow is a panic path too and fails the check.
- **Rules that make it achievable** (clippy denies indexing, `unwrap`, `expect`, `panic`,
  `unreachable`): no slice indexing in the block path (iterators, `get`, fixed-size arrays and
  masked indices); integer arithmetic explicitly wrapping or saturating, or provably in range;
  bit tests with `checked_shr` rather than variable shifts; no formatting; vector loads only from
  fixed-size arrays; every table read masks its index.
- **What it found on the spike**: three panic paths the compiler could not remove, all fixed: two
  `enumerate()` counters (overflow-checked), one over a zipped pair of arrays in the end rule and one
  over the carrier accumulation, and the variable shifts `1 << i` in the bit tests. The second step
  was shown to fail when a panic path is present (an index into the output by an unknown voice
  number: the link fails on the missing symbol).

## 5. Toolchain, vector type and targets

- Stable Rust pinned to 1.95.0 (`rust-toolchain.toml`).
- `crates/fm-core/src/simd`: the traits `F4` (four `f32` lanes) and `U4` (four `u32` lanes) with three
  back ends: NEON on aarch64, SSE2 on x86_64, scalar on any target (four lanes as arrays; the
  definition the others are tested against; the `scalar` feature makes it the native one). The
  kernels are generic over the trait; state stays in memory as plain arrays whatever the back end.
- Identical results: every operation is exact IEEE single precision or exact integer arithmetic;
  `mul_add` is a multiply then an add, never fused; minimum and maximum follow one rule (the second
  operand unless the first is smaller), which NEON reproduces by compare and select; float-to-integer
  conversions are used only where the inputs are clamped inside the `i32` range; no libm in the
  block path (2^x is a polynomial at control rate). Result: the three back ends are bit-identical
  (tests), and no tolerance is needed so far. Should a later law need a fused operation or a native
  NEON minimum, its tolerance is stated where it is used.
- Targets: Linux arm64, Linux x86_64, macOS arm64, Windows x86_64 (CI; the Windows job has not run
  yet). The device is Linux arm64 (Cortex-A53).

## 6. Tests and gates

| Gate | Where | State |
|---|---|---|
| `cargo fmt --check`, `clippy -D warnings` | CI, all targets | seen pass on macOS arm64 (M1); CI on all four targets at M0 |
| native back end against scalar, bit for bit (vector operations; the spike on every form, layout and routing; the engine in the fuzz) | CI, all targets | seen pass: macOS arm64, Linux arm64, x86_64 under translation |
| golden render hashes, spike and engine (same on every back end) | CI, all targets | as above, and the scalar back end |
| scalar back end as the native one | CI | seen pass on macOS arm64 and Linux arm64 |
| finite, bounded, every note ends exactly once (fuzz); silence in, silence out | CI | seen pass |
| the laws against the existing profile's measured numbers; the 88 routings against the chart; the coverage table against the code; every corruption and truncation of a sound blob refused | CI | seen pass |
| hot set within half of the 32 KB first-level cache (spike and engine) | unit test | seen pass |
| core builds without `std` and without `alloc` | CI (bare-metal target) | seen pass locally |
| no panic path (symbols, link), the engine's note on and off, pedal, choke, limits and render included | CI (unix) | seen pass on macOS arm64 and Linux arm64 |
| cost: rest at most 2,000 a block, the worst eight-operator voice (engine, and the spike's non-sine forms) at most 28,000, every case at most 10 % over `tools/cost/baseline.txt`; data misses with the device's caches at most 10 % and half a miss over `tools/cost/cache-baseline.txt` | CI (aarch64, callgrind) | seen pass in the aarch64 container |

To come: ABI-level tests of the shell with the aborting allocator; a hosted cost gate (the plugin
through CLAP against the core alone); sample-rate equivalence (48 to 192 kHz against 44.1 kHz
behaviour); the device timing per milestone (hosted target about 30 µs a voice; the existing
profile measured 110–140 µs a voice on the device; the M1 binaries are not yet run there).

## 7. Milestones

Sizes are rough, in working sessions of the size of this round.

| | Milestone | Content | Size |
|---|---|---|---|
| M0 | this round | design, scaffold, vector type, spike, cost and panic gates | done |
| M1 | the voice | sound format and parameter table (all ids), coverage report, plans, the 88 routings as data, per-sound pools, note on/off, end rule, shedding, sine operators with the measured level curves, amplitude envelopes and their time tables, feedback law, the pitch grid (ratio and fixed), device timing | done (section 3.1), except frame-exact note starts (a block is not yet split at events) |
| M2 | waves | the six forms with spread and formant, tables from the measured amplitudes and phases, `i16` (Q14) storage, cost re-measured | done (section 3.2), except a lane's cost following its voice count |
| M3 | pitch | operator and common pitch envelopes, key follow, velocity, random pitch, detune, bend, portamento, mono, key assign | 2 |
| M4 | modulators | voice LFO, sound LFO (all waves, destinations, user wave), the seven macros, level scaling, velocity, part offsets | 2–3 |
| M5 | filter and output | 19 filter types, filter envelope, pan (three kinds), key-on delay, output high-pass | 2–3 |
| M6 | the shell | the CLAP plugin: params, state, note and audio ports, sound snapshots, voice budget, memory, UI hints; allocation tests, fuzz, hosted cost gate | 3 |
| | **the first converted sound set plays** | after M6, with the converter writing this engine's format | |
| M7 | everything | controller-set switches and sensitivities (as decided), micro tuning, the partial items of section 1.5 re-measured, the known-differences register | 2–3 |

## 8. Open questions

- The 288 controller-set switches and 16 controller sensitivities: as per-operator parameters the
  platform's control tree can target (one modulatable "operator level offset" and "pitch offset"
  per operator, the converter folding switches and sensitivities into the targets), or as engine
  state with in-engine controller boxes?
- Micro tuning: the engine's own scales, or the host's tuning?
- The deliberate differences of section 1.5 (band-limiting, formant silence above Nyquist, no
  filter overload, no output clip, widened pitch ranges): keep them all?
- The voice pool size an instance (the existing profile: 128 shared by every sound).

Provisional decisions taken at M1 (conservative defaults, each open to a later ruling):

- The voice pool is 128 voices an instance, a ceiling shared by its sounds; each sound's lanes are
  taken by use from 40 (2.8).
- The existing profile's deliberate departures from the reference are not adopted silently: where
  the reference's behaviour is known and cheap it is played (fixed frequencies held at 22.04 kHz,
  note shift +-48, fractional part velocities), otherwise the difference is listed in
  `docs/known-differences.md`.
- The controller-set switches and the pitch and level controller sensitivities have their ids,
  ranges and defaults in the sound format now; their behaviour waits for M7.
- Parameter defaults are the model's initial values, except portamento (off), so that a sound
  without the parameter plays as before it existed.
