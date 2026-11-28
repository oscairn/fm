# Saved test states

Saved states of Oscairn FM (plugin id `org.oscairn.fm`), one folder per published version, each
written by that version's own published macOS arm64 build. `cargo test` (the CI test step) loads
every file with the current code (`saved_states_of_every_covered_version_open` in
`crates/fm-clap/src/tests.rs`): it must open, read back the parameters listed in that test, be
written back as the same text, and render as those parameters set on a fresh instance do. The
packages repository's state check (`oscairn-plugin-check --state-from`) can use the same files.

## Versions covered

0.6.0 to 0.6.6, every 0.6.x published in the packages repository. Versions before 0.6.0 were a
different plugin under another id and are not covered. The saved text (`fm-state 1`, with
`fm-sound/1` sounds) has not changed since 0.6.0: the state and sound code is the same in all of
them, so each version's files are identical text. They are kept per version anyway, so a later
change to the format shows as a failing version.

## The files

- `default.state`: a new instance, nothing changed.
- `lead.state`: algorithm 12, feedback 5, operator 1 coarse 5 and level 99, operator 2 level 80.
- `mixed.state`: algorithm 40, feedback 3, LFO wave 2 and pitch depth 40, mono, operator 1 level 90,
  operator 3 fine 50 and level 70.

The release publishes no sounds of its own (its library has no items), so there is no published
init sound beyond the defaults.

## How they were made

For each version: take the release's `.olib` from the packages repository, check it against the
`sha256` in the version's record, and unzip it. Then, with the host's `oscairn-plugin-check`:

    oscairn-plugin-check <that version's macos-arm64 .clap> org.oscairn.fm synth \
      [--param ID=VALUE ...] --keep-archive saved.zip

and take `Performances/*/nodes/node-1/state/state.clap` from `saved.zip` as the `.state` file.
The `--param` ids (parameter ids from `crates/fm-core/src/params.rs`):

- `lead`: `30=12 31=5 1042=5 1064=99 1164=80`
- `mixed`: `30=40 31=3 22=2 27=40 200=0 1243=50 1264=70 1064=90`
