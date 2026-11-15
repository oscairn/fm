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
four-wide, it costs as a full one.

As built (section 3.3): a lane keeps its voices in its first slots (when a voice ends below
another, the higher one's state moves down whole, at the next control step, so its output does
not change), and a lane holding one voice runs one lane wide: its envelopes, step buffers and
kernels on the scalar back end with `L` = 1 (the vector traits and the scalar back end are generic
over the lane count; the state stays four-wide in memory, so a voice moves between widths without
a copy). Two or more voices run four-wide. A two-lane width was measured on the scalar back end
and lost to four-wide (46,878 against 37,110 instructions a voice-block for two voices, routing 12),
so it is not built; a two-lane NEON/SSE2 type is the remaining option (estimated, not measured:
about 15 % off a two-voice lane, for two more back ends).

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
frame), each with a linear ramp to the next step where it reaches the audio. As built:
`render_events` takes a block's note, pedal, bend and parameter events (since M4: a parameter's
raw value by its id in the sound format, the shell's route for knobs, macros and every other
parameter event), and since M7 controller and note volume events (section 3.9), with their frames and renders each sound in
segments between its own events (the sibling engines' rule), its steps starting again at each
event; a step shorter than 16 frames moves each envelope that share of a step (the fall times n/16,
a rise's share 1 - e^(-rate n/16), the hold counted in frames) and ramps the gains over its
frames; its kernels run the whole 16 frames, then the phases and the feedback history are set to
where its last frame left them (frames past it are never heard), so the kernels have no
variable-length path. What is bit-identical under splitting: every other sound's output, always; a
voice of the same sound whose envelopes stand still; and the frames of a note started mid-block up
to the block's end in whole steps (against the same note started at a block's start). What is not:
a voice of the same sound whose envelopes move while a step is cut (the shorter steps' shares are
computed in single precision: 1e-6 against a peak of 0.65 in the test, -116 dB), and the mix of
several sounds (they are summed sound by sound, no longer lane by lane).

| Modulator | Rate and why it is safe |
|---|---|
| Amplitude envelopes (8 a voice), level scaling, velocity, voice and sound LFO amplitude, macros on levels | a target gain a step, ramped linearly a frame (implemented in the spike). 16 frames is 0.33 ms at 48 kHz: the sibling engine measured its reference with no artefact at 16/32/64-sample periods above −55 dB and its own 16-frame ramps at −68 dB on the fastest sweep. Exception to check: segments faster than a step (instant times) become one ramp; the reference's own fastest segments read slower than the existing tables, so a per-segment check is a gate of M1 |
| Operator and common pitch envelopes, key follow, bend, portamento, voice and sound LFO pitch, random pitch, detune, ratio shift | as built at M3: what a note keeps (key follow, velocity, random pitch, detune) is in its increments from note-on, on the grid; what moves is evaluated a step, each increment its note's times 2 to the octaves that move it (one vector `exp2` an operator, 0.15 cent at worst). While a pitch envelope moves, the increments ramp linearly across the step from their value at its start to the law's at its end (kernels instantiated with and without the ramp; a lane whose pitch stands still runs them without it and skips the pitch work after a test of three flags); bend and portamento change at a step's start (an event's frame, the glide clock's tick), as their laws step. Against the per-sample law: the ramp's frequency is within 1.2 cents at the fastest operator pitch envelope (times 0-2, 387 cents/ms, 129 cents a step), within 0.65 cent from time 3, within 0.002 cent at the fastest common one (14 cents/ms) (computed from the chord of the exponential); the fastest sweep (4800 cents in 12 ms, key 93) leaves the phase 0.0022 cycles off the law's (0.26 cycles with a staircase instead of the ramp: a test). On a modulator at the highest index (4 pi) the momentary frequency error is the same share (at most 7e-4) and that phase error becomes a carrier phase offset of at most 4 pi × 0.0022 × 2 pi ≈ 0.17 rad after the fastest 4-octave sweep, constant once it ends. Within the existing profile's 1-cent pitch tolerance except during the fastest operator pitch envelope segments (12 ms at most), so no per-sample pitch is paid for |
| Sound and voice LFOs | evaluated a step. At 48 kHz that is 3,000 values a second: up to about 150 Hz that is 20 a cycle. As built at M4 (voice LFO): its value at a step's end moves the pitch through the increments' ramp (as the pitch envelopes) and the amplitude through the gains' ramp. The extended range's top rate is 1,357 Hz (the measured table); above about 150 Hz the LFO is under-sampled: the audio-rate LFO for those speeds is not built (docs/known-differences.md). Against the per-sample law (computed from the chord of each wave between step ends, T = 1/3000 s): a sine of rate f and swing A is within A (pi f T)^2 / 2, at the fastest normal speed (49.8 Hz) 0.13 % of the swing (3.3 cents at the deepest, 2400 cents; 0.13 dB of a 96 dB amplitude swing); a triangle's corner is cut by up to 2 A f T for one step (at 49.8 Hz and 2400 cents 80 cents, at 6 Hz and 93 cents 0.37 cent); a square's or sample and hold's edge is a 16-frame ramp. Measured: the pitch route at 1 Hz, 93 cents, within 0.2 cent of the per-sample law's mean frequency over every 8 cycles (a test; the vector exp2 alone is within 0.15 cent). The sound LFO (M4, section 3.6): one a sound, its value at each step's end shared by its voices, the same routes; its top rate 11.86 Hz is 253 steps a cycle |
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
  oldest; never a held mono key's voice (M3). A note that gets no voice (every
  candidate protected, or no free slot while fades are under way) ends at once; every note-on is
  answered by exactly one end (the fuzz test). A sound is installed only while none of its voices
  sounds (`choke` fades them first).

