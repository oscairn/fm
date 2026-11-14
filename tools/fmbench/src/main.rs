// SPDX-License-Identifier: AGPL-3.0-or-later
//! fmbench VOICES BLOCKS [om|sm] [edges|loop|chain] [sine|table|window] [native|scalar]
//!
//! Starts VOICES held notes and renders BLOCKS 128-frame blocks at 48 kHz. Prints the time a
//! voice-block and a checksum. Under callgrind, `--toggle-collect=fmbench_render` counts only the
//! rendering (tools/cost/cg.sh); `fmbench footprint` prints the bytes a lane touches a block.
use fm_core::simd::{scalar, Native, F4};
use fm_core::spike::{footprint, Engine, Form, Layout, Routing, Sound, BLOCK};
use std::time::Instant;

#[no_mangle]
#[inline(never)]
pub extern "C" fn fmbench_render(e: &mut Engine, out: &mut [f32; BLOCK], scalar: bool) {
    if scalar {
        e.render::<scalar::F>(out)
    } else {
        e.render::<Native>(out)
    }
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let arg = |i: usize, d: &str| a.get(i).cloned().unwrap_or_else(|| d.to_string());
    let form = match arg(4, "table").as_str() {
        "sine" => Form::Sine,
        "window" => Form::Window { centre: 7 },
        _ => Form::Table,
    };
    if arg(0, "") == "footprint" {
        let f = footprint(form);
        println!(
            "lane state {} B, scratch {} B, tables {} B, per voice {} B",
            f.lane,
            f.scratch,
            f.tables,
            f.lane / 4
        );
        return;
    }
    let voices: usize = arg(0, "8").parse().unwrap_or(8);
    let blocks: usize = arg(1, "100").parse().unwrap_or(100);
    let layout = if arg(2, "om") == "sm" {
        Layout::SampleMajor
    } else {
        Layout::OperatorMajor
    };
    let routing = match arg(3, "edges").as_str() {
        "loop" => Routing::LOOP,
        "chain" => Routing::CHAIN,
        _ => Routing::MOST_EDGES,
    };
    let scalar = arg(5, "native") == "scalar";
    let mut sound = Sound::test(routing, form);
    sound.release = [0.0; 8]; // held for the whole run
    let mut e = Box::new(Engine::new(sound, layout));
    for v in 0..voices {
        e.note_on(v, 60.0 + 23.0 * v as f32, 48000.0);
    }
    let mut out = [0.0f32; BLOCK];
    let t = Instant::now();
    let mut sum = 0.0f64;
    for _ in 0..blocks {
        out.fill(0.0);
        fmbench_render(&mut e, &mut out, scalar);
        sum += out.iter().map(|x| f64::from(x.abs())).sum::<f64>();
    }
    let us = t.elapsed().as_secs_f64() * 1e6 / (blocks * voices.max(1)) as f64;
    println!(
        "voices {voices} blocks {blocks} {:?} {} {:?} {}: {us:.2} us a voice-block, checksum {sum:.6}",
        layout,
        arg(3, "edges"),
        form,
        if scalar { "scalar" } else { "native" }
    );
    let _ = Native::splat(0.0); // the native type is linked whatever the arguments
}
