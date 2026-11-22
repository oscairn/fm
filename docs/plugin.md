# The plugin

`crates/fm-clap` builds the CLAP plugin `org.oscairn.fm`, "Oscairn FM", vendor Oscairn,
version 0.6.5 (the crate's version, which the descriptor reads), AGPL-3.0-or-later. It is a
`cdylib` (`libfm_clap.so`, `.dylib`, `fm_clap.dll`) with one entry, `clap_entry`; packaging it as a
`.clap` bundle, signing and publishing are not done here.

## Bindings

CLAP's own types are `clap-sys` 0.5.0 (MIT or Apache-2.0, raw FFI only); the platform's extensions
are its pre-generated bindings, `oscairn-clap-sys`, from `third_party/oscairn-clap` (a submodule
pinned at c2119dbe992ce7551f49fb5efa49fe48e93cfe34, MIT), which are built on `clap-sys`'s types, so
the shell has one set of CLAP types and no casts. No framework: the shell is the callbacks in
`src/ffi.rs` over the instance in `src/shell.rs`.

## What it offers

| Extension | What |
|---|---|
| `clap.audio-ports` | no input; nine stereo outputs, ids 0 (main) and 1-8 (bus n) |
| `clap.audio-ports-activation` (and its draft-2 id) | active while processing; a bus plays only once activated, else its sounds play on main; main always plays |
| `clap.note-ports` | one input, CLAP notes (preferred) and MIDI |
| `clap.params` | every sound parameter and the output bus (below) |
| `clap.state` | the whole instance (below) |
| `oscairn.sound-contexts/draft-5` | 8 sounds; `prepare` from the sound text form and parameter values; `select`, `discard`, `get`; its `state` is the same text as `clap.state`; no `prepare_preset` |
| `oscairn.sound-snapshots/draft-4` | `prepare` (all or none), `ring_out` and `choke_retiring`, `save_sound` and `sound_values` |
| `oscairn.voice-budget/draft-2` (and draft-1, the same table) | limits applied, voices given up in the extension's order, cost and notes reported, each sound's least important voices listed (`importance`); `honors_volume` false |
| `oscairn.memory/draft` | the instance's bytes (one allocation; the tables are shared read-only data) |
| `oscairn.ui/draft-6` (and drafts 5 and 4) | `param_hint` for every parameter; the output bus has the output-bus role, the others none yet |

## Parameters

| CLAP id | Parameter |
|---|---|
| the sound format's id (0-40, 100-134, 200-266, 300-555, 1000 + 100 op + k) | that sound parameter, raw units, its range and default (`docs/sound-format.md`) |
| 10000 | output bus, 0 (main) to 8 |

Ids never change: a new sound parameter takes the sound format's new id; shell parameters take ids
from 10000 up. Every parameter is automatable and stepped; none is modulatable (`PARAM_MOD` is not
taken). Names are the text form's (`algorithm`, `op3.level`), modules `Common`, `Filter`, `Part`,
`Controllers` (ids 256-555), `Operator n`, `Output`.

The controller boxes' sources that a host cannot send as MIDI are sound parameters, named as the
sibling engines name theirs so that a host and a converter treat the engines alike:

| CLAP id | name | range (default) | box source |
|---|---|---|---|
| 256-263 | `knob1`..`knob8` | 0-1023 (512) | assignable knobs 1-8 (sources 8-15): the raw value, 1023 as 1024 |
| 264 | `ribbon` | 0-127 (0) | the ribbon (source 6), as its controller |
| 265, 266 | `switch1`, `switch2` | 0-1 (0) | assignable switches 1 and 2 (sources 16, 17): off 0, on 1024 |

Being sound parameters they are saved with the sound and, like every parameter, take effect at the
start of the stretch after their event (a preparation of the sound, below); the boxes themselves
(ids 300-555) are the sound's (`docs/coverage.md`, `boxN_*`). Value text is the integer (the bus: `Main`,
`Bus n`), and text back is the integer.