### 2.9 Cost and memory reporting

- Cost: callgrind on aarch64 (`tools/cost/cg.sh`), instructions a voice-block per case, gated in CI
  (section 6); the device timing binary is `fmbench` (time a voice-block) built for the device.
- The lane gate (`cg.sh check`): one voice alone, and two voices of a sound, against a voice of 32
  in one sound; one voice in each of five sounds against five voices in one sound. Targets 1.5,
  1.25 and 1.6: **open** (reached 2.51, 1.96 and 1.88, section 3.3); the gate holds the reached
  figures (`ratio-` lines of `tools/cost/baseline.txt`, 2 % allowed) until the targets are met.
- Memory: the instance is fixed-size structs (`size_of` is the figure, reported through
  `oscairn.memory` as instance bytes, plus per-context bytes); tables are shared read-only data,
  reported once as shared bytes. Bytes touched per voice-block are stated by `fmbench footprint`
  and checked by a unit test (section 3).

### 2.10 The shell

Built at M6 (`crates/fm-clap`; what it offers, its ids, parameters and state: `docs/plugin.md`).
CLAP's types are `clap-sys` 0.5.0, the platform's extensions its pre-generated bindings
(`oscairn-clap-sys`, built on `clap-sys`'s types: one set, no casts), from the submodule
`third_party/oscairn-clap` at c2119db. No framework: `src/ffi.rs` holds the entry, the factory and
every extension's table, each a thin call into `src/shell.rs`, which holds the instance; the roles
of the platform's C++ helper (sound contexts, the selection queue, note ownership, `NOTE_END`, the
snapshot) are written there in Rust, and the voice-budget order is the core's (2.8).

- Data by thread: atomics (each sound's values, bus, id, pins and channel mask; the selection queue;
  port activation); the main thread's (names, which slots are prepared, ids); the audio thread's
  (the engine, its output pairs, the note table, the owed note ends, the limits). The main thread
  takes the audio thread's data only to activate and to load a state, with a flag pair: the audio
  thread, finding it held, skips that callback (a silent block) and never waits. A prepared sound
  is written into a free slot's buffer by the main thread and installed by the audio thread with
  the first selection of it (the selection pins it), so no copy is ever read while written.
- Rendering: the core renders a block in stretches of at most 128 frames (`render_buses`, any
  length up to `BLOCK`: a block of 128 frames is the core's own block, bit for bit), each sound
  into its own output pair (its bus, main where the bus is not active, or a ring-out pair), only
  what sounds written; the shell copies each written pair to its port (left to both channels for a
  sound without pan) and zeros an active port nothing played into. Note ends are reported at the
  end of the stretch they happen in.
- Parameter values are staged in the core as they arrive and each sound changed is prepared once
  at the start of the next stretch (`stage`): at most once a stretch however many values come.
- Sleep: no note, voice, owed end or queued selection; the engine's clock moves on over skipped
  blocks from `steady_time` (`skip`).
- Proven: the audio thread's calls allocate nothing (the tests' counting allocator) and reach no
  panic function (section 4).

Cost: `tools/cost/cg.sh`'s `hosted-*` cases (`fmhost`, a binary of its own: linking the plugin into
`fmbench` changed the core's code there, a voice alone 0.7 % dearer); figures in
`tools/cost/baseline.txt` and section 3.8.

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

A lane cost the same with one voice as with four (routing 12: one voice alone 72,948 a
voice-block, two 37,110, against 18,910 a voice at 32), and the device shows it too: section 3.3.

### 3.3 Lanes and headroom (measured)

Same tool and settings. Where a block's instructions go, routing 12 with eight sines, before this
round (a profiling build with each phase kept out of line, so totals differ from the gate's by a
few per cent): (a) one voice in one sound, (b) four voices in one sound, (c) one voice in each of
five sounds, (d) 32 voices in one sound; instructions a block.

| Phase | (a) | (b) | (c) | (d) |
|---|---|---|---|---|
| operator kernels (single operators and the feedback loop) | 55,256 | 55,256 | 276,280 | 442,048 |
| control step (envelopes, segment checks, end rule) | 8,254 | 12,130 | 41,413 | 97,040 |
| modulation sums | 1,936 | 1,936 | 9,680 | 15,488 |
| buffers cleared and copied | 2,378 | 2,378 | 11,886 | 19,017 |
| carriers' accumulation and the output | 1,589 | 1,589 | 7,945 | 12,712 |
| per-lane dispatch | 1,642 | 1,642 | 8,210 | 13,136 |
| per block (shedding checks, the lane scan) | 1,086 | 1,086 | 1,118 | 1,142 |
| total (a voice-block) | 72,141 (72,141) | 76,017 (19,004) | 356,531 (71,306) | 600,583 (18,768) |

