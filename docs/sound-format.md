# Sound format v1

A sound is the model's raw parameter values, all of them (docs/design.md, section 1), each by a
stable numeric id. `crates/fm-core/src/params.rs` is the table: id, name, operator, range and
default of every parameter; `crates/fm-core/src/sound.rs` writes, reads and validates both forms.
Neither form changes when the engine learns to play a parameter: what it does not play yet is the
coverage report's business (`docs/coverage.md`).

## Ids

| ids | what |
|---|---|
| 0-40 | part common: pan, key-on delay, pitch, the common pitch envelope, the voice LFO, routing, feedback, macros |
| 100-134 | the filter and the filter envelope |
| 200-255 | part parameters that act on the engine: mono, portamento, bend, detune, note shift, tuning, velocity, the sound LFO, part offsets |
| 256-266 | the controller boxes' own sources: assignable knobs 1-8 (raw 0-1023), the ribbon (0-127), assignable switches 1 and 2 |
| 300-555 | controller boxes 1-32, eight fields each at 300 + 8 (n - 1): source, destination (the reference's number, 0 off), ratio, polarity, curve bank, curve type, curve parameters 1 and 2 |
| 1000 + 100 op + k | operator `op` (0-7): k 0-38 its switches (32 controller sets, 4 key-controller sets, 3 sound-LFO destinations), k 40-77 its parameters |

Values are the model's raw values, signed where the model shows a signed value (detune -15 to
+15, not 0 to 30). The table only grows: a new parameter takes a new id, and its default is the
behaviour sounds had before it existed; nothing is renumbered or reused.

## Blob (little-endian)

| offset | field |
|---|---|
| 0 | `OFMS` |
| 4 | u16 version (1), u16 flags (0) |
| 8 | u32 size of the whole blob |
| 12 | u32 CRC-32 (IEEE) of bytes 16 to the end |
| 16 | u16 count, u16 0 |
| 20 | count x (u16 id, i16 value): ids strictly increasing, only values that differ from the default |

The default sound is the bare 20-byte header.

## Text form

```
fm-sound/1
# a comment
algorithm=12 feedback=5 velocity_depth=90
op1.coarse=2 op1.eg_time_decay1=40 op1.level=99
op8.detune=-7 op8.level=70
```

The first line is `fm-sound/1`; then `name=value` items separated by spaces or new lines, operator
parameters as `op1.name` to `op8.name` (operators from 1 here, from 0 in the ids); a `#` line is a
comment. Written out, the part-wide values are one line and each operator's its own, in id order.
Errors name their line.

## Validation

Both forms refuse: a wrong magic, version, flags or reserved field, a size that disagrees with
the blob, a CRC mismatch, ids not strictly increasing or given twice, an unknown id or name, a
value outside its parameter's range, and cutoff scaling break points that are not strictly
ascending. Every single-byte corruption and every truncation of a test blob is refused (the
`sound` tests).
