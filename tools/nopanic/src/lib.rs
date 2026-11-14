// SPDX-License-Identifier: AGPL-3.0-or-later
//! Every entry point of the core's block path behind one C function whose arguments the optimiser
//! cannot see. With `link-fail-on-panic` the panic handler calls a symbol that does not exist, so a
//! program linked against this library with dead-code removal fails to link if any panic path
//! survives optimisation (tools/check-no-panic.sh).
#![no_std]

use fm_core::simd::Native;
use fm_core::spike::{Engine, Form, Layout, Routing, Sound, BLOCK};

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    #[cfg(feature = "link-fail-on-panic")]
    extern "C" {
        fn fm_panic_reachable() -> !;
    }
    #[cfg(not(feature = "link-fail-on-panic"))]
    extern "C" {
        fn abort() -> !;
    }
    // SAFETY: both take no arguments and never return; `abort` is the C library's.
    unsafe {
        #[cfg(feature = "link-fail-on-panic")]
        fm_panic_reachable();
        #[cfg(not(feature = "link-fail-on-panic"))]
        abort()
    }
}

/// Plays `voice` at `hz` with routing, form and layout chosen by `pick`'s low bits; renders `blocks`
/// blocks, releasing the note halfway, into `out` (128 floats, added to).
#[no_mangle]
pub extern "C" fn fm_nopanic_play(pick: u32, centre: u32, voice: usize, hz: f32, blocks: u32, out: &mut [f32; BLOCK]) {
    let routing = match pick % 3 {
        0 => Routing::MOST_EDGES,
        1 => Routing::LOOP,
        _ => Routing::CHAIN,
    };
    let form = match (pick / 3) % 3 {
        0 => Form::Sine,
        1 => Form::Table,
        _ => Form::Window { centre },
    };
    let layout = if pick & 16 != 0 {
        Layout::SampleMajor
    } else {
        Layout::OperatorMajor
    };
    let mut e = Engine::new(Sound::test(routing, form), layout);
    e.note_on(voice, hz, 48000.0);
    for b in 0..blocks {
        if b == blocks / 2 {
            e.note_off(voice);
        }
        e.render::<Native>(out);
    }
}