The kernels were three quarters of a lane, and a lane cost the same at any voice count; of the rest
only the segment checks scaled with voices. The feedback loop cost 84 instructions an operator-frame
of a lane against 36 for an operator alone (its inputs summed sample by sample, every source
tested, its state not kept in registers). Decided from it: (i) a lane holding one voice runs one
lane wide (section 2.2; a two-lane width on the scalar back end lost to four-wide), (ii) per-voice
skipping inside four-wide buys nothing (the arithmetic is the same for one voice as for four), and
(iii) sounds sharing lanes was not built: it cannot help one voice alone, and it helps only sounds
of compatible plans (the sibling engine's risk list applies: every stage's constants per element,
more code, the shell's batching of sounds without events). After this round (the same build):

| Phase | (a) | (b) | (c) | (d) |
|---|---|---|---|---|
| operator kernels | 21,488 | 41,736 | 107,440 | 333,888 |
| control step | 8,006 | 11,370 | 40,171 | 90,960 |
| step buffers, sums, accumulation and dispatch | 2,760 | 4,760 | 13,800 | 38,080 |
| per lane and per block | 1,667 | 1,683 | 4,143 | 4,259 |
| total (a voice-block) | 33,920 (33,920) | 59,549 (14,887) | 165,554 (33,111) | 467,187 (14,600) |

What each change bought (the gate's figures, instructions a voice-block): one voice one lane wide,
72,933 to 53,177; the feedback loop's inputs from below summed a step at a time and the loop written
out for two or three operators, routing 12 at 32 voices 19,066 to 15,530 (eight resonant forms
27,636 to 23,313); the step buffers no longer cleared each step and the envelope step over the
arrays, 15,530 to 15,045 (one voice 37,721 to 36,352); the timed events' short steps cost one voice
about 2 % (36,155 to 36,693) and nothing four-wide (14,626 at 32 voices).

| Case | Before | After |
|---|---|---|
| routing 12, eight sines, 32 voices | 18,910 | 14,626 |
| one voice alone (fm-s1-v1) | 72,948 | 36,693 |
| two voices (fm-s1-v2) | 37,110 | 28,613 |
| one voice in each of five sounds (fm-s5-v1) | 71,915 | 35,746 |
| five voices in one sound (fm-s1-v5) | 29,682 | 19,004 |
| worst eight-operator plan (gated) | 27,636 (resonant) | 23,021 (the spike's loop on its window form; the engine's worst: odd forms, 22,642) |
| all harmonics; odd; resonant; mixed forms | 21,454; 27,478; 27,636; 25,288 | 16,618; 22,642; 22,466; 20,358 |
| at rest, a block | 1,277 | 1,186 |

The lane gate's ratios (section 2.9): one voice alone 3.86 to 2.51 (target 1.5), two voices 1.96 to
1.96 (target 1.25), one voice in each of five sounds 2.42 to 1.88 (target 1.6): all three open. What
stops them: one lane wide, a kernel cannot use the vector across voices, and the compiler's
vectorising across a step's frames still pays the table reads' lane extracts (an operator alone
costs 17 instructions a frame one lane wide against 9.4 a voice four-wide; the feedback loop, serial,
28); the control step costs about as much for one voice as for four. Two voices run four-wide. The
remaining routes: a two-lane vector type, explicit vectorising across frames for operators without
feedback, and sounds sharing lanes for compatible plans (case c only).

Bytes: the scratch is the step's operator outputs four-wide (2 KB) and one-wide (512 bytes), kept in
the engine and never cleared; a lane at work touches 9,492 bytes with its scratch and the sine table
(10,516 before: the lane's output is a step's, no longer a block's), within the first-level budget
of 16 KB (a unit test). Code: the one-wide instantiations of every kernel and the short step add
40,872 bytes of text to fmbench (583,546 to 624,418, both back ends linked; the engine's
out-of-line functions 27,888 to 53,356 bytes). A block whose lanes run at both widths touches both
instantiations, in the core's own instruction cache and the shared second-level cache. Data misses
with the device's caches (`tools/cost/cache-baseline.txt`): sines 2.0, 0.3, 0.5 and 2.6 first-level
a voice-block at 1, 8, 25 and 64 voices (7.9, 1.2, 1.6, 2.7 before); eight mixed forms 50.3, 55.5,
68.9 and 105.8 (129.4, 90.4, 98.2, 143.6); last level unchanged (1.1 and 5.3 at one voice). These
counts move with the binary's layout (section 3.2).

Correctness, seen pass: a voice's state after every block, and with silent neighbours its output,
the same bits alone, in a pair, in a full lane and while neighbours start and end around it (moving
it to another slot and between the widths); the golden hashes unchanged; the fuzz (native against
scalar, every note ending once) with its notes as timed events every other block; a note starting on
its frame; another sound's bits unchanged by a sound's events.

### 3.4 Pitch at M3 (measured)

Same tool and settings. As built: the laws in `laws.rs` (the existing profile's measured tables;
bend from the documented ranges only); per voice a pitch state in the lane (`Pitch`: each
operator's increment from the pitch the note keeps, the operator envelopes' levels with their
segments as bit masks, the common envelope's level, segment and the note's factors, the glide left
and its step), 348 bytes a lane (87 a voice), read only while a voice's pitch moves; three flags
read every step beside the lane's other step flags. A segment's rate is now made when it is entered
(from the note's key) instead of kept for every segment: 512 bytes off a lane, the same bits. A
lane is 1,684 bytes (about 1,772 before M3); the hot set 9,368 bytes (the first-level budget test:
at most 16 KB). The engine keeps each sound's bend, latest note, random source and a mono sound's
16 kept keys, and one glide clock (9.977 ms).

Mono: one voice a sound, last-note priority; with portamento a note whose predecessor's key is
down takes over its voice (legato), otherwise the sounding note fades out over 13.3 ms by the
square of the share left while the new one starts (also key assign single's retrigger); a key
taken over is kept and returned to when the newer key is released. Every note ends exactly once: a
voice whose note lives on as a kept key does not report the end when it fades; a kept key's note
ends at its release, when a 17th is kept, or at `choke`. The fuzz plays random sounds with every
pitch parameter (mono, portamento, both envelopes, random pitch, bend ranges) and bend events,
native against scalar bit for bit, every note ending once.

Cost, instructions a voice-block (at M3's start / after): routing 12 sines at 32 voices 14,626 /
14,581; one voice alone 36,693 / 37,007; eight mixed forms 20,358 / 20,317; at rest 1,186 / 1,215 a
block; the worst eight-operator plan 23,021 / 23,021 (gate 28,000); the lane gate's ratios 2.509 /
2.538, 1.956 / 1.959, 1.881 / 1.894 (2 % allowed). New cases, both pitch envelopes moving and
portamento gliding on every voice: 18,332 at 32 voices (25 % over sines: the ramped kernels, the
increments made each step, the envelopes) and 47,065 for one voice alone (27 % over). Sounds that use
no pitch feature render the same bits as before M3 (the golden hashes unchanged).

The cache figures (`tools/cost/cache-baseline.txt`) moved with the binary's layout: the same engine
read 61.0 and 90.7 first-level misses a voice-block at cache-mixed-25 in two fmbench builds that
differ only by an added cost case. The sine and the form tables are now one static aligned to the
first-level cache's way (8 KB), and callgrind runs with an empty environment: two builds differing
by unrelated cases read within 3 % of each other at every case. The aligned placement's figures are
higher than the earlier lucky ones (sines at one voice 11.0 against 2.0; mixed forms 83.9 to 142.4
against 55.5 to 105.8); the device's own placement is not known either way.

### 3.5 Modulators at M4 (measured)

Same tool and settings. Built: the voice LFO (pitch and amplitude routes, per-operator offsets),
the seven macros, level scaling, the part's four envelope offsets, timed parameter events, and the
sound LFO (section 3.6). The voice LFO's filter route came with the filter (M5, section 3.7).

- **Per sound, once** (`Prepared::new`): the macros and the part's offsets move the operators' raw
  values before the laws (depth on modulators' levels, divided by 1 + feedback on the operator that
  feeds back; attack, decay, sustain and release on modulators' envelope values; ratio on
  ratio-mode modulators' ratios; texture on the forms; the offsets on every operator), so they cost
  nothing in the block path. Level scaling is added at note-on (the six-operator model's documented
  curves; not measured).
- **Voice LFO**: one a voice, evaluated at each step's end (`voice_lfo`): its pitch share moves the
  increments through the pitch envelopes' ramp, its amplitude share each operator's gain through
  the bend's grid (`laws::amp_loss_log2`, 90 values, 360 bytes, read only by a sound with an
  amplitude depth); a sound without one tests two flags a step. Lane state: phase, frames since the
  note started and the sample-and-hold source, 48 bytes a lane (12 a voice): a lane is 1,732 bytes,
  the hot set 9,416 bytes (computed from the structures; the first-level budget test: at most
  16 KB).
- **Parameter events** (`Kind::Param`, `Engine::param`): the raw value set in the sound's raw values
  (now kept in `Prepared`) and the sound prepared again at the event's frame; a change of what is
  rendered (the plan), mono or key assign waits until the sound is silent (a voice's state stays
  valid). Measured (callgrind, Cortex-A53 tuning): one event that prepares the sound again costs
  about 17,200 instructions (a block of one voice with an event at its start, 54,621, against the
  same block without, 37,433), half a voice-block of one voice alone; not redesigned.
- **Controllers**: the model's fixed routes from controllers to the LFO depths are not part of its
  M4 parameters (the controller sets decide them); the controllers and the boxes came at M7
  (section 3.9).

Cost, instructions a voice-block (M3 / M4): routing 12 sines at 32 voices 14,581 / 14,646; one voice
alone 37,007 / 37,237; eight mixed forms 20,317 / 20,383; at rest 1,215 / 1,226 a block; the worst
eight-operator plan 23,021 / 23,021 (gate 28,000); the lane gate's ratios 2.538 / 2.542, 1.959 /
1.959, 1.894 / 1.896 (baselines 2.509, 1.956, 1.881, 2 % allowed). Sounds that use no M4 feature
render the same bits (the golden hashes unchanged) and cost 0.3-0.9 % more (the flags a step and
the larger lane). New cases, the voice LFO on pitch and amplitude and level scaling on every
operator of every voice: 19,081 at 32 voices (30 % over sines: the ramped kernels, the increments
made each step, the bend) and 48,230 for one voice alone. Data misses with the device's caches:
sines 2.0, 0.3, 0.4, 2.8 first-level a voice-block at 1, 8, 25 and 64 voices; mixed forms 57.0,
47.9, 59.2, 85.1 (the baseline's 102.0, 83.9, 96.7, 142.4: the layout moved again, section 3.4).

### 3.6 The sound LFO at M4 (measured)

Same tool and settings. Laws from the existing profile's measurements (each row's accuracy in
`docs/coverage.md`): the rate table at every speed (0.0019 to 11.86 Hz), the one time table for
delay, fade in, hold and fade out (0 to 11.5 s), the thirteen waves and the user wave, the six
start phases, the 12 ms onset after a reset, key-on reset each, first and off, the depth ratio
rule, and the destinations' depths at the LFO's top (bottom for level).

- **One a sound**, as the reference (its key-on reset modes are per sound: first resets only when
  no key of the sound is down). State: phase, frames since its envelope started, frames still
  standing still after a reset, a random source and the held value, 20 bytes a sound (`Slfo`), in
  the engine beside the lanes, read only by a sound that uses it. It runs once a sound a control
  step, before the sound's lanes (`Slfo::steps`): its value at each step's end, a scalar shared by
  every voice. Off keeps it free-running from the engine's clock (a first key takes the clock's
  phase and starts the envelope).
- **Destinations** (the reference's numbers, the sound format's range widened from 0-69 to 78):
  71 the voice LFO's speed (its increment a step), 74 feedback (the feedback amount at the moved
  value, the plan prepared with the feedback path when the destination has depth), 75 operator
  frequency (into the increments as the voice LFO's pitch), 76 form (the plan's forms moved a
  step, `Plan::with_waves`), 77 detune (a move per voice and operator, held a lane and made again
  only when the moved detune changes or a voice starts: 136 bytes a lane), 78 level (straight in
  dB on each operator's gain). Each operator's box and ratio decide what reaches it; frequency
  and detune reach ratio-mode operators only. 70 pan, 72 cutoff and 73 resonance are parsed and
  reported as not covered (M5); 0-69 are the effects' parameters, reported. Tempo sync is
  reported as not covered (no tempo input); random speed has no law (M7).
- **Control rate**: the LFO's value at each step's end moves pitch through the increments' ramp
  and level through the gains' ramp, as the voice LFO (section 2.3). Against the per-sample law
  (T = 1/3000 s at 48 kHz): its fastest speed, 11.86 Hz, is 253 steps a cycle; a triangle's corner
  is cut by at most 2 A f T for one step (A the swing: 28.5 cents at depth 127, 3600 cents; 1.8
  cents at depth 8); a square's, sample and hold's or user step's edge is a 16-frame ramp; the
  stepped destinations change at a step's start. Measured: the frequency route, a triangle at
  1.64 Hz and depth 8, within 0.2 cent of the per-sample law's mean frequency over every 8 cycles;
  the waves and envelope within 1e-5 and 1e-4 of the laws at every step's end; the stepped
  destinations at the LFO's top the same bits as the sound with the moved value (tests).
- **Hot state**: a sound that does not use it tests one flag a segment and three a step (its
  `slfo` bits); the lane grew by the detune moves to 1,868 bytes, the hot set to 9,552 bytes
  (computed from the field sizes; the first-level budget test passes); `Prepared` about 290 bytes
  more a sound (read once a step only by a sound that uses it).

Cost, instructions a voice-block (4041ba3, measured again here / with the sound LFO): routing 12
sines at 32 voices 14,647 / 14,767; one voice alone 37,237 / 37,433; eight mixed forms 20,383 /
20,525; pitch envelopes 18,565 / 18,596; the voice LFO case 19,081 / 19,223; at rest 1,226 / 1,217 a
block; the worst eight-operator plan 23,021 / 23,021 (gate 28,000); the lane gate's ratios 2.535,
1.959, 1.893 (baselines 2.509, 1.956, 1.881, 2 % allowed). Sounds that do not use it render the
same bits (the golden hashes unchanged) and cost 0.2-0.8 % more (the flags and the step's extra
argument). New cases, the sound LFO (triangle, delay) on every operator's frequency, level and
detune of every voice: 17,785 at 32 voices and 49,321 for one voice alone (the first version, the
detune made every step, was 42,003). Data misses with the device's caches: sines 1.8, 0.3, 0.5,
2.9 first-level a voice-block at 1, 8, 25 and 64 voices; mixed forms 72.4, 72.3, 77.0, 111.2.

### 3.7 Filter, pan and key-on delay at M5 (measured)

Same tool and settings. Laws from the existing profile's measurements (`crates/fm-core/src/filter.rs`;
each row's accuracy in `docs/coverage.md`): the cutoff table (193 measured points, straight in Hz
between), the least Q near the top, Q 0.5 × 2^(r / 16), key follow, velocity, scaling, distance,
gain, the wide band-pass's edges, the envelope's level, time and hold tables with its depth,
velocity and key follow, the voice LFO's filter depth, the sound LFO's pan, cutoff and resonance
depths, the pan law, scaling and random pan, and the key-on delay table.

- **Structure**: every type is two sections the measurements fit with no free shape parameter, a
  state-variable section (`lp += F bp; hp = x - lp - d bp; bp += F hp`; F = 2 sin(pi fc / rate), d
  = 1 / Q) and a one-pole low-pass with the same F, in series or summed per type (the 24 dB types two
  sections in series, the 18 dB types a section and the one-pole, the low-pass and high-pass types a
  section or the one-pole then a Q-1 high-pass at the high-pass cutoff, the dual types two sections
  summed, the dual band-stop two notches in series, the second section at F × 2^(distance / 16) up
  to 1.75). Stored values 9, 18 and 20 are no type (thru, reported). Thru at gain 255 runs nothing.
- **Per voice, across the lane**: after the operators, before the mix. At each step's end (16
  frames) every voice's envelope (scalar, a segment's frames exact inside a step) and LFO moves give
  its coefficient targets, computed for the lane's voices at once with the vector type (`exp2`, a
  degree-7 sine, the least Q's table and the dual types' limit lane by lane only where they act);
  F, damping and the one-pole's coefficient ramp linearly a frame from the last step's, a voice's
  first step starting at its own. The sections run four voices a vector (one wide for a lane holding
  one voice), bit-identical on every back end (tests: the fuzz compares NEON or SSE2 with scalar).
  Against the sections' transfer functions every type's kernel is within 0.01 dB (test); the 12 dB
  high-pass through the engine is within 0.3 dB of the second-order law (test).
- **Stability**: the damping is held to the least Q at the cutoff, and below 44.1 kHz under the
  section's edge (2 / F - F / 2); the second section to F 1.75 with its damping under 0.9 of the edge;
  every state is held within +-8 (18 dB over full scale) each frame, so no output can run away or go
  non-finite (the reference's overload is not modelled). Test: 400 random filters (half at cutoff
  1023, resonance 127, distance +127), every voice of a lane on a noise of +-4 with the sound LFO and
  the LFO random each step and steps of 1-16 frames: every output finite and within +-64. The fuzz
  (random sounds with the filter, pan, key-on delay and the sound LFO, both back ends) and the
  no-panic driver cover them too.
- **At rest**: a state below 1e-15 at a step's end is set to 0, so a decaying filter never reaches
  subnormals (test: an impulse at resonance 100, then 20,000 steps of silence: no subnormal output,
  the states exactly 0; the fuzz fails on any subnormal sample).
- **Pan and output**: `render` writes left and right. A sound without pan adds to the left channel,
  which is copied to the right at the block's end: its bits are the mono output's in both channels
  (the golden hashes unchanged; every test checks the two channels equal). A sound with pan adds to a
  pair of its own, mixed in at the end; each voice's position (alternate, scaling, random, held
  within -64..+63) through the pan law, the sound LFO's pan at each step's end, the gains ramped a
  frame. A centred voice through the pan path gives the mono bits (test).
