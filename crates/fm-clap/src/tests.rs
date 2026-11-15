// SPDX-License-Identifier: AGPL-3.0-or-later
//! An in-process CLAP host: the plugin through its C entry, as a host sees it. The audio thread's
//! calls run under a counting allocator: any allocation in them fails the test.

use crate::ffi::{clap_entry, this};
use crate::shell::{BUS_ID, PARAM_COUNT, PORTS};
use core::ffi::{c_char, c_void, CStr};
use fm_core::engine::{Engine, Event, Kind, Prepared, BLOCK};
use fm_core::simd::Native;
use fm_core::sound::Sound;
use oscairn_clap_sys::clap_sys::{
    audio_buffer::clap_audio_buffer,
    events::*,
    ext::{audio_ports::*, audio_ports_activation::*, note_ports::*, params::*, state::*},
    factory::plugin_factory::*,
    host::clap_host,
    plugin::clap_plugin,
    process::*,
    stream::{clap_istream, clap_ostream},
    version::CLAP_VERSION,
};
use oscairn_clap_sys::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering::Relaxed};

// ---- the audio thread's allocations ------------------------------------------------------------

struct Counting;
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
thread_local!(static IN_AUDIO: Cell<bool> = const { Cell::new(false) });

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if IN_AUDIO.with(Cell::get) {
            ALLOCATIONS.fetch_add(1, Relaxed);
        }
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Runs an audio-thread call, failing on an allocation in it.
fn audio<R>(f: impl FnOnce() -> R) -> R {
    let before = ALLOCATIONS.load(Relaxed);
    IN_AUDIO.with(|x| x.set(true));
    let r = f();
    IN_AUDIO.with(|x| x.set(false));
    assert_eq!(ALLOCATIONS.load(Relaxed), before, "the audio thread allocated");
    r
}

// ---- events ---------------------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Ev {
    Note(clap_event_note),
    Param(clap_event_param_value),
    Midi(clap_event_midi),
    Expr(clap_event_note_expression),
}

impl Ev {
    fn header(&self) -> *const clap_event_header {
        match self {
            Ev::Note(e) => &e.header,
            Ev::Param(e) => &e.header,
            Ev::Midi(e) => &e.header,
            Ev::Expr(e) => &e.header,
        }
    }
}

fn expression(t: u32, kind: i32, ch: i16, id: i32, value: f64) -> Ev {
    Ev::Expr(clap_event_note_expression {
        header: header::<clap_event_note_expression>(t, CLAP_EVENT_NOTE_EXPRESSION),
        expression_id: kind,
        note_id: id,
        port_index: 0,
        channel: ch,
        key: -1,
        value,
    })
}

fn header<T>(time: u32, kind: u16) -> clap_event_header {
    clap_event_header {
        size: size_of::<T>() as u32,
        time,
        space_id: CLAP_CORE_EVENT_SPACE_ID,
        type_: kind,
        flags: 0,
    }
}

fn note(kind: u16, t: u32, ch: i16, key: i16, id: i32, vel: f64) -> Ev {
    Ev::Note(clap_event_note {
        header: header::<clap_event_note>(t, kind),
        note_id: id,
        port_index: 0,
        channel: ch,
        key,
        velocity: vel,
    })
}
fn on(t: u32, ch: i16, key: i16, id: i32) -> Ev {
    note(CLAP_EVENT_NOTE_ON, t, ch, key, id, 100.0 / 127.0)
}
fn off(t: u32, ch: i16, key: i16, id: i32) -> Ev {
    note(CLAP_EVENT_NOTE_OFF, t, ch, key, id, 0.0)
}
fn param(t: u32, id: u32, ch: i16, value: f64) -> Ev {
    Ev::Param(clap_event_param_value {
        header: header::<clap_event_param_value>(t, CLAP_EVENT_PARAM_VALUE),
        param_id: id,
        cookie: core::ptr::null_mut(),
        note_id: -1,
        port_index: -1,
        channel: ch,
        key: -1,
        value,
    })
}
fn midi(t: u32, data: [u8; 3]) -> Ev {
    Ev::Midi(clap_event_midi {
        header: header::<clap_event_midi>(t, CLAP_EVENT_MIDI),
        port_index: 0,
        data,
    })
}

unsafe fn list<'a>(l: *const clap_input_events) -> &'a Vec<Ev> {
    unsafe { &*(*l).ctx.cast::<Vec<Ev>>() }
}
unsafe extern "C" fn in_size(l: *const clap_input_events) -> u32 {
    unsafe { list(l) }.len() as u32
}
unsafe extern "C" fn in_get(l: *const clap_input_events, i: u32) -> *const clap_event_header {
    unsafe { list(l) }.get(i as usize).map_or(core::ptr::null(), Ev::header)
}

#[derive(Default)]
struct Log {
    /// NOTE_ENDs: time, note id, channel, key.
    ends: Vec<(u32, i32, i16, i16)>,
    params: Vec<(u32, f64)>,
}

