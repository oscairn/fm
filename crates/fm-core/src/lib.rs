// SPDX-License-Identifier: AGPL-3.0-or-later
//! The FM engine's DSP core: no OS, allocator, threads, files, locks or formatting, and no panic
//! path in the block path (docs/design.md, "Audio-thread safety"). The caller owns every object.
#![cfg_attr(not(test), no_std)]

pub mod simd;
pub mod spike;
pub mod tables;
