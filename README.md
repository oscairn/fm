# Oscairn FM

An eight-operator FM synthesis engine for the Oscairn platform, in Rust: a platform-free DSP core
and a CLAP shell. Licence: AGPL-3.0-or-later.

**Version 0.6.4 (plugin id `org.oscairn.fm`, shown as "Oscairn FM").
The engine plays the seven operator forms (sine, all harmonics, odd harmonics, resonant, with
spread and formant) on every routing with the measured level, envelope, feedback, pitch, LFO and
filter laws, in stereo, and a CLAP plugin exists that builds and loads on macOS, Linux and
Windows. The channel controllers and the controller boxes on the operator level and frequency,
cutoff, resonance, the voice LFO's speed and depths and the pitch play. Not done yet: the other
box destinations (see below), a packaged and signed
release. "The reference" in these docs is the FM engine this project is measured
against.**

This project is independent and is not affiliated with or endorsed by any instrument
manufacturer. It contains and distributes no manufacturer samples, presets, firmware or code.
Product names, where they appear at all, belong to their owners.

What exists:

- `docs/design.md`: the model (every parameter of the reference's eight-operator FM part engine),
  the architecture, the measurements, how audio-thread safety is proven, the tests and gates,
  milestones and the provisional decisions.
- `docs/sound-format.md`, `docs/coverage.md`, `docs/known-differences.md`: the sound format, what
  the engine plays parameter by parameter (with each law's accuracy), and where it differs from
  the reference.
- `crates/fm-core`: the `no_std` core, without an allocator: the parameter table and the sound
  format (`params`, `sound`), the coverage report (`coverage`), the measured laws (`laws`), the 88
  routings (`routing`), plans (`plan`), the voice engine (`engine`: per-sound lanes of four voices, one
  lane wide while a lane holds one, timed note events, envelopes, the end rule, shedding), the
  vector type (`simd`: four lanes on NEON, SSE2 and scalar, bit-identical; the scalar back end any
  width), tables, the operator forms (`forms`: Q14 tables written by the build script
  from the measured data), and the M0 voice-kernel spike (`spike`).
- `tools/fmbench`: the engine's and the spike's cost cases, `fmbench time` (the device timing
  tool), `fmbench check`, `fmbench coverage SOUND`; `fmhost`, the plugin's cost through CLAP
  (`fmhost 3 2000 shared time`: a shared instance's block timed on the device).
- `tools/cost/cg.sh`: instructions a voice-block on aarch64 under callgrind, and data misses with
  the device's caches, gated against `tools/cost/baseline.txt` and `cache-baseline.txt`, and the
  lane gate (a voice's cost with few voices a sound).
- `tools/check-no-panic.sh`: the link-time proof that the block path, and everything the plugin's
  audio-thread callbacks run, has no panic path.
- `crates/fm-clap`: the CLAP plugin `org.oscairn.fm` (`docs/plugin.md`); `tools/load-plugin.py`
  loads a built one as a host would.

What does not exist yet: the controller box destinations without a measured law (the filter
envelope's times, an operator's pitch envelope attack and a few others, reported by `fmbench
coverage`; `docs/coverage.md`, `boxN_dst`), the key controller sets, the soft pedal and portamento switch (ignored), micro tuning, the output high-pass, the sound
LFO's tempo sync, packaging. See `docs/design.md`, section 7.

## Build and test

```sh
git submodule update --init       # third_party/oscairn-clap, the platform's bindings (pinned)
cargo test --workspace            # laws, format, engine (fuzz, native against scalar, golden render), spike
cargo test -p fm-core --features scalar
sh tools/check-no-panic.sh        # unix
cargo run --release -p fmbench -- engine 32 1000 12   # 32 voices, routing 12, eight operators
cargo run --release -p fmbench -- coverage SOUND
```

The toolchain is pinned (`rust-toolchain.toml`, Rust 1.95.0). The cost gate needs aarch64 Linux
with valgrind:

```sh
docker run --rm -v "$PWD":/src:ro -v /tmp/cg:/out IMAGE sh /src/tools/cost/cg.sh /src /out check
```