- **Key-on delay**: a note waits in a list of 32 (an instance's) and starts at its frame as an event
  of its sound (released before then, it never sounds and ends at once); tempo sync is reported (no
  tempo input). Test: the delayed note plays the bits of the same note started at that frame.
- **Hot state**: a lane's filter and pan state (`filter::Voices`, 364 bytes: five states, five
  coefficients, the envelope, what a note keeps, the position and gains), read only when its sound
  uses them; the lane is 2,232 bytes, the hot set with it 9,916 bytes (computed from the field
  sizes; the first-level budget test passes). The engine holds the pan pair (1 KB, touched only
  when a sound has pan) and the delayed notes (640 bytes). `Prepared` about 140 bytes more (from the
fields).

Cost, instructions a voice-block (40a0617 / c984ce2): routing 12 sines at 32 voices 14,767 / 14,872
(+0.7 %); one voice alone 37,457 / 38,282 (+2.2 %); five sounds of one voice 36,461 / 36,889 (+1.2 %);
the worst plan without a filter 22,809 / 22,914; at rest 1,216 / 1,276 a block. Sounds not using M5
render the same bits; their cost rise is the right channel's copy and a few tests a step (the
target was under 1 %: met at 32 voices, not for a voice alone). The lane gate's one-voice ratio is
2.574 against 2.509 + 2 % (not met; the other two 1.978 and 1.892, met). The filter at 32 voices /
one voice, its envelope moving on every voice: 24 dB low-pass 17,089 / 45,040, 18 dB low-pass
16,815 / 43,927, 12 dB high-pass 16,687 / 43,407, 12 dB band-pass 17,087 / 45,024, 12 dB band-stop
16,685 / 43,407, dual band-stop 17,249 / 45,536 (the costliest type: +2,377 at 32 voices), low-pass
12 + band-pass 6 17,167 / 45,328, thru with a gain 15,110 / 39,163; pan alone 16,000 / 39,768. The
worst plan with the costliest filter and every modulator on it (the dual band-stop, its envelope,
the voice LFO on the cutoff, the sound LFO on cutoff, resonance and pan) is 26,943 (gate 28,000;
the first version was 28,041): the plan 22,914, and by subtraction of the separate cases the filter
about 2,400, pan about 1,100, the LFOs' moves about 500. The gate's worst case includes the filter cases (every `fm-r*-8`
case). Data misses with the device's caches: sines 19.6, 3.4, 2.5, 3.7 first-level a voice-block
at 1, 8, 25, 64 voices (the right channel's 8 lines a block); mixed forms 102.8, 43.7, 48.2, 69.8.

### 3.8 The shell at M6 (measured)

The lane gate's one-voice ratio, back under its limit by removing work: the core writes only what
sounds (`render_buses`: a right channel that is the left is left to the caller, which copies its
output anyway; the left cleared only when a lane first plays into it) and skips the pool's
over-limit scan when the limit is the pool's own (a note starts only within it). A voice alone
38,282 → 37,923, at 32 voices 14,872 → 14,863: 2.552 against 2.509 + 2 % = 2.559 (met, a margin of
0.3 %: linking the plugin into `fmbench` alone moved it to 2.584, so the hosted cases are a binary
of their own). Golden hashes unchanged.

Through CLAP (`fmhost`, a 128-frame block, the routing-12 eight-sine case; the core alone in
brackets): asleep 1,148 a block (the core at rest 1,190); awake at rest, an ignored controller each
block, 3,291 a block; eight held voices of one sound 15,824 a voice-block (`fm-r12-8-v8` 15,034:
+5.3 %, about 6,300 a block); one voice in each of five sounds 38,197 (`fm-s5-v1` 36,846: +3.7 %,
about 6,800 a block). The overhead a block: the selection queue and limits, the bus map, the event
translation, copying the output pairs to the host's channels and the idle test's count of voices
(a pass over the lanes). A parameter change costs one preparation a sound a stretch (about 17,200),
however many events.

### 3.9 Controllers and controller boxes at M7 (measured)

Events: `Kind::Control` (a controller by its MIDI number, or `PRESSURE`) and `Kind::Volume` (a
note's volume) join the timed events of 2.3; the shell sends each to the sound its channel selects
(`docs/plugin.md`). Each sound keeps its channel controllers (mod wheel, aftertouch, both foot
controllers, breath, expression) and, when one moves, the bend moves (if the sound has boxes) or the
sound is prepared or installed, makes again what they do to it (`Mods`: the output gain, each
operator's level offset, the cutoff and resonance offsets): once an event, never a step. A step
reads it only when it moves something (`on`), so a sound without a route pays a test a segment and a
flag a lane step. The sostenuto pedal is a bit a lane (`latch`: the voices whose keys were down
when it went down), released as the sustain pedal's are.

Boxes (`crates/fm-core/src/boxes.rs`, laws in `docs/coverage.md`, `boxN_*`): a box's steps are
summed per destination within its span. The operator level: an operator a box can raise is
prepared at the top the boxes reach (its envelope levels, the attack's overshoot, the plan's live
operators: a silent operator a box can raise is rendered), and the boxes' offset, the level law at
(level + steps) against the law at that top (log2, at most 0), is added where the LFOs' losses are,
so it rides the gain ramp of each step. The cutoff's values and the resonance's steps join the sound
LFO's in the filter's step (coefficients ramped). Expression and the notes' volumes multiply the
lane's output after the filter, ramped linearly across a step (the lane's voices at once, not
inlined: inlined, it moved a voice alone 0.9 % with no controller at work).

Cost (callgrind, Cortex-A53 tuning, against 4e5803c before M7): sounds without a route render the
same bits (golden hashes unchanged) at +0.0 to +0.9 % (a voice alone 37,923 → 37,920, routing 12
at 32 voices 14,863 → 14,973, the costliest filter plan 26,927 → 27,091, one operator 2,850 →
2,876); a parameter event's preparation 55,586 → 57,055 (the bigger sound, the boxes read once).
Every box at work (30 on the operators' levels, the cutoff, the resonance, expression, a controller
every block, so the boxes are evaluated again each block) on the costliest plan with the costliest
filter: 27,389 a voice-block (the 28,000 gate met); a voice alone so 44,727. Lane gate: one voice
2.533 against 2.509 + 2 % (met), two voices 1.967, five sounds 1.888. State: a lane 34 bytes of
fields more (`latch`, `vol`, `og`, `vol_on`; the 32 bytes of gains read only while one is not 1),
`Mods` (46 bytes of fields) a sound read once a segment, the sound itself 534 bytes more (the boxes' 267 values: cold).

## 4. Audio-thread safety, and how it is proven

- **No allocation**, by construction: the core is `#![no_std]` and does not link the `alloc` crate;
  CI builds it for a bare-metal target without the standard library and checks the sources for any
  use of `alloc` or `std`. The shell's tests run every audio-thread call under a counting global
  allocator (a thread-local flag set around the call): any allocation fails the test.
- **No locks or logging**: the core has neither (no `std`); the shell passes selections through a
  lock-free queue, installs a prepared sound by a copy the main thread no longer writes, keeps
  shared values in atomics, never waits for the main thread (a block it holds the engine through
  is skipped, silent) and never logs or formats on the audio thread.
- **No panic path in the shell**: its audio-thread callbacks (process, reset, port activation,
  parameter flush, ring-out, the voice budget's) are walked along every direct call and tail call
  of the plugin's release build (`tools/check-no-panic-shell.awk`); no panic function may be
  reached, and the renderer must be (the walk reached 94 functions on macOS arm64, 107 on Linux
  arm64). Shown to fail with an index
  added to `process`. What it found: two `enumerate()` counters and an open range's counter
  (overflow-checked) and variable shifts in the shell; all fixed.
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
| cost: rest at most 2,000 a block, the worst eight-operator voice (engine, and the spike's non-sine forms) at most 28,000, every case at most 10 % over `tools/cost/baseline.txt`; data misses with the device's caches at most 10 % and half a miss over `tools/cost/cache-baseline.txt`; the lane gate's ratios at most 2 % over their baseline (targets open, 2.9) | CI (aarch64, callgrind) | seen pass in the aarch64 container |
| a voice the same bits at every lane width and slot; a note on its frame; a sound's events leave other sounds' bits alone | CI, all targets | seen pass: macOS arm64, Linux arm64, x86_64 under translation |
| the shell through its C entry (in-process host): every required callback; a note the core's render bit for bit; sharing on bus ports; snapshot and state round trips; sleep and wake; voice limits; one preparation a stretch; MIDI, pedal, choke; reset; no allocation on the audio thread | CI, all targets | seen pass: macOS arm64, Linux arm64 (M6) |
| no panic function reachable from the shell's audio-thread callbacks | CI (unix) | seen pass: macOS arm64, Linux arm64 (M6) |
| the plugin builds and loads (entry, factory, one instance) | CI, all targets | seen pass: macOS arm64, Linux arm64 (M6) |

To come: sample-rate equivalence (48 to 192 kHz against 44.1 kHz
behaviour); the device timing per milestone (hosted target about 30 µs a voice; the existing
profile measured 110–140 µs a voice on the device; the M1 binaries are not yet run there).

## 7. Milestones

Sizes are rough, in working sessions of the size of this round.

| | Milestone | Content | Size |
|---|---|---|---|
| M0 | this round | design, scaffold, vector type, spike, cost and panic gates | done |
| M1 | the voice | sound format and parameter table (all ids), coverage report, plans, the 88 routings as data, per-sound pools, note on/off, end rule, shedding, sine operators with the measured level curves, amplitude envelopes and their time tables, feedback law, the pitch grid (ratio and fixed), device timing | done (section 3.1); frame-exact note starts since (section 2.3) |
| M2 | waves | the six forms with spread and formant, tables from the measured amplitudes and phases, `i16` (Q14) storage, cost re-measured | done (section 3.2); a lane's cost follows its voice count in part (one voice one lane wide), the lane gate's targets open (section 3.3) |
| M3 | pitch | operator and common pitch envelopes, key follow, velocity, random pitch, detune, bend, portamento, mono, key assign | done (section 3.4) |
| M4 | modulators | voice LFO, sound LFO (all waves, destinations, user wave), the seven macros, level scaling, velocity, part offsets | done (sections 3.5 and 3.6; the sound LFO's pan and filter destinations at M5, tempo sync not covered) |
| M5 | filter and output | 19 filter types, filter envelope, pan (three kinds), key-on delay, output high-pass | done (section 3.7) but the output high-pass (it would change every sound's bits: section 8) |
| M6 | the shell | the CLAP plugin: params, state, note and audio ports, sound snapshots, voice budget, memory, UI hints; allocation tests, fuzz, hosted cost gate | done (sections 2.10 and 3.8, `docs/plugin.md`) but a shell fuzz and UI roles beyond the output bus |
| | **the first converted sound set plays** | after M6, with the converter writing this engine's format | |
| M7 | everything | controller-set switches and sensitivities (as decided), micro tuning, the partial items of section 1.5 re-measured, the known-differences register | in part (section 3.9): the channel controllers, note expressions, the sostenuto pedal and the controller boxes on the operator level, cutoff and resonance; not the sensitivities, the other box destinations, the key controller boxes, micro tuning, the sound LFO's random speed |

## 8. Open questions

- The controller boxes are in the engine (M7, section 3.9). Open: modulatable box destinations
  (`PARAM_MOD`) beside them, as the sibling engines offer; the box destinations without a law
  (the operator frequency with the pitch sensitivity, the voice LFO's depths, the envelopes'
  times): measure them, or play documented guesses; CC 65 (the portamento switch over a stored
  parameter).
- Micro tuning: the engine's own scales, or the host's tuning?
- The deliberate differences of section 1.5 (band-limiting, formant silence above Nyquist, no
  filter overload, no output clip, widened pitch ranges): keep them all?
- The voice pool size an instance (the existing profile: 128 shared by every sound).
- The 10.3 Hz first-order output high-pass (section 1.5, the reference's): not built at M5, as it
  changes every sound's output (the golden hashes); in the engine, or the platform's mixer?
- The lane gate's one-voice ratio: back under its limit at M6 (2.552 against 2.559, section 3.8),
  with a margin of 0.3 % that a change of code layout alone can use up; raise the baseline, or keep
  it?
- The shell's overhead a block (about 6,300-6,800 instructions with voices, 2,100 more than the
  core at rest when awake with nothing to play): worth a pass of its own?

Provisional decisions taken at M1 (conservative defaults, each open to a later ruling):

- The voice pool is 128 voices an instance, a ceiling shared by its sounds; each sound's lanes are
  taken by use from 40 (2.8).
- The existing profile's deliberate departures from the reference are not adopted silently: where
  the reference's behaviour is known and cheap it is played (fixed frequencies held at 22.04 kHz,
  note shift +-48, fractional part velocities), otherwise the difference is listed in
  `docs/known-differences.md`.
- The pitch and level controller sensitivities have their ids, ranges and defaults in the sound
  format; their behaviour waits for a law.
- Parameter defaults are the model's initial values. Portamento was off until M3 built it; it is
  now the documented default, on (the reference also needs the performance's switch, off by
  default: docs/known-differences.md), so a sound that sets nothing glides full-time at time 64.