unsafe extern "C" fn out_push(l: *const clap_output_events, h: *const clap_event_header) -> bool {
    // (the host's own allocations are not the plugin's)
    let was = IN_AUDIO.with(|x| x.replace(false));
    let r = unsafe { log_event(l, h) };
    IN_AUDIO.with(|x| x.set(was));
    r
}

unsafe fn log_event(l: *const clap_output_events, h: *const clap_event_header) -> bool {
    let log = unsafe { &mut *(*l).ctx.cast::<Log>() };
    let h = unsafe { &*h };
    if h.type_ == CLAP_EVENT_NOTE_END {
        let n = unsafe { &*(h as *const clap_event_header).cast::<clap_event_note>() };
        log.ends.push((h.time, n.note_id, n.channel, n.key));
    } else if h.type_ == CLAP_EVENT_PARAM_VALUE {
        let p = unsafe { &*(h as *const clap_event_header).cast::<clap_event_param_value>() };
        log.params.push((p.param_id, p.value));
    }
    true
}

// ---- streams -------------------------------------------------------------------------------------

unsafe extern "C" fn write(s: *const clap_ostream, b: *const c_void, n: u64) -> i64 {
    let v = unsafe { &mut *(*s).ctx.cast::<Vec<u8>>() };
    let n = n.min(100); // a host that takes a little at a time
    v.extend_from_slice(unsafe { core::slice::from_raw_parts(b.cast::<u8>(), n as usize) });
    n as i64
}
unsafe extern "C" fn read(s: *const clap_istream, b: *mut c_void, n: u64) -> i64 {
    let (v, at) = unsafe { &mut *(*s).ctx.cast::<(Vec<u8>, usize)>() };
    let k = (v.len() - *at).min(n as usize).min(77);
    unsafe { core::ptr::copy_nonoverlapping(v.as_ptr().add(*at), b.cast::<u8>(), k) };
    *at += k;
    k as i64
}

// ---- the host and an instance ----------------------------------------------------------------------

unsafe extern "C" fn host_ext(_: *const clap_host, _: *const c_char) -> *const c_void {
    core::ptr::null()
}
unsafe extern "C" fn host_request(h: *const clap_host) {
    unsafe { &*(*h).host_data.cast::<AtomicU32>() }.fetch_add(1, Relaxed);
}
unsafe extern "C" fn host_nothing(_: *const clap_host) {}

struct Inst {
    p: *const clap_plugin,
    _host: Box<clap_host>,
    wakes: Box<AtomicU32>,
    steady: i64,
    /// Every port's output so far, left and right.
    out: Vec<[Vec<f32>; 2]>,
    status: clap_process_status,
    log: Log,
    ports: u16,
}

impl Inst {
    fn new() -> Inst {
        let wakes = Box::new(AtomicU32::new(0));
        let host = Box::new(clap_host {
            clap_version: CLAP_VERSION,
            host_data: (&*wakes as *const AtomicU32).cast_mut().cast(),
            name: c"test".as_ptr(),
            vendor: c"".as_ptr(),
            url: c"".as_ptr(),
            version: c"1".as_ptr(),
            get_extension: Some(host_ext),
            request_restart: Some(host_nothing),
            request_process: Some(host_request),
            request_callback: Some(host_nothing),
        });
        let p = unsafe {
            let f = clap_entry.get_factory.unwrap()(CLAP_PLUGIN_FACTORY_ID.as_ptr()).cast::<clap_plugin_factory>();
            let p = (*f).create_plugin.unwrap()(f, &*host, crate::PLUGIN_ID.as_ptr());
            assert!((*p).init.unwrap()(p));
            assert!((*p).activate.unwrap()(p, 48000.0, 1, 4096));
            assert!((*p).start_processing.unwrap()(p));
            p
        };
        Inst {
            p,
            _host: host,
            wakes,
            steady: 0,
            out: vec![[vec![], vec![]]; PORTS],
            status: 0,
            log: Log::default(),
            ports: 1,
        }
    }

