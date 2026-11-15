// SPDX-License-Identifier: AGPL-3.0-or-later
//! Writes the operator form tables (src/forms_build.rs) as a static for src/forms.rs.
#![allow(clippy::expect_used)] // a build script fails by panicking
use std::fmt::Write;

#[path = "src/forms_build.rs"]
mod forms_build;
#[rustfmt::skip]
#[path = "src/forms_data.rs"]
mod forms_data;
#[allow(dead_code)]
#[path = "src/turns.rs"]
mod turns;

fn main() {
    for f in ["build.rs", "src/forms_build.rs", "src/forms_data.rs", "src/turns.rs"] {
        println!("cargo:rerun-if-changed={f}");
    }
    // (a const, so it can initialise the aligned block of tables::BLOCK, its one use)
    let mut s = format!(
        "#[allow(clippy::large_const_arrays)]\npub(crate) const FORMS_DATA: [Table16; {}] = [\n",
        forms_build::COUNT
    );
    for t in forms_build::build() {
        s += "[";
        for v in t {
            write!(s, "{v},").ok();
        }
        s += "],\n";
    }
    s += "];\n";
    let out = std::path::Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("forms.rs");
    std::fs::write(out, s).expect("writing the form tables");
}
