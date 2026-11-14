// SPDX-License-Identifier: AGPL-3.0-or-later
//! What the engine plays of the model: for each parameter, played, or the milestone that brings
//! it. `docs/coverage.md` is the same table with each law's accuracy, and a test ties the two.
//! A sound's report lists its parameters at non-default values that the engine does not play.

use crate::params::Param;
use crate::sound::Sound;

/// Played, or why not (the milestone that brings it).
pub const PLAYED: &str = "played";

/// The coverage of one parameter.
pub fn status(p: &Param) -> &'static str {
    let n = p.name;
    let op = p.op.is_some();
    let played_op = [
        "key_on_reset",
        "mode",
        "coarse",
        "fine",
        "detune",
        "eg_level_attack",
        "eg_level_decay1",
        "eg_level_decay2",
        "eg_level_release",
        "eg_time_attack",
        "eg_time_decay1",
        "eg_time_decay2",
        "eg_time_release",
        "eg_time_hold",
        "eg_time_key_follow",
        "level",
        "level_velocity",
    ];
    let played_part = [
        "algorithm",
        "feedback",
        "note_shift",
        "velocity_depth",
        "velocity_offset",
    ];
    let starts = |prefixes: &[&str]| prefixes.iter().any(|x| n.starts_with(x));
    if (op && played_op.contains(&n)) || (!op && played_part.contains(&n)) {
        PLAYED
    } else if starts(&["ctrl_set", "key_ctrl_set", "tuning_"]) || n.ends_with("_ctrl_sens") {
        "M7 (controllers, tuning)"
    } else if op && starts(&["form", "spread", "formant"]) {
        "M2 (waves)"
    } else if starts(&[
        "pitch_",
        "random_pitch",
        "portamento",
        "bend_",
        "mono_poly",
        "key_assign",
        "fixed_key",
    ]) || (!op && n == "detune")
    {
        "M3 (pitch)"
    } else if starts(&[
        "filter_",
        "cutoff",
        "resonance",
        "hpf_",
        "distance",
        "random_pan",
        "alternate_pan",
    ]) || starts(&["scaling_pan", "key_on_delay"])
    {
        "M5 (filter and output)"
    } else {
        // the LFOs, macros, level scaling, part envelope offsets
        "M4 (modulators)"
    }
}

/// Calls `f` with each parameter of `s` at a non-default value that the engine does not play,
/// its value and why.
pub fn not_covered(s: &Sound, mut f: impl FnMut(&'static Param, i16, &'static str)) {
    for (p, v) in s.changed() {
        let why = status(p);
        if why != PLAYED {
            f(p, v, why);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::PARAMS;

    /// docs/coverage.md has one row per parameter (operator parameters once, as `opN.name`) whose
    /// third column is this module's status.
    #[test]
    fn the_coverage_table_matches_the_code() {
        let doc = include_str!("../../../docs/coverage.md");
        let rows: Vec<Vec<&str>> = doc
            .lines()
            .filter(|l| l.starts_with("| `"))
            .map(|l| l.trim_matches('|').split(" | ").map(str::trim).collect())
            .collect();
        let mut seen = std::collections::HashSet::new();
        for p in PARAMS.iter() {
            let name = if p.op.is_some() {
                format!("`opN.{}`", p.name)
            } else {
                format!("`{}`", p.name)
            };
            if !seen.insert(name.clone()) {
                continue;
            }
            let row = rows
                .iter()
                .find(|r| r[0] == name)
                .unwrap_or_else(|| panic!("{name} missing from docs/coverage.md"));
            assert_eq!(row[2], status(p), "{name}");
        }
        assert_eq!(
            rows.len(),
            seen.len(),
            "docs/coverage.md has rows for parameters that do not exist"
        );
    }

    #[test]
    fn report() {
        let s = Sound::from_text("fm-sound/1\nalgorithm=5 random_pan=3\nop2.level=50 op2.form=3\n").unwrap();
        let mut got = Vec::new();
        not_covered(&s, |p, v, why| got.push((p.id, v, why)));
        assert_eq!(got, vec![(0, 3, "M5 (filter and output)"), (1147, 3, "M2 (waves)")]);
        let mut n = 0;
        not_covered(&Sound::default(), |_, _, _| n += 1);
        assert_eq!(n, 0);
    }
}