    fn ext<T>(&self, id: &CStr) -> &'static T {
        let t = unsafe { (*self.p).get_extension.unwrap()(self.p, id.as_ptr()) };
        assert!(!t.is_null(), "{id:?}");
        unsafe { &*t.cast::<T>() }
    }
    fn contexts(&self) -> &'static oscairn_plugin_sound_contexts {
        self.ext(ids::OSCAIRN_EXT_SOUND_CONTEXTS)
    }
    fn snapshots(&self) -> &'static oscairn_plugin_sound_snapshots {
        self.ext(ids::OSCAIRN_EXT_SOUND_SNAPSHOTS)
    }
    fn budget(&self) -> &'static oscairn_plugin_voice_budget {
        self.ext(ids::OSCAIRN_EXT_VOICE_BUDGET)
    }

    /// One block of `frames`; the ports' buffers start filled with 123, as a host's may be.
    fn block(&mut self, frames: usize, events: Vec<Ev>) -> clap_process_status {
        self.block_at(self.steady, frames, events)
    }

    fn block_at(&mut self, steady: i64, frames: usize, events: Vec<Ev>) -> clap_process_status {
        let mut bufs: Vec<[Vec<f32>; 2]> = (0..PORTS).map(|_| [vec![123.0; frames], vec![123.0; frames]]).collect();
        let mut ptrs: Vec<[*mut f32; 2]> = bufs.iter_mut().map(|[l, r]| [l.as_mut_ptr(), r.as_mut_ptr()]).collect();
        let audio_out: Vec<clap_audio_buffer> = ptrs
            .iter_mut()
            .map(|c| clap_audio_buffer {
                data32: c.as_mut_ptr(),
                data64: core::ptr::null_mut(),
                channel_count: 2,
                latency: 0,
                constant_mask: 0,
            })
            .collect();
        let mut events = events;
        let input = clap_input_events {
            ctx: (&mut events as *mut Vec<Ev>).cast(),
            size: Some(in_size),
            get: Some(in_get),
        };
        let output = clap_output_events {
            ctx: (&mut self.log as *mut Log).cast(),
            try_push: Some(out_push),
        };
        let pr = clap_process {
            steady_time: steady,
            frames_count: frames as u32,
            transport: core::ptr::null(),
            audio_inputs: core::ptr::null(),
            audio_outputs: audio_out.as_ptr().cast_mut(),
            audio_inputs_count: 0,
            audio_outputs_count: PORTS as u32,
            in_events: &input,
            out_events: &output,
        };
        self.status = audio(|| unsafe { (*self.p).process.unwrap()(self.p, &pr) });
        // the host reads its report after every block
        let b = self.budget();
        let mut notes = [oscairn_voice_budget_note::default(); 64];
        audio(|| unsafe {
            b.active_voices.unwrap()(self.p, &mut 0.0);
            b.notes.unwrap()(self.p, notes.as_mut_ptr(), 64);
        });
        for (o, b) in self.out.iter_mut().zip(bufs) {
            for (o, b) in o.iter_mut().zip(b) {
                o.extend(b);
            }
        }
        self.steady = steady + frames as i64;
        self.status
    }

    fn set_port(&mut self, i: u32, on: bool) {
        let a: &clap_plugin_audio_ports_activation = self.ext(CLAP_EXT_AUDIO_PORTS_ACTIVATION);
        assert!(audio(|| unsafe { a.set_active.unwrap()(self.p, false, i, on, 32) }));
        self.ports = if on {
            self.ports | 1 << i
        } else {
            self.ports & !(1 << i)
        };
    }

    fn prepare(&self, name: &CStr, patch: &CStr) -> u32 {
        let mut msg = [0 as c_char; 128];
        let c = self.contexts();
        let id = unsafe {
            c.prepare.unwrap()(
                self.p,
                name.as_ptr(),
                core::ptr::null(),
                0,
                patch.as_ptr(),
                core::ptr::null(),
                msg.as_mut_ptr(),
                128,
            )
        };
        assert_ne!(id, u32::MAX, "{:?}", unsafe { CStr::from_ptr(msg.as_ptr()) });
        id
    }

    fn save(&self) -> Vec<u8> {
        let mut v = Vec::new();
        let s = clap_ostream {
            ctx: (&mut v as *mut Vec<u8>).cast(),
            write: Some(write),
        };
        let st: &clap_plugin_state = self.ext(CLAP_EXT_STATE);
        assert!(unsafe { st.save.unwrap()(self.p, &s) });
        v
    }

    fn load(&self, data: &[u8]) -> bool {
        let mut ctx = (data.to_vec(), 0usize);
        let s = clap_istream {
            ctx: (&mut ctx as *mut (Vec<u8>, usize)).cast(),
            read: Some(read),
        };
        let st: &clap_plugin_state = self.ext(CLAP_EXT_STATE);
        unsafe { st.load.unwrap()(self.p, &s) }
    }

    fn ended(&self, id: i32) -> usize {
        self.log.ends.iter().filter(|e| e.1 == id).count()
    }

    fn prepares(&self) -> u64 {
        unsafe { this(self.p) }.with_audio(|a| a.prepares).unwrap()
    }
}

impl Drop for Inst {
    fn drop(&mut self) {
        unsafe {
            (*self.p).stop_processing.unwrap()(self.p);
            (*self.p).deactivate.unwrap()(self.p);
            (*self.p).destroy.unwrap()(self.p);
        }
    }
}

fn peak(v: &[f32]) -> f32 {
    v.iter().fold(0.0, |m, x| m.max(x.abs()))
}

