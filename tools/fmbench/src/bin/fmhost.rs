// SPDX-License-Identifier: AGPL-3.0-or-later
//! fmhost: the plugin's cost through CLAP, as a host calls it (`fmhost VOICES BLOCKS CASE`): the
//! plugin made by its entry, a state with the case's sounds loaded, notes started in a first block
//! that is not counted, then `fmbench_render_hosted` (counted under callgrind) a block each.
//! CASE: `sleep` (no note, no event: the asleep call), `idle` (no note, one ignored controller a
//! block, CC 65: a call that wakes and renders nothing), `voices` (VOICES held notes of one sound),
//! `sounds` (one note in each of VOICES sounds, each on its own channel); fmbench's routing 12 case
//! with eight sine operators.
#[path = "../case.rs"]
mod case;

use case::case_text;
use fm_clap::clap_entry;
use oscairn_clap_sys::clap_sys::{
    audio_buffer::clap_audio_buffer, events::*, ext::state::*, factory::plugin_factory::*, host::clap_host,
    plugin::clap_plugin, process::*, stream::clap_istream, version::CLAP_VERSION,
};
use std::ffi::{c_char, c_void};

#[derive(Clone, Copy)]
enum Ev {
    Note(clap_event_note),
    Midi(clap_event_midi),
}

unsafe extern "C" fn size(l: *const clap_input_events) -> u32 {
    unsafe { (*(*l).ctx.cast::<Vec<Ev>>()).len() as u32 }
}
unsafe extern "C" fn get(l: *const clap_input_events, i: u32) -> *const clap_event_header {
    let v = unsafe { &*(*l).ctx.cast::<Vec<Ev>>() };
    match v.get(i as usize) {
        Some(Ev::Note(n)) => &n.header,
        Some(Ev::Midi(m)) => &m.header,
        None => std::ptr::null(),
    }
}
unsafe extern "C" fn push(_: *const clap_output_events, _: *const clap_event_header) -> bool {
    true
}
unsafe extern "C" fn read(s: *const clap_istream, b: *mut c_void, n: u64) -> i64 {
    let (v, at) = unsafe { &mut *(*s).ctx.cast::<(Vec<u8>, usize)>() };
    let k = (v.len() - *at).min(n as usize);
    unsafe { std::ptr::copy_nonoverlapping(v.as_ptr().add(*at), b.cast::<u8>(), k) };
    *at += k;
    k as i64
}
unsafe extern "C" fn no_ext(_: *const clap_host, _: *const c_char) -> *const c_void {
    std::ptr::null()
}
unsafe extern "C" fn nothing(_: *const clap_host) {}

fn header<T>(kind: u16) -> clap_event_header {
    clap_event_header {
        size: size_of::<T>() as u32,
        time: 0,
        space_id: CLAP_CORE_EVENT_SPACE_ID,
        type_: kind,
        flags: 0,
    }
}

/// One block through the plugin's `process`, counted under callgrind.
#[no_mangle]
#[inline(never)]
pub extern "C" fn fmbench_render_hosted(p: &clap_plugin, pr: &clap_process) -> i32 {
    unsafe { p.process.expect("process")(p, pr) }
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let num = |i: usize, d: usize| a.get(i).and_then(|x| x.parse().ok()).unwrap_or(d);
    let case = a.get(2).map_or("voices", String::as_str);
    println!("hosted {case}: output sum {:.3}", run(num(0, 8), num(1, 100), case));
}

