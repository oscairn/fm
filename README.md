# Oscairn FM

An eight-operator FM synthesis engine for the Oscairn platform, in Rust: a platform-free DSP core
and a CLAP shell. Licence: AGPL-3.0-or-later.

**State: milestone M2, "waves": the engine plays the seven operator forms (sine, all harmonics,
odd harmonics, resonant, with spread and formant) on every routing with the measured level,
envelope, feedback and pitch laws, in the core only. It does not make sound in a
host yet.**

What exists:

- `docs/design.md`: the model (every parameter of the reference's eight-operator FM part engine),
  the architecture, the measurements, how audio-thread safety is proven, the tests and gates,
  milestones and the provisional decisions.
- `docs/sound-format.md`, `docs/coverage.md`, `docs/known-differences.md`: the sound format, what
  the engine plays parameter by parameter (with each law's accuracy), and where it differs from
  the reference.
- `crates/fm-core`: the `no_std` core, without an allocator: the parameter table and the sound
  format (`params`, `sound`), the coverage report (`coverage`), the measured laws (`laws`), the 88
  routings (`routing`), plans (`plan`), the voice engine (`engine`: per-sound lanes of four voices,
  envelopes, the end rule, shedding), the four-lane vector type (`simd`: NEON, SSE2, scalar,
  bit-identical), tables, the operator forms (`forms`: Q14 tables written by the build script
  from the measured data), and the M0 voice-kernel spike (`spike`).
- `tools/fmbench`: the engine's and the spike's cost cases, `fmbench time` (the device timing
  tool), `fmbench check`, `fmbench coverage SOUND`.
- `tools/cost/cg.sh`: instructions a voice-block on aarch64 under callgrind, and data misses with
  the device's caches, gated against `tools/cost/baseline.txt` and `cache-baseline.txt`.
- `tools/check-no-panic.sh`: the link-time proof that the block path has no panic path.
- `crates/fm-clap`: the plugin's identity only. The shell is not built.

What does not exist yet: pitch envelopes and pitch modulation, LFO modulation, the
macros, level scaling, the filter, pan, the CLAP plugin. See `docs/design.md`, section 7.

## Build and test

```sh
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