const LEAD: &CStr = c"fm-sound/1\nalgorithm=12 feedback=5\nop1.level=99 op2.level=80 op1.eg_time_release=60\n";

// ---- the tests -----------------------------------------------------------------------------------

#[test]
fn descriptor_and_every_required_callback() {
    unsafe {
        assert_eq!(clap_entry.clap_version.major, CLAP_VERSION.major);
        let f = clap_entry.get_factory.unwrap()(CLAP_PLUGIN_FACTORY_ID.as_ptr()).cast::<clap_plugin_factory>();
        assert_eq!((*f).get_plugin_count.unwrap()(f), 1);
        let d = &*(*f).get_plugin_descriptor.unwrap()(f, 0);
        assert_eq!(CStr::from_ptr(d.id), c"org.oscairn.fm2");
        assert_eq!(CStr::from_ptr(d.name), c"Oscairn FM (preview)");
        assert_eq!(CStr::from_ptr(d.vendor), c"Oscairn");
        assert_eq!(CStr::from_ptr(d.version), c"0.3.0");
        assert!((*f).create_plugin.unwrap()(f, core::ptr::null(), c"org.oscairn.other".as_ptr()).is_null());
    }
    let a = Inst::new();
    let p = unsafe { &*a.p };
    assert!(p.init.is_some() && p.destroy.is_some() && p.activate.is_some() && p.deactivate.is_some());
    assert!(p.start_processing.is_some() && p.stop_processing.is_some() && p.reset.is_some());
    assert!(p.process.is_some() && p.get_extension.is_some() && p.on_main_thread.is_some());
    let ports: &clap_plugin_audio_ports = a.ext(CLAP_EXT_AUDIO_PORTS);
    assert!(ports.count.is_some() && ports.get.is_some());
    let (n_in, n_out) = unsafe { (ports.count.unwrap()(a.p, true), ports.count.unwrap()(a.p, false)) };
    assert_eq!((n_in, n_out), (0, 9));
    let mut info: clap_audio_port_info = unsafe { core::mem::zeroed() };
    for i in 0..9 {
        assert!(unsafe { ports.get.unwrap()(a.p, i, false, &mut info) });
        assert_eq!(
            (info.id, info.channel_count, info.flags == CLAP_AUDIO_PORT_IS_MAIN),
            (i, 2, i == 0)
        );
    }
    let act: &clap_plugin_audio_ports_activation = a.ext(CLAP_EXT_AUDIO_PORTS_ACTIVATION);
    assert!(act.can_activate_while_processing.is_some() && act.set_active.is_some());
    let notes: &clap_plugin_note_ports = a.ext(CLAP_EXT_NOTE_PORTS);
    assert_eq!(
        unsafe { (notes.count.unwrap()(a.p, true), notes.count.unwrap()(a.p, false)) },
        (1, 0)
    );
    let params: &clap_plugin_params = a.ext(CLAP_EXT_PARAMS);
    assert!(
        params.count.is_some() && params.get_info.is_some() && params.get_value.is_some() && params.flush.is_some()
    );
    assert!(params.value_to_text.is_some() && params.text_to_value.is_some());
    // every parameter with info, a unique id, min <= default <= max; the bus role
    let n = unsafe { params.count.unwrap()(a.p) };
    assert_eq!(n as usize, PARAM_COUNT);
    let mut ids = std::collections::HashSet::new();
    let ui: &oscairn_plugin_ui = a.ext(ids::OSCAIRN_EXT_UI);
    for i in 0..n {
        let mut pi: clap_param_info = unsafe { core::mem::zeroed() };
        assert!(unsafe { params.get_info.unwrap()(a.p, i, &mut pi) });
        assert!(ids.insert(pi.id) && pi.min_value <= pi.default_value && pi.default_value <= pi.max_value);
        let mut v = f64::NAN;
        assert!(unsafe { params.get_value.unwrap()(a.p, pi.id, &mut v) } && v == pi.default_value);
        let mut h = oscairn_param_hint::default();
        assert!(unsafe { ui.param_hint.unwrap()(a.p, pi.id, &mut h) });
        assert_eq!(h.role == oscairn_role::OSCAIRN_ROLE_OUTPUT_BUS, pi.id == BUS_ID);
    }
    let mut t = [0 as c_char; 32];
    assert!(unsafe { params.value_to_text.unwrap()(a.p, BUS_ID, 3.0, t.as_mut_ptr(), 32) });
    assert_eq!(unsafe { CStr::from_ptr(t.as_ptr()) }, c"Bus 3");
    let mut v = 0.0;
    assert!(unsafe { params.text_to_value.unwrap()(a.p, 1042, c"7".as_ptr(), &mut v) } && v == 7.0);
    let st: &clap_plugin_state = a.ext(CLAP_EXT_STATE);
    assert!(st.save.is_some() && st.load.is_some());
    let c = a.contexts();
    assert!(
        c.capacity.is_some() && c.get.is_some() && c.prepare.is_some() && c.select.is_some() && c.discard.is_some()
    );
    assert_eq!(unsafe { c.capacity.unwrap()(a.p) }, 8);
    let s = a.snapshots();
    assert!(s.prepare.is_some() && s.ring_out.is_some() && s.choke_retiring.is_some());
    assert!(s.save_sound.is_some() && s.sound_values.is_some());
    let b = a.budget();
    assert!(b.honors_volume.is_some() && b.set_limits.is_some() && b.active_voices.is_some() && b.notes.is_some());
    let m: &oscairn_plugin_memory = a.ext(ids::OSCAIRN_EXT_MEMORY);
    assert!(m.estimate.is_some() && m.current.is_some());
    assert!(ui.param_hint.is_some());
}

