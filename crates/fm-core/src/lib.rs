// SPDX-License-Identifier: AGPL-3.0-or-later
//! The FM engine's DSP core: no OS, allocator, threads, files, locks or formatting, and no panic
//! path in the block path (docs/design.md, "Audio-thread safety"). The caller owns every object.
#![cfg_attr(not(test), no_std)]
// tests may index and unwrap: a panic there is a failed test
#![cfg_attr(
    test,
    allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]

pub mod coverage;
pub mod engine;
pub mod forms;
#[cfg(test)]
mod forms_build;
#[cfg(test)]
#[rustfmt::skip]
mod forms_data;
pub mod laws;
pub mod params;
pub mod plan;
pub mod routing;
pub mod simd;
pub mod sound;
pub mod spike;
pub mod tables;
mod turns;