fn run(voices: usize, blocks: usize, case: &str) -> f64 {
    let host = clap_host {
        clap_version: CLAP_VERSION,
        host_data: std::ptr::null_mut(),
        name: c"fmbench".as_ptr(),
        vendor: c"".as_ptr(),
        url: c"".as_ptr(),
        version: c"1".as_ptr(),
        get_extension: Some(no_ext),
        request_restart: Some(nothing),
        request_process: Some(nothing),
        request_callback: Some(nothing),
    };
    let sounds = if case == "sounds" { voices.clamp(1, 8) } else { 1 };
    let mut state = String::from("fm-state 1\nselected");
    for c in 0..16 {
        state += &format!(" {}", if c < sounds { c } else { 0 });
    }
    state += "\n";
    for s in 0..sounds {
        state += &format!("sound {s} bus 0 case\n{}end\n", case_text(12, 8, "sine"));
    }
    let p = unsafe {
        let f = clap_entry.get_factory.expect("entry")(CLAP_PLUGIN_FACTORY_ID.as_ptr()).cast::<clap_plugin_factory>();
        let p = (*f).create_plugin.expect("factory")(f, &host, fm_clap::PLUGIN_ID.as_ptr());
        assert!((*p).init.expect("init")(p) && (*p).activate.expect("activate")(p, 48000.0, 1, 4096));
        let st = (*p).get_extension.expect("ext")(p, CLAP_EXT_STATE.as_ptr()).cast::<clap_plugin_state>();
        let mut ctx = (state.into_bytes(), 0usize);
        let s = clap_istream {
            ctx: (&mut ctx as *mut (Vec<u8>, usize)).cast(),
            read: Some(read),
        };
        assert!((*st).load.expect("load")(p, &s));
        assert!((*p).start_processing.expect("start")(p));
        p
    };
    let mut bufs = vec![[[0.0f32; fm_core::engine::BLOCK]; 2]; 9];
    let mut ptrs: Vec<[*mut f32; 2]> = bufs.iter_mut().map(|[l, r]| [l.as_mut_ptr(), r.as_mut_ptr()]).collect();
    let outs: Vec<clap_audio_buffer> = ptrs
        .iter_mut()
        .map(|c| clap_audio_buffer {
            data32: c.as_mut_ptr(),
            data64: std::ptr::null_mut(),
            channel_count: 2,
            latency: 0,
            constant_mask: 0,
        })
        .collect();
    let notes: Vec<Ev> = match case {
        "voices" => (0..voices).map(|v| (0, 36 + (v % 60) as i16)).collect(),
        "sounds" => (0..sounds).map(|s| (s as i16, 36 + (7 * s % 60) as i16)).collect(),
        _ => vec![],
    }
    .into_iter()
    .map(|(channel, key)| {
        Ev::Note(clap_event_note {
            header: header::<clap_event_note>(CLAP_EVENT_NOTE_ON),
            note_id: -1,
            port_index: 0,
            channel,
            key,
            velocity: 100.0 / 127.0,
        })
    })
    .collect();
    let cc = vec![Ev::Midi(clap_event_midi {
        header: header::<clap_event_midi>(CLAP_EVENT_MIDI),
        port_index: 0,
        data: [0xb0, 65, 64],
    })];
    let mut sum = 0.0f64;
    for b in 0..=blocks {
        let mut events = match (b, case) {
            (0, _) => notes.clone(),
            (_, "idle") => cc.clone(),
            _ => vec![],
        };
        let input = clap_input_events {
            ctx: (&mut events as *mut Vec<Ev>).cast(),
            size: Some(size),
            get: Some(get),
        };
        let output = clap_output_events {
            ctx: std::ptr::null_mut(),
            try_push: Some(push),
        };
        let pr = clap_process {
            steady_time: (b * fm_core::engine::BLOCK) as i64,
            frames_count: fm_core::engine::BLOCK as u32,
            transport: std::ptr::null(),
            audio_inputs: std::ptr::null(),
            audio_outputs: outs.as_ptr().cast_mut(),
            audio_inputs_count: 0,
            audio_outputs_count: 9,
            in_events: &input,
            out_events: &output,
        };
        // the first block starts the notes and is not counted
        if b == 0 {
            unsafe { (*p).process.expect("process")(p, &pr) };
        } else {
            fmbench_render_hosted(unsafe { &*p }, &pr);
        }
        sum += unsafe { std::slice::from_raw_parts(ptrs[0][0], fm_core::engine::BLOCK) }
            .iter()
            .map(|x| f64::from(x.abs()))
            .sum::<f64>();
    }
    unsafe {
        (*p).stop_processing.expect("stop")(p);
        (*p).deactivate.expect("deactivate")(p);
        (*p).destroy.expect("destroy")(p);
    }
    sum
}
