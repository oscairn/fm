#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Instruction cost of the voice kernel under callgrind (the cost gate; docs/design.md, "Cost").
# Runs on aarch64 Linux with cargo and valgrind, e.g. in a device-like container:
#   docker run --rm -v "$PWD":/src:ro -v /tmp/cg:/out IMAGE sh /src/tools/cost/cg.sh /src /out [check]
# Builds tools/fmbench for the device's core (Cortex-A53), counts only the rendering
# (--toggle-collect) with the cache simulator on, and prints per case: instructions a voice-block,
# data reads and writes a voice-block, and first-level data misses a block; then, with the cache
# simulator set to the device (32 KB 4-way first level, 512 KB 16-way shared second level, 64-byte
# lines), first-level and last-level data misses a voice-block of the engine at 1, 8, 25 and 64
# voices. "check" fails (exit 1) when a case is more than 10 % over tools/cost/baseline.txt (cache
# misses: tools/cost/cache-baseline.txt, 10 % and half a miss) or over the absolute gates: at most
# 2,000 a block at rest, at most 28,000 a voice-block for the worst plan with all eight operators
# (the engine's cases, fm-*-8, and the spike's operator-major cases on its non-sine forms; the
# spike's sample-major cases are kept as the measured alternative); and the lane gate: a voice's
# cost with few voices a sound against its cost at 32 voices of one sound, each ratio at most 2 %
# over its "ratio-" line in tools/cost/baseline.txt (docs/design.md, "Lanes": the targets).
set -e
SRC=$1; OUT=$2; MODE=$3
BLOCKS=64
export CARGO_TARGET_DIR="$OUT/target" RUSTFLAGS="-C target-cpu=cortex-a53"
(cd "$SRC" && cargo build --release --locked -p fmbench -j4)
B="$OUT/target/release/fmbench"
# callgrind on fmbench with an empty environment, from the binary's own directory: the stack's
# place, and with it the cache misses, then does not depend on the caller's environment or paths
vg() { (cd "$OUT/target/release" && env -i valgrind --tool=callgrind --cache-sim=yes --toggle-collect='fmbench_render*' "$@"); }
run() {  # name voices then fmbench's arguments after VOICES BLOCKS (the engine's: e VOICES BLOCKS ...)
  n=$1; v=$2; shift 2
  if [ "$1" = e ]; then shift; set -- engine "$v" $BLOCKS "$@"; else set -- "$v" $BLOCKS "$@"; fi
  vg --callgrind-out-file="$OUT/$n.cg" ./fmbench "$@" > "$OUT/$n.txt" 2> "$OUT/$n.err"
  # events: Ir Dr Dw I1mr D1mr D1mw ILmr DLmr DLmw
  awk -v n="$n" -v v="$v" -v t="${TOTAL:-0}" -v b=$BLOCKS '/^summary:/ {
      d = (t > 0 ? t : (v > 0 ? v : 1)) * b
      printf "%-22s voices %2d  Ir/voice-block %8.0f  Drw/voice-block %7.0f  D1miss/block %6.0f\n", n, v, $2 / d, ($3 + $4) / d, ($6 + $7) / b }' "$OUT/$n.cg"
  rm -f "$OUT/$n.cg"
}
{
run rest 0 om edges table
for f in table window sine; do
  run "om-edges-$f" 32 om edges $f
  run "sm-edges-$f" 32 sm edges $f
done
run om-edges-table-8 8 om edges table
run om-chain-table 32 om chain table
run om-loop-table 32 om loop table
run om-loop-window 32 om loop window
run sm-loop-table 32 sm loop table
run sm-loop-window 32 sm loop window
run scalar-om-edges-table 32 om edges table scalar
run fm-rest 0 e 12 8
for r in 12 14 80 8 1; do run "fm-r$r-8" 32 e $r 8; done
run fm-r12-8-v8 8 e 12 8
run fm-r1-1 32 e 1 1
run fm-r12-8-scalar 32 e 12 8 scalar
for f in all odd res mixed; do run "fm-r12-8-$f" 32 e 12 8 native $f; done
run fm-r80-8-res 32 e 80 8 native res
run fm-r1-8-res 32 e 1 8 native res
run fm-r12-8-mixed-v8 8 e 12 8 native mixed
# both pitch envelopes moving and portamento gliding on every voice
run fm-r12-8-pitch 32 e 12 8 native pitch
run fm-s1-v1-pitch 1 e 12 8 native pitch 1
# the voice LFO on pitch and amplitude and level scaling on every operator of every voice
run fm-r12-8-mod 32 e 12 8 native mod
run fm-s1-v1-mod 1 e 12 8 native mod 1
# the sound LFO on every operator's frequency, level and detune
run fm-r12-8-slfo 32 e 12 8 native slfo
run fm-s1-v1-slfo 1 e 12 8 native slfo 1
# a parameter event every block (the sound prepared again) on one voice: less fm-s1-v1, an event's cost
run fm-s1-v1-param 1 e 12 8 native sine 1 param
# voices few a sound: 1 and 2 voices in each of 1, 5 and 8 sounds (a voice-block: by all the voices)
for s in 1 5 8; do for v in 1 2; do TOTAL=$((s * v)); run "fm-s$s-v$v" "$v" e 12 8 native sine $s; done; done; TOTAL=
run fm-s1-v5 5 e 12 8 native sine 1
for f in table window; do echo "footprint $f: $("$B" footprint 0 x x $f)"; done
} | tee "$OUT/summary.txt"
cache() {  # name voices forms: the device's caches, routing 12 with eight operators
  vg --D1=32768,4,64 --LL=524288,16,64 --callgrind-out-file="$OUT/c$1.cg" \
    ./fmbench engine "$2" $BLOCKS 12 8 native "$3" > /dev/null 2> "$OUT/c$1.err"
  awk -v n="$1" -v v="$2" -v b=$BLOCKS '/^summary:/ { d = v * b
      printf "%-18s voices %2d  D1miss/voice-block %7.1f  LLmiss/voice-block %6.1f\n", n, v, ($6 + $7) / d, ($9 + $10) / d }' "$OUT/c$1.cg"
  rm -f "$OUT/c$1.cg"
}
# sines, and eight operators on eight different form tables and pitches (the worst case for locality)
{ for v in 1 8 25 64; do cache "cache-$v" $v sine; done
  for v in 1 8 25 64; do cache "cache-mixed-$v" $v mixed; done; } | tee "$OUT/cache.txt"
