#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Instruction cost of the voice kernel under callgrind (the cost gate; docs/design.md, "Cost").
# Runs on aarch64 Linux with cargo and valgrind, e.g. in a device-like container:
#   docker run --rm -v "$PWD":/src:ro -v /tmp/cg:/out IMAGE sh /src/tools/cost/cg.sh /src /out [check]
# Builds tools/fmbench for the device's core (Cortex-A53), counts only the rendering
# (--toggle-collect) with the cache simulator on, and prints per case: instructions a voice-block,
# data reads and writes a voice-block, and first-level data misses a block. "check" fails (exit 1)
# when a case is more than 10 % over tools/cost/baseline.txt or over the absolute gates (on the
# engine's layout, operator-major; the sample-major cases are kept as the measured alternative).
set -e
SRC=$1; OUT=$2; MODE=$3
BLOCKS=64
export CARGO_TARGET_DIR="$OUT/target" RUSTFLAGS="-C target-cpu=cortex-a53"
(cd "$SRC" && cargo build --release --locked -p fmbench -j4)
B="$OUT/target/release/fmbench"
run() {  # name voices layout routing form backend
  n=$1; v=$2; shift 2
  valgrind --tool=callgrind --cache-sim=yes --toggle-collect=fmbench_render --callgrind-out-file="$OUT/$n.cg" \
    "$B" "$v" $BLOCKS "$@" > "$OUT/$n.txt" 2> "$OUT/$n.err"
  # events: Ir Dr Dw I1mr D1mr D1mw ILmr DLmr DLmw
  awk -v n="$n" -v v="$v" -v b=$BLOCKS '/^summary:/ {
      d = (v > 0 ? v : 1) * b
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
for f in table window; do echo "footprint $f: $("$B" footprint 0 x x $f)"; done
} | tee "$OUT/summary.txt"
if [ "$MODE" = check ]; then
  fail=0
  rest=$(awk '$1=="rest" {print $5}' "$OUT/summary.txt")
  worst=$(awk '$1 ~ /^om-/ && $1 !~ /sine/ && $3 >= 32 {if ($5 > m) m = $5} END {print m}' "$OUT/summary.txt")
  if [ "$(awk -v r="$rest" -v w="$worst" 'BEGIN {print (r > 2000 || w > 28000)}')" = 1 ]; then
    echo "GATE: rest $rest (at most 2000 a block), worst non-sine voice $worst (at most 28000 a voice-block)"; fail=1
  else echo "ok gates: rest $rest, worst non-sine voice $worst"; fi
  while read -r name base; do
    case "$name" in ''|'#'*) continue;; esac
    got=$(awk -v n="$name" '$1==n {print $5}' "$OUT/summary.txt")
    if [ -z "$got" ] || [ "$(awk -v g="$got" -v b="$base" 'BEGIN {print (g > b * 1.10)}')" = 1 ]; then
      echo "REGRESSION $name: $got, baseline $base (+10 % allowed)"; fail=1
    else echo "ok $name: $got (baseline $base)"; fi
  done < "$SRC/tools/cost/baseline.txt"
  exit $fail
fi
