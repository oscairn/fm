#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Panic-freedom of the core's block path (docs/design.md, "Audio-thread safety"):
#  1. no panic, bounds-check or unwrap machinery in the release static library's symbol table;
#  2. a build whose panic handler calls a non-existent symbol still links into a program, because
#     dead-code removal leaves no path to the handler. Release profile: overflow checks are on, so an
#     unproven integer overflow is a panic path too.
set -e
cd "$(dirname "$0")/.."
T=${TMPDIR:-/tmp}/fm-nopanic.$$; mkdir -p "$T"; trap 'rm -rf "$T"' EXIT
cargo build --release --locked -p fm-nopanic --target-dir "$T/a"
if nm "$T/a/release/libfm_nopanic.a" 2>/dev/null | grep -E ' [TtWw] .*(panicking|panic_fmt|unwrap_failed|expect_failed|slice_index|bounds_check|slice_end_index|_overflow)'; then
    echo "FAIL: panic machinery defined in the archive"; exit 1
fi
echo "ok: no panic symbols defined in libfm_nopanic.a"
cargo build --release --locked -p fm-nopanic --features link-fail-on-panic --target-dir "$T/b"
case $(uname) in Darwin) gc=-Wl,-dead_strip;; *) gc="-Wl,--gc-sections -ffunction-sections";; esac
cat > "$T/m.c" <<'EOC'
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
void fm_nopanic_play(uint32_t pick, uint32_t centre, size_t voice, float hz, uint32_t blocks, float *out);
int main(int argc, char **argv) {
    static float out[128];
    uint32_t a = argc > 1 ? (uint32_t)atoi(argv[1]) : 0;
    for (uint32_t pick = 0; pick < 32; pick++)
        fm_nopanic_play(pick + a, 7 + a, (size_t)argc * 5, 440.0f * (float)argc, 4, out);
    printf("%g\n", out[0]);
    return 0;
}
EOC
${CC:-cc} -O1 $gc "$T/m.c" "$T/b/release/libfm_nopanic.a" -o "$T/m"
"$T/m"
echo "ok: program links against the link-fail-on-panic build, so no panic path is reachable"