A `CLAP_EVENT_PARAM_VALUE` acts on the sound its `channel` selects, or with channel -1 on every
sound a channel selects. `get_value` shows the sound channel 0 selects; after a selection changes
channel 0's sound, the values that differ go out as `PARAM_VALUE` events. A cutoff break point that
would leave the four out of ascending order is ignored (a state could not hold it).

Coalescing: parameter values are staged in the core as they arrive and each sound changed is
prepared once at the start of the next stretch it renders (a stretch is at most 128 frames: a block
of up to 128 frames is one), so a block's parameter events cost one preparation per sound changed
(about 17,200 instructions each), not one per event. They take effect at that stretch's start, not
at their frame. A value the sound already has prepares nothing, and a box's source (a knob, the
ribbon, a switch) makes again only what the boxes do (when nothing else of the sound waits), so a
host that sends its controls' values again each block costs almost nothing.

## Notes and controllers

CLAP notes: note on (velocity times 127, at least 1), note off (by note id, channel and key, -1 any),
note choke (a 3 ms fade). A note that cannot start (a port other than 0, a key out of range, the
instance's 320 tracked notes all in use) ends at once. Every note gets one `NOTE_END`, stamped at the
end of the stretch in which it ended; the ends a state load owes go out at the next block's start
(the plugin asks for that block). CLAP `reset` ends everything silently.

MIDI (port 0): note on and off (a note off releases the oldest held note of its key and channel),
pitch bend, CC 64 (the sustain pedal of the sound the channel selects), CC 120 (every note of the
channel choked), CC 123 (every held note of the channel released, and what the pedal holds). The
part's receive settings are the host's: as in the sibling engines, the plugin has none.

Controllers, each a timed event of the sound the channel selects, from the next control step at its
frame: CC 1 (mod wheel, box source 1), CC 2 (breath, 7), CC 4 (foot controller 2, 4), CC 11 (foot
controller 1, source 3, and expression: the sound's output times (v / 127)^2, ramped over a step),
CC 66 (the sostenuto pedal: holds the notes whose keys are down when it goes down), channel
pressure, key pressure and the CLAP pressure expression (aftertouch, source 2, the latest wins for
the whole sound), the CLAP volume expression (the addressed notes' output, linear, ramped over a
step). CC 65 (portamento) and CC 67 (the soft pedal, which the reference does not receive) are
ignored; the tuning expression is not played.

Tempo: the host's transport tempo, read at each block's start when the transport has one (120
quarter notes a minute until then), times a key-on delay with tempo sync (`key_on_delay_sync`): the
note waits its note length at the tempo of its note-on.

## Sound contexts and sharing

Slot 0 is "Default", selected by every channel at first, never discarded. `prepare` builds a sound
on the main thread from the text form (`fm-sound/1 ...`) and parameter values, into a free slot's
buffer; the audio thread installs it when it applies its first selection. Selections and
`choke_retiring` are queued (32) and applied at the start of the next `process`, before its events;
the bus value the host then sends on the part's channel lands the sound on its port. A sound
selected by no channel plays its main output into the ring-out buffer when one was given for that
block (`ring_out`); after `choke_retiring` its notes fade (3 ms) into the second pair.

## State

Text, deterministic and the same on every architecture; at most 1 MiB:

```
fm-state 1
selected 0 0 0 3 0 0 0 0 0 0 0 0 0 0 0 0
sound 0 bus 0 Default
fm-sound/1
end
sound 3 bus 2 a name
fm-sound/1
algorithm=12 feedback=5
op1.level=80
end
```

The second line is the sound each MIDI channel selects; then each prepared sound: its slot, output
bus and name (up to 63 bytes, the rest of the line), its sound in the core's text form, and `end`.
Slot 0 is always present. Everything malformed is refused whole. A later version adds lines, never
changes one. `save_sound` writes one sound alone in the same form (slot 0, main bus, every channel).
A load ends the notes sounding (their `NOTE_END`s go out in the next block) and asks for that block.

## Threads

The audio thread never allocates, locks, logs or waits. Selections pass through a lock-free queue;
a prepared sound is written by the main thread only into a free slot and read by the audio thread
only after the selection that pins it; sound values, buses, channel masks, pins and the port
activation are atomics. Activation and state loads take the audio thread's data with a flag pair:
the audio thread, finding the main thread holding it, skips that callback (a block of silence)
instead of waiting. The shell's tests run the audio thread's calls under a counting allocator.
`tools/check-no-panic.sh` walks the plugin's release build from every audio-thread callback along
its direct calls and fails if a panic function is reachable.

## Sleep

`process` returns `CLAP_PROCESS_SLEEP` when no note or voice is left, no `NOTE_END` is owed and no
selection is queued; a block with no event in that state clears the active outputs and returns at
once. The engine's clock (the glide clock's phase) moves on over blocks the host skipped, from
`steady_time`; with a `steady_time` of -1 it does not count skipped blocks (blocks it is called for
still count). Nothing else runs free while nothing sounds: the sound LFO moves only while its sound
plays, in the core as in the shell.

## Voice budget

`set_limits`: the instance's limit and, for each sound, the entry naming its id, else (selected)
the entry for the selected sounds: its limit, reserve and priority; past them the core gives voices
up at the next block's start in the extension's order, each with a 3 ms fade. `active_voices`
reports the voices sounding and one voice's cost a block: the running mean of the sounding voices'
plan costs (instructions a 128-frame block, the plans' cost table), scaled to the block size and
stated in microseconds at 1,000 instructions a microsecond: the device timed the engine at 11-17 µs
a voice-block for plans of 14,000-20,000 instructions at 32 voices, 0.8-1.0 µs a thousand, and the
slow end is taken so that the cost is not understated.
`notes` reports each sounding note's address and loudness (its carriers' gain bound times its fade).

`importance` (draft 2) lists, for each sound with a voice sounding, under its context's id, its
voices sounding and its 16 least important, least important first. The unit is a voice: a note
plays one (there is no unison), so a voice is what the limits and `active_voices` count, each of
weight 1 (a context's weight its voice count). A voice's level is its carriers' gain bound (their
envelope levels with the velocity and output levels in them) times its fade, its note's volume and
the sound's expression, in dBFS (-120 to +24), with no pass over its samples; its tier is released
(or below -60 dBFS unless struck under 50 ms ago), held only by a pedal (sustain or sostenuto), or
key held; its age is from its note-on at the end of the block just rendered. The order is the
SDK helper's (`voice-budget-plugin.hpp`), the core's for every voice it gives up: by tier, then the
new note's own sound, then the lower priority, then a voice struck under 50 ms ago last, then the
least audible (its level less a dB a second of age, at most ten), then the oldest (a note-on
serial). A voice the order never gives up (a held voice of a mono sound, or of a sound at or under
its reserve) is not listed; past its reserve a sound lists only its held voices beyond it. The
lists are kept: past a limit at a block's start, and when it makes room for a new note at a limit,
the core gives up the voices the last list named first (none before a first report), each sound's in its listed order and across sounds the first of each by tier, then priority,
each only while its lane still holds the same note (its serial: a lane moves its voices into its
first slots), then by the order.

## Not covered

The tuning expression and per-note bend; CC 65 (portamento switch); parameter modulation
(`PARAM_MOD`) and modulatable box destinations; the controller boxes' destinations without a law
(`docs/coverage.md`); the sound LFO's tempo sync; a tempo change within a block (the transport's
tempo is read at the block's start); micro tuning; the output
high-pass; preset discovery and `prepare_preset`; the plugin's window; UI roles beyond the output bus,
envelope views and the FM algorithm view; per-channel bend of notes left on a sound no longer
selected (bend and pedal go to the channel's selected sound); packaging, signing.