#[test]
fn a_note_through_the_shell_is_the_cores_render() {
    let sound = Sound::from_text(LEAD.to_str().unwrap()).unwrap();
    let mut a = Inst::new();
    let text = format!(
        "fm-state 1\nselected{}\nsound 0 bus 0 Default\n{}end\n",
        " 0".repeat(16),
        LEAD.to_str().unwrap()
    );
    assert!(a.load(text.as_bytes()));
    let mut e = Engine::new(48000.0);
    e.set_sound(0, &Prepared::new(&sound));
    let mut want = [vec![], vec![]];
    for b in 0..40 {
        let ev = match b {
            1 => vec![on(5, 0, 60, 7)],
            12 => vec![off(37, 0, 60, 7)],
            _ => vec![],
        };
        a.block(BLOCK, ev);
        let core: Vec<Event> = match b {
            1 => vec![Event {
                frame: 5,
                sound: 0,
                kind: Kind::On {
                    id: 0,
                    key: 60,
                    velocity: 100.0,
                },
            }],
            12 => vec![Event {
                frame: 37,
                sound: 0,
                kind: Kind::Off { id: 0 },
            }],
            _ => vec![],
        };
        let mut o = [[0.0; BLOCK]; 2];
        e.render_events::<Native>(&mut o, &core, &mut |_| {});
        want[0].extend(o[0]);
        want[1].extend(o[1]);
    }
    assert!(peak(&want[0]) > 0.01);
    for (got, want) in a.out[0].iter().zip(&want) {
        assert!(got.iter().zip(want).all(|(x, y)| x.to_bits() == y.to_bits()));
    }
    assert!(
        a.out[1..].iter().all(|p| p[0].iter().all(|&x| x == 123.0)),
        "inactive ports untouched"
    );
}

#[test]
fn shared_sounds_play_on_their_own_bus_ports() {
    let mut a = Inst::new();
    let x = a.prepare(c"x", LEAD);
    let y = a.prepare(c"y", c"fm-sound/1\nalgorithm=1\nop1.level=99 op1.coarse=2\n");
    let c = a.contexts();
    assert!(unsafe { c.select.unwrap()(a.p, x, 1) && c.select.unwrap()(a.p, y, 2) });
    a.set_port(1, true);
    a.set_port(2, true);
    // the host's one channel-addressed bus value for each, in the first block after the selects
    a.block(
        BLOCK,
        vec![
            param(0, BUS_ID, 1, 1.0),
            param(0, BUS_ID, 2, 2.0),
            on(3, 1, 60, 1),
            on(9, 2, 64, 2),
        ],
    );
    for _ in 0..8 {
        a.block(BLOCK, vec![]);
    }
    assert!(peak(&a.out[1][0]) > 0.01 && peak(&a.out[2][0]) > 0.01);
    assert_eq!((peak(&a.out[0][0]), peak(&a.out[0][1])), (0.0, 0.0), "main is silent");
    assert!(
        a.out[3..].iter().all(|p| p[0].iter().all(|&v| v == 123.0)),
        "other ports untouched"
    );
    // each bus is its sound alone: x's port as an instance playing x on main
    let mut b = Inst::new();
    let text = format!(
        "fm-state 1\nselected{}\nsound 0 bus 0 Default\n{}end\n",
        " 0".repeat(16),
        LEAD.to_str().unwrap()
    );
    assert!(b.load(text.as_bytes()));
    b.block(BLOCK, vec![on(3, 0, 60, 1)]);
    for _ in 0..8 {
        b.block(BLOCK, vec![]);
    }
    assert!(a.out[1][0]
        .iter()
        .zip(&b.out[0][0])
        .all(|(p, q)| p.to_bits() == q.to_bits()));
    // a deactivated port's sound plays on main
    a.set_port(2, false);
    a.block(BLOCK, vec![]);
    assert!(peak(&a.out[0][0][9 * BLOCK..]) > 0.0);
    let mut info = oscairn_sound_context_info {
        id: 0,
        name: [0; 64],
        selected_channels: 0,
        active_voices: 0,
        idle: false,
    };
    assert!(
        unsafe { c.get.unwrap()(a.p, 1, &mut info) } && info.selected_channels == 1 << 1 && info.active_voices == 1
    );
}

