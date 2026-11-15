// SPDX-License-Identifier: AGPL-3.0-or-later
//! Every entry point of the core's block path behind one C function whose arguments the optimiser
//! cannot see. With `link-fail-on-panic` the panic handler calls a symbol that does not exist, so a
//! program linked against this library with dead-code removal fails to link if any panic path
//! survives optimisation (tools/check-no-panic.sh).
#![no_std]

use fm_core::engine::{Engine, Event, Kind, Limits, Prepared, SOUNDS};
use fm_core::simd::Native;
use fm_core::sound::Sound;
use fm_core::spike::{self, Form, Layout, Routing, BLOCK};

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
    let mut e = spike::Engine::new(spike::Sound::test(routing, form), layout);
    e.note_on(voice, hz, 48000.0);
    for b in 0..blocks {
        if b == blocks / 2 {
            e.note_off(voice);
        }
        e.render::<Native>(out);
    }
}

/// Plays the engine: a sound on routing and feedback chosen by `pick`, every operator at full
/// level on a form chosen by `pick`, six notes from `key`, a voice limit, the pedal, a release and a choke halfway; `blocks`
/// blocks into `out`. Returns the notes that ended.
#[no_mangle]
pub extern "C" fn fm_nopanic_engine(pick: u32, key: u8, velocity: f32, blocks: u32, out: &mut [f32; BLOCK]) -> u32 {
    let mut s = Sound::default();
    let _ = s.set(30, (pick % 88 + 1) as i16);
    let _ = s.set(31, (pick / 88 % 8) as i16);
    for op in 0..8 {
        let _ = s.set(1064 + 100 * op, 99);
        // every form, spread and formant over the picks
        let _ = s.set(1047 + 100 * op, ((pick / 3).wrapping_add(u32::from(op)) % 7) as i16);
        let _ = s.set(1048 + 100 * op, ((pick / 5).wrapping_add(u32::from(op)) % 8) as i16);
        let _ = s.set(
            1049 + 100 * op,
            (pick.wrapping_mul(7).wrapping_add(u32::from(op) * 13) % 100) as i16,
        );
        // operator pitch envelopes over the picks
        let _ = s.set(
            1050 + 100 * op,
            ((pick / 13).wrapping_add(u32::from(op)) % 101) as i16 - 50,
        );
        let _ = s.set(1052 + 100 * op, ((pick / 17).wrapping_add(u32::from(op)) % 100) as i16);
    }
    // the common pitch envelope, key follow, random pitch, the part's detune
    for (id, v) in [
        (10, (pick % 101) as i16 - 50),
        (11, (pick / 7 % 101) as i16 - 50),
        (15, (pick % 100) as i16),
        (16, (pick / 3 % 100) as i16),
        (20, (pick % 4) as i16),
        (7, (pick % 128) as i16),
        (8, (pick / 11 % 128) as i16),
        (208, (pick % 256) as i16 - 128),
        (206, (pick % 73) as i16 - 48),
        (207, (pick / 5 % 73) as i16 - 48),
    ] {
        let _ = s.set(id, v);
    }
    let mut e = Engine::new(48000.0);
    e.set_sound(0, &Prepared::new(&s));
    e.set_limits((pick % 7) as usize, &[Limits::default(); SOUNDS]);
    let mut ended = 0u32;
    let mut end = |_| ended = ended.wrapping_add(1);
    for v in 0..6u8 {
        e.note_on(
            usize::from(v % 2),
            u32::from(v),
            key.wrapping_add(v),
            velocity,
            &mut end,
        );
    }
    for b in 0..blocks {
        if b == blocks / 2 {
            e.pedal(0, pick & 1 != 0);
            e.note_off(0, 2);
            e.choke(1);
        }
        // timed events at frames and for sounds the optimiser cannot see (some out of range)
        let at = |k: u32| (pick.rotate_left(k) % 160) as usize;
        let sound = (pick >> 8) as u8 % 10;
        let events = [
            Event {
                frame: at(3),
                sound,
                kind: Kind::On {
                    id: b,
                    key: key.wrapping_add(b as u8),
                    velocity,
                },
            },
            Event {
                frame: at(9),
                sound,
                kind: Kind::Pedal { down: pick & 2 != 0 },
            },
            Event {
                frame: at(17),
                sound,
                kind: Kind::Off { id: b.wrapping_sub(1) },
            },
            Event {
                frame: at(23),
                sound,
                kind: Kind::Bend {
                    value: pick.rotate_left(b) as i16,
                },
            },
        ];
        e.render_events::<Native>(out, &events, &mut end);
    }
    ended
}
