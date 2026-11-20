# SPDX-License-Identifier: AGPL-3.0-or-later
# Reads "caller callee" lines (every direct call and tail call of the plugin's release build) and
# fails (exit 1) when a panic function is reachable from an audio-thread callback of the shell
# (tools/check-no-panic.sh). Prints how many functions the callbacks reach.
{ e[$1] = e[$1] " " $2; fn[$1] = 1; fn[$2] = 1 }
END {
    roots = "fm_clap3ffi(7process|5reset|17budget_set_limits|20budget_active_voices|12budget_notes|17budget_importance|17snapshot_ring_out|10set_active|12params_flush)17h"
    for (f in fn) if (f ~ roots) { q[++n] = f; seen[f] = 1 }
    if (n == 0) { print "FAIL: no audio-thread callback found"; exit 1 }
    for (i = 1; i <= n; i++) {
        split(e[q[i]], t, " ")
        for (k in t) if (!(t[k] in seen)) { seen[t[k]] = 1; from[t[k]] = q[i]; q[++n] = t[k] }
    }
    bad = 0
    for (f in seen)
        if (f ~ /panicking|panic_|unwrap_failed|expect_failed|slice_index|slice_end_index|slice_start_index|_overflow|bounds_check/) {
            bad = 1; print "FAIL: " f " reachable, from " from[f]
        }
    if (want != "") { found = 0; for (f in seen) if (f ~ want) found = 1; if (!found) { print "FAIL: " want " not reached"; bad = 1 } }
    print n " functions reached from the audio-thread callbacks"
    exit bad
}
