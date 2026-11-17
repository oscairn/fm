// SPDX-License-Identifier: AGPL-3.0-or-later
//! The CLAP shell of the FM engine: the plugin `org.oscairn.fm` (docs/plugin.md). The audio
//! thread never allocates, locks, logs or waits; `tools/check-no-panic.sh` proves at link time
//! that nothing its callbacks run can panic.
// tests may index and unwrap: a panic there is a failed test
#![cfg_attr(
    test,
    allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]

mod ffi;
pub mod shell;
pub mod state;
#[cfg(test)]
mod tests;

pub use ffi::{clap_entry, PLUGIN_ID, PLUGIN_NAME};