/// Each controller reaches the sound its channel selects and no other: expression (CC 11) and a
/// note's volume expression silence what they address, while the other channel's sound plays the
/// same bits as without them; the other controllers and pressure leave a sound without routes as
/// it was.
#[test]
fn controllers_reach_their_channels_sound_only() {
    let run = |events: Vec<Ev>| {
        let mut a = Inst::new();
        let x = a.prepare(c"x", LEAD);
        let y = a.prepare(c"y", c"fm-sound/1\nalgorithm=1\nop1.level=99 op1.coarse=2\n");
        let c = a.contexts();
        assert!(unsafe { c.select.unwrap()(a.p, x, 1) && c.select.unwrap()(a.p, y, 2) });
        a.set_port(1, true);
        a.set_port(2, true);
        let first = vec![
            param(0, BUS_ID, 1, 1.0),
            param(0, BUS_ID, 2, 2.0),
            on(0, 1, 60, 1),
            on(0, 2, 64, 2),
        ];
        a.block(BLOCK, first);
        a.block(BLOCK, events);
        for _ in 0..4 {
            a.block(BLOCK, vec![]);
        }
        a
    };
    let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    let plain = run(vec![]);
    let tail = |a: &Inst, port: usize| a.out[port][0][3 * BLOCK..].to_vec();
    assert!(peak(&tail(&plain, 1)) > 0.01 && peak(&tail(&plain, 2)) > 0.01);
    for events in [
        vec![midi(0, [0xb1, 11, 0])],
        vec![expression(0, CLAP_NOTE_EXPRESSION_VOLUME, 1, 1, 0.0)],
        vec![expression(0, CLAP_NOTE_EXPRESSION_VOLUME, -1, 1, 0.0)],
    ] {
        let a = run(events);
        assert_eq!(peak(&tail(&a, 1)), 0.0, "channel 1's sound silenced");
        assert_eq!(
            bits(&a.out[2][0]),
            bits(&plain.out[2][0]),
            "channel 2's sound untouched"
        );
    }
    let others = vec![
        midi(0, [0xb1, 1, 100]),
        midi(0, [0xb1, 2, 100]),
        midi(0, [0xb1, 4, 100]),
        midi(0, [0xb1, 67, 127]),
        midi(0, [0xd1, 90, 0]),
        midi(0, [0xa1, 60, 90]),
        expression(0, CLAP_NOTE_EXPRESSION_PRESSURE, 1, -1, 0.5),
    ];
    let a = run(others);
    assert_eq!(bits(&a.out[1][0]), bits(&plain.out[1][0]));
}

#[test]
fn snapshot_save_and_prepare_round_trip() {
    let mut a = Inst::new();
    let x = a.prepare(c"lead", LEAD);
    let s = a.snapshots();
    let mut v = Vec::new();
    let out = clap_ostream {
        ctx: (&mut v as *mut Vec<u8>).cast(),
        write: Some(write),
    };
    assert!(unsafe { s.save_sound.unwrap()(a.p, x, &out) });
    let mut ctx = (v.clone(), 0usize);
    let input = clap_istream {
        ctx: (&mut ctx as *mut (Vec<u8>, usize)).cast(),
        read: Some(read),
    };
    let mut sel = [0u32; 16];
    let mut msg = [0 as c_char; 128];
    assert!(unsafe { s.prepare.unwrap()(a.p, &input, sel.as_mut_ptr(), msg.as_mut_ptr(), 128) });
    let y = sel[0];
    assert!(sel.iter().all(|&i| i == y) && y != x);
    let values = |id| {
        let mut out: Vec<_> = (0..PARAM_COUNT)
            .map(|_| oscairn_sound_parameter { id: 0, value: 0.0 })
            .collect();
        let n = unsafe { s.sound_values.unwrap()(a.p, id, out.as_mut_ptr(), PARAM_COUNT as u32) };
        assert_eq!(n as usize, PARAM_COUNT);
        out.iter().map(|p| (p.id, p.value)).collect::<Vec<_>>()
    };
    assert_eq!(values(x), values(y));
    // selected, a sound's values are not offered; its own saved text is the same
    let c = a.contexts();
    assert!(unsafe { c.select.unwrap()(a.p, y, 4) });
    a.block(BLOCK, vec![]);
    assert_eq!(unsafe { s.sound_values.unwrap()(a.p, y, core::ptr::null_mut(), 0) }, 0);
    // the default sound is never discarded, a selected one not while selected
    assert!(!unsafe { c.discard.unwrap()(a.p, 0) } && !unsafe { c.discard.unwrap()(a.p, y) });
    assert!(unsafe { c.discard.unwrap()(a.p, x) });
}