if [ "$MODE" = check ]; then
  fail=0
  rest=$(awk '$1=="rest" || $1=="fm-rest" {if ($5 > m) m = $5} END {print m}' "$OUT/summary.txt")
  worst=$(awk '(($1 ~ /^om-/ && $1 !~ /sine/) || $1 ~ /^fm-r.*-8/) && $3 >= 8 {if ($5 > m) m = $5} END {print m}' "$OUT/summary.txt")
  if [ "$(awk -v r="$rest" -v w="$worst" 'BEGIN {print (r > 2000 || w > 28000)}')" = 1 ]; then
    echo "GATE: rest $rest (at most 2000 a block), worst eight-operator voice $worst (at most 28000 a voice-block)"; fail=1
  else echo "ok gates: rest $rest, worst eight-operator voice $worst"; fi
  # the lane gate: one voice alone and two voices against 32, one voice in each of five sounds
  # against five in one
  ir() { awk -v n="$1" '$1==n {print $5}' "$OUT/summary.txt"; }
  for r in "s1-v1 $(ir fm-s1-v1) $(ir fm-r12-8)" "s1-v2 $(ir fm-s1-v2) $(ir fm-r12-8)" "s5-v1 $(ir fm-s5-v1) $(ir fm-s1-v5)"; do
    set -- $r
    base=$(awk -v n="ratio-$1" '$1==n {print $2}' "$SRC/tools/cost/baseline.txt")
    got=$(awk -v a="$2" -v b="$3" 'BEGIN {printf "%.3f", a / b}')
    if [ -z "$base" ] || [ "$(awk -v g="$got" -v b="$base" 'BEGIN {print (g > b * 1.02)}')" = 1 ]; then
      echo "LANE GATE ratio-$1: $got, baseline $base (+2 % allowed)"; fail=1
    else echo "ok ratio-$1: $got (baseline $base)"; fi
  done
  while read -r name base; do
    case "$name" in ''|'#'*|ratio-*) continue;; esac
    got=$(awk -v n="$name" '$1==n {print $5}' "$OUT/summary.txt")
    if [ -z "$got" ] || [ "$(awk -v g="$got" -v b="$base" 'BEGIN {print (g > b * 1.10)}')" = 1 ]; then
      echo "REGRESSION $name: $got, baseline $base (+10 % allowed)"; fail=1
    else echo "ok $name: $got (baseline $base)"; fi
  done < "$SRC/tools/cost/baseline.txt"
  while read -r name d1 ll; do
    case "$name" in ''|'#'*) continue;; esac
    got=$(awk -v n="$name" '$1==n {print $5, $7}' "$OUT/cache.txt")
    if [ -z "$got" ] || [ "$(echo "$got" | awk -v a="$d1" -v b="$ll" '{print ($1 > a * 1.1 + 0.5 || $2 > b * 1.1 + 0.5)}')" = 1 ]; then
      echo "REGRESSION $name: D1 and LL misses $got, baseline $d1 $ll"; fail=1
    else echo "ok $name: D1 and LL misses $got (baseline $d1 $ll)"; fi
  done < "$SRC/tools/cost/cache-baseline.txt"
  exit $fail
fi
