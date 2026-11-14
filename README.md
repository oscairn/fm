# Oscairn FM

An eight-operator FM synthesis engine for the Oscairn platform, in Rust: a platform-free DSP core
and a CLAP shell. Licence: AGPL-3.0-or-later.

**State: design and a measured spike. It does not make sound in a host yet.**

What exists:

- `docs/design.md`: the model (every parameter of the reference's eight-operator FM part engine,
  with the state of the project's earlier implementation for each), the architecture, the spike's
  measurements, how audio-thread safety is proven, the test and gate plan, milestones.
- `crates/fm-core`: the `no_std` core, without an allocator. The four-lane vector type
  (`simd`: NEON, SSE2, scalar, bit-identical to each other), compile-time tables, and the voice-kernel
  spike (`spike`: eight operators, envelopes at control rate, one feedback path, four voices a lane).
- `tools/fmbench`: renders N voices of the spike (time a voice-block; the callgrind driver).
- `tools/cost/cg.sh`: instructions a voice-block on aarch64 under callgrind, gated against
  `tools/cost/baseline.txt`.
- `tools/check-no-panic.sh`: the link-time proof that the block path has no panic path.
- `crates/fm-clap`: the plugin's identity only. The shell is not built.

What does not exist yet: the engine's laws, the operator waves of the reference, pitch and LFO
modulation, the filter, the sound format, the CLAP plugin. See `docs/design.md`, section 7.

## Build and test

```sh
cargo test --workspace            # native back end against scalar, the golden render, the spike's checks
cargo test -p fm-core --features scalar
sh tools/check-no-panic.sh        # unix
cargo run --release -p fmbench -- 32 1000 om edges window
```

The toolchain is pinned (`rust-toolchain.toml`, Rust 1.95.0). The cost gate needs aarch64 Linux
with valgrind:

```sh
docker run --rm -v "$PWD":/src:ro -v /tmp/cg:/out IMAGE sh /src/tools/cost/cg.sh /src /out check
```