#[test]
fn state_round_trip_and_a_load_owes_its_note_ends() {
    let mut a = Inst::new();
    let x = a.prepare(c"lead", LEAD);
    let c = a.contexts();
    assert!(unsafe { c.select.unwrap()(a.p, x, 3) });
    a.block(
        BLOCK,
        vec![param(0, BUS_ID, 3, 4.0), param(0, 1042, 3, 5.0), on(0, 3, 62, 11)],
    );
    let saved = a.save();
    let b = Inst::new();
    assert!(b.load(&saved));
    assert_eq!(b.save(), saved);
    assert!(std::str::from_utf8(&saved).unwrap().contains("op1.coarse=5"));
    // a load while a note sounds: the note's end goes out in the next block, which the plugin asks for
    let wakes = a.wakes.load(Relaxed);
    assert!(a.load(&b.save()));
    assert!(a.wakes.load(Relaxed) > wakes);
    a.block(BLOCK, vec![]);
    assert_eq!(a.ended(11), 1);
    assert!(!a.load(b"fm-state 1\nselected 0\n"));
}

fn until_ended(a: &mut Inst, id: i32) {
    for _ in 0..2000 {
        if a.ended(id) > 0 {
            break;
        }
        a.block(BLOCK, vec![]);
    }
    a.block(BLOCK, vec![]);
}

#[test]
fn sleeps_after_the_release_ends() {
    let mut a = Inst::new();
    assert_eq!(a.block(BLOCK, vec![on(0, 0, 60, 1)]), CLAP_PROCESS_CONTINUE);
    assert_eq!(a.block(BLOCK, vec![off(0, 0, 60, 1)]), CLAP_PROCESS_CONTINUE);
    until_ended(&mut a, 1);
    assert_eq!(a.ended(1), 1);
    let from = a.out[0][0].len();
    for _ in 0..4 {
        assert_eq!(a.block(BLOCK, vec![]), CLAP_PROCESS_SLEEP);
    }
    assert_eq!(
        (peak(&a.out[0][0][from..]), peak(&a.out[0][1][from..])),
        (0.0, 0.0),
        "the 123s were cleared"
    );
}

#[test]
fn a_note_after_sleep_is_the_same_note() {
    // a: the idle blocks are called and sleep; b: the host skips them (its steady time counts on)
    let (mut a, mut b) = (Inst::new(), Inst::new());
    for _ in 0..10 {
        assert_eq!(a.block(BLOCK, vec![]), CLAP_PROCESS_SLEEP);
    }
    b.steady = 10 * BLOCK as i64;
    let from = a.out[0][0].len();
    for x in [&mut a, &mut b] {
        x.block(BLOCK, vec![on(5, 0, 60, 1)]);
        x.block(BLOCK, vec![off(0, 0, 60, 1)]);
        for _ in 0..40 {
            x.block(BLOCK, vec![]);
        }
    }
    assert!(peak(&a.out[0][0][from..]) > 0.01);
    for c in 0..2 {
        assert!(a.out[0][c][from..]
            .iter()
            .zip(&b.out[0][c])
            .all(|(p, q)| p.to_bits() == q.to_bits()));
    }
}

#[test]
fn a_selection_on_a_sleeping_plugin_is_applied_by_the_next_block() {
    let mut a = Inst::new();
    assert_eq!(a.block(BLOCK, vec![]), CLAP_PROCESS_SLEEP);
    let x = a.prepare(c"x", LEAD);
    let c = a.contexts();
    assert!(unsafe { c.select.unwrap()(a.p, x, 3) });
    // the host calls after an accepted select: it applies, nothing is left queued, nothing renders
    assert_eq!(a.block(BLOCK, vec![]), CLAP_PROCESS_SLEEP);
    let mut info = oscairn_sound_context_info {
        id: 0,
        name: [0; 64],
        selected_channels: 0,
        active_voices: 0,
        idle: true,
    };
    assert!(unsafe { c.get.unwrap()(a.p, 1, &mut info) } && info.selected_channels == 1 << 3 && !info.idle);
}

#[test]
fn a_state_load_asks_for_a_block() {
    let mut a = Inst::new();
    a.block(BLOCK, vec![]);
    let wakes = a.wakes.load(Relaxed);
    assert!(a.load(&a.save()));
    assert!(a.wakes.load(Relaxed) > wakes);
}

