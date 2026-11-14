// SPDX-License-Identifier: AGPL-3.0-or-later
//! The CLAP shell. Not built yet: this crate holds the plugin's identity, defined once, and the
//! plan is in docs/design.md ("The shell"). Nothing here is a loadable plugin.
#![no_std]

/// The CLAP plugin id.
pub const PLUGIN_ID: &str = "org.oscairn.fm2";
/// The name a host shows.
pub const PLUGIN_NAME: &str = "Oscairn FM (preview)";