#[test]
fn a_lowered_voice_limit_gives_voices_up_in_order() {
    let mut a = Inst::new();
    let keys = [48, 52, 55, 60, 64, 67, 72, 76];
    a.block(BLOCK, keys.iter().map(|&k| on(0, 0, k, i32::from(k))).collect());
    // the two lowest keys released: they go first
    a.block(BLOCK, vec![off(0, 0, 48, 48), off(0, 0, 52, 52)]);
    let b = a.budget();
    let all = oscairn_voice_budget_context {
        context: OSCAIRN_VOICE_BUDGET_SELECTED,
        limit: 0,
        reserve: 0,
        priority: 0,
        flags: 0,
    };
    audio(|| unsafe { b.set_limits.unwrap()(a.p, 4, &all, 1) });
    for _ in 0..4 {
        a.block(BLOCK, vec![]);
    }
    let mut cost = 0.0;
    assert_eq!(audio(|| unsafe { b.active_voices.unwrap()(a.p, &mut cost) }), 4);
    assert!(cost > 0.0);
    assert_eq!((a.ended(48), a.ended(52)), (1, 1), "released first");
    let mut notes = [oscairn_voice_budget_note::default(); 16];
    let n = audio(|| unsafe { b.notes.unwrap()(a.p, notes.as_mut_ptr(), 16) });
    assert_eq!(n, 4);
    assert!(notes[..4].iter().all(|n| n.loudness > 0.0 && n.voices == 1));
    assert_eq!(a.log.ends.len(), 4);
}

#[test]
fn the_selected_sounds_limit_reaches_a_sound_selected_after_it() {
    let mut a = Inst::new();
    let b = a.budget();
    let selected = oscairn_voice_budget_context {
        context: OSCAIRN_VOICE_BUDGET_SELECTED,
        limit: 2,
        reserve: 0,
        priority: 0,
        flags: 0,
    };
    audio(|| unsafe { b.set_limits.unwrap()(a.p, 16, &selected, 1) });
    // (a note on channel 0, so the block runs and the limits are made before the selection)
    a.block(BLOCK, vec![on(0, 0, 60, 60)]);
    let x = a.prepare(c"x", LEAD);
    assert!(unsafe { a.contexts().select.unwrap()(a.p, x, 1) });
    a.block(BLOCK, [48, 52, 55].iter().map(|&k| on(0, 1, k, i32::from(k))).collect());
    for _ in 0..4 {
        a.block(BLOCK, vec![]);
    }
    // two of the new sound's three and channel 0's note
    assert_eq!(audio(|| unsafe { b.active_voices.unwrap()(a.p, &mut 0.0) }), 3);
}

#[test]
fn a_blocks_parameter_events_prepare_a_sound_once() {
    let mut a = Inst::new();
    a.block(BLOCK, vec![on(0, 0, 60, 1)]);
    let before = a.prepares();
    a.block(BLOCK, (0..100).map(|i| param(i, 1043, 0, f64::from(i % 50))).collect());
    assert_eq!(a.prepares() - before, 1);
    let mut v = 0.0;
    let params: &clap_plugin_params = a.ext(CLAP_EXT_PARAMS);
    assert!(
        unsafe { params.get_value.unwrap()(a.p, 1043, &mut v) } && v == 49.0,
        "the last value"
    );
    // a longer block prepares once a stretch of 128 frames
    a.block(
        4 * BLOCK,
        (0..4 * BLOCK as u32)
            .map(|i| param(i, 1043, 0, f64::from(i % 20)))
            .collect(),
    );
    assert_eq!(a.prepares() - before, 5);
}

#[test]
fn midi_pedal_bend_and_all_notes_off() {
    let mut a = Inst::new();
    // MIDI note on, the pedal down, the key up: the note is held
    a.block(
        BLOCK,
        vec![
            midi(0, [0x90, 60, 100]),
            midi(1, [0xb0, 64, 127]),
            midi(2, [0x80, 60, 0]),
            midi(3, [0xe0, 0, 80]),
        ],
    );
    for _ in 0..20 {
        a.block(BLOCK, vec![]);
    }
    assert_eq!(a.ended(-1), 0, "the pedal holds it");
    // CC 120 cuts every note of the channel (a short fade)
    a.block(BLOCK, vec![midi(0, [0x90, 64, 90]), midi(1, [0xb0, 120, 0])]);
    a.block(BLOCK, vec![]);
    assert_eq!(a.ended(-1), 2);
    // a NOTE_CHOKE by id; a note on a key out of range ends at once
    a.block(
        BLOCK,
        vec![
            on(0, 0, 60, 5),
            note(CLAP_EVENT_NOTE_CHOKE, 4, -1, -1, 5, 0.0),
            on(5, 0, 200, 6),
        ],
    );
    a.block(BLOCK, vec![]);
    assert_eq!((a.ended(5), a.ended(6)), (1, 1));
}

#[test]
fn reset_ends_everything_silently() {
    let mut a = Inst::new();
    a.block(BLOCK, vec![on(0, 0, 60, 1), on(0, 0, 64, 2)]);
    audio(|| unsafe { (*a.p).reset.unwrap()(a.p) });
    assert_eq!(a.block(BLOCK, vec![]), CLAP_PROCESS_SLEEP);
    assert_eq!(a.log.ends.len(), 0);
    a.block(BLOCK, vec![on(0, 0, 60, 3)]);
    assert!(peak(&a.out[0][0][2 * BLOCK..]) > 0.0);
}
