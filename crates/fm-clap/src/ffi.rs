// SPDX-License-Identifier: AGPL-3.0-or-later
//! The C side: the entry, the factory, the plugin's callbacks and every extension's table, each a
//! thin call into [`crate::shell`], and the host's events, buffers and streams read and written.
//! The CLAP types are clap-sys 0.5.0's, the platform's extensions those of its pre-generated
//! bindings (`oscairn-clap-sys`, built on the same clap-sys types: no casts between them).

use crate::shell::{Audio, Plugin, BUS_ID, PARAM_COUNT, PORTS};
use crate::state;
use core::ffi::{c_char, c_void, CStr};
use core::sync::atomic::Ordering::Relaxed;
use fm_core::engine::{Written, BLOCK, SOUNDS};
use fm_core::params::{index_of, PARAMS};
use fm_core::sound::Sound;
use oscairn_clap_sys::clap_sys::{
    audio_buffer::clap_audio_buffer,
    entry::clap_plugin_entry,
    events::*,
    ext::{audio_ports::*, audio_ports_activation::*, note_ports::*, params::*, state::*},
    factory::plugin_factory::*,
    host::clap_host,
    id::{clap_id, CLAP_INVALID_ID},
    plugin::{clap_plugin, clap_plugin_descriptor},
    plugin_features::*,
    process::*,
    stream::{clap_istream, clap_ostream},
    version::CLAP_VERSION,
};
use oscairn_clap_sys::*;

pub const PLUGIN_ID: &CStr = c"org.oscairn.fm2";
pub const PLUGIN_NAME: &CStr = c"Oscairn FM (preview)";
const VERSION: &CStr = match CStr::from_bytes_with_nul(concat!(env!("CARGO_PKG_VERSION"), "\0").as_bytes()) {
    Ok(v) => v,
    Err(_) => c"0",
};

struct Features([*const c_char; 4]);
// SAFETY: pointers to static strings
unsafe impl Sync for Features {}
static FEATURES: Features = Features([
    CLAP_PLUGIN_FEATURE_INSTRUMENT.as_ptr(),
    CLAP_PLUGIN_FEATURE_SYNTHESIZER.as_ptr(),
    CLAP_PLUGIN_FEATURE_STEREO.as_ptr(),
    core::ptr::null(),
]);

pub static DESCRIPTOR: clap_plugin_descriptor = clap_plugin_descriptor {
    clap_version: CLAP_VERSION,
    id: PLUGIN_ID.as_ptr(),
    name: PLUGIN_NAME.as_ptr(),
    vendor: c"Oscairn".as_ptr(),
    url: c"https://github.com/oscairn/fm".as_ptr(),
    manual_url: c"".as_ptr(),
    support_url: c"".as_ptr(),
    version: VERSION.as_ptr(),
    description: c"FM instrument".as_ptr(),
    features: FEATURES.0.as_ptr(),
};

/// The instance behind a plugin pointer, with the plugin struct the host holds.
#[repr(C)]
struct Instance {
    clap: clap_plugin,
    plugin: Box<Plugin>,
}

/// # Safety
/// `p` is a plugin this factory made and not yet destroyed.
pub(crate) unsafe fn this<'a>(p: *const clap_plugin) -> &'a Plugin {
    unsafe { &(*p.cast::<Instance>()).plugin }
}

/// Copies `s` (cut to fit) into a C string field.
fn copy(s: &[u8], to: &mut [c_char]) {
    let n = s.len().min(to.len().saturating_sub(1));
    for (d, &b) in to.iter_mut().zip(s.iter().take(n)) {
        *d = b as c_char;
    }
    if let Some(z) = to.get_mut(n) {
        *z = 0;
    }
}

/// A C string argument, if valid UTF-8.
///
/// # Safety
/// `s` is null or a NUL-terminated string.
unsafe fn text<'a>(s: *const c_char) -> Option<&'a str> {
    (!s.is_null())
        .then(|| unsafe { CStr::from_ptr(s) }.to_str().ok())
        .flatten()
}

fn fail(message: *mut c_char, capacity: u32, why: &str) {
    if !message.is_null() && capacity > 0 {
        // SAFETY: the host gave `capacity` bytes
        copy(why.as_bytes(), unsafe {
            core::slice::from_raw_parts_mut(message, capacity as usize)
        });
    }
}

// ---- the host's events and buffers -----------------------------------------------------------------

pub fn events_in(pr: &clap_process) -> Option<&clap_input_events> {
    unsafe { pr.in_events.as_ref() }
}

pub fn count(e: &clap_input_events) -> usize {
    e.size.map_or(0, |f| unsafe { f(e) }) as usize
}

pub fn events(e: &clap_input_events) -> impl Iterator<Item = &clap_event_header> {
    let n = e.size.map_or(0, |f| unsafe { f(e) });
    (0..n).filter_map(move |i| event(e, i as usize))
}

pub fn event(e: &clap_input_events, i: usize) -> Option<&clap_event_header> {
    let n = e.size.map_or(0, |f| unsafe { f(e) });
    let i = u32::try_from(i).ok().filter(|&i| i < n)?;
    e.get.and_then(|f| unsafe { f(e, i).as_ref() })
}

fn is<T>(h: &clap_event_header, kind: u16) -> Option<&T> {
    (h.space_id == CLAP_CORE_EVENT_SPACE_ID && h.type_ == kind && h.size as usize >= core::mem::size_of::<T>())
        // SAFETY: the host's event of that type and size
        .then(|| unsafe { &*(h as *const clap_event_header).cast::<T>() })
}

pub fn param_value(h: &clap_event_header) -> Option<&clap_event_param_value> {
    is(h, CLAP_EVENT_PARAM_VALUE)
}

pub struct NoteRef {
    pub id: i32,
    pub port_index: i16,
    pub channel: i16,
    pub key: i16,
    pub velocity: f64,
}

pub fn note(h: &clap_event_header) -> Option<(u16, NoteRef)> {
    [CLAP_EVENT_NOTE_ON, CLAP_EVENT_NOTE_OFF, CLAP_EVENT_NOTE_CHOKE]
        .into_iter()
        .find_map(|k| {
            let n: &clap_event_note = is(h, k)?;
            Some((
                k,
                NoteRef {
                    id: n.note_id,
                    port_index: n.port_index,
                    channel: n.channel,
                    key: n.key,
                    velocity: n.velocity,
                },
            ))
        })
}

/// A note expression on port 0: its kind, note id, channel, key and value.
pub fn expression(h: &clap_event_header) -> Option<(i32, i32, i16, i16, f64)> {
    is::<clap_event_note_expression>(h, CLAP_EVENT_NOTE_EXPRESSION)
        .filter(|e| e.port_index <= 0)
        .map(|e| (e.expression_id, e.note_id, e.channel, e.key, e.value))
}

pub fn midi(h: &clap_event_header) -> Option<[u8; 3]> {
    is::<clap_event_midi>(h, CLAP_EVENT_MIDI)
        .filter(|m| m.port_index == 0)
        .map(|m| m.data)
}

/// The output event queue.
pub struct Out(*const clap_output_events);

pub fn out(pr: &clap_process) -> Option<Out> {
    (!pr.out_events.is_null()).then_some(Out(pr.out_events))
}

impl Out {
    fn push(&self, h: &clap_event_header) {
        // SAFETY: the host's queue, valid through this process call
        if let Some(f) = unsafe { (*self.0).try_push } {
            unsafe { f(self.0, h) };
        }
    }

    pub fn note_end(&self, time: u32, id: i32, channel: i16, key: i16) {
        let e = clap_event_note {
            header: header::<clap_event_note>(time, CLAP_EVENT_NOTE_END),
            note_id: id,
            port_index: 0,
            channel,
            key,
            velocity: 0.0,
        };
        self.push(&e.header);
    }

    pub fn param_value(&self, id: u32, value: f64) {
        let e = clap_event_param_value {
            header: header::<clap_event_param_value>(0, CLAP_EVENT_PARAM_VALUE),
            param_id: id,
            cookie: core::ptr::null_mut(),
            note_id: -1,
            port_index: -1,
            channel: -1,
            key: -1,
            value,
        };
        self.push(&e.header);
    }
}

fn header<T>(time: u32, kind: u16) -> clap_event_header {
    clap_event_header {
        size: core::mem::size_of::<T>() as u32,
        time,
        space_id: CLAP_CORE_EVENT_SPACE_ID,
        type_: kind,
        flags: 0,
    }
}

fn has(bits: u16, k: usize) -> bool {
    bits.checked_shr(k as u32).is_some_and(|b| b & 1 != 0)
}

/// This block's output ports: main, and each bus the host gave with two channels and activated.
pub struct Ports {
    ch: [[*mut f32; 2]; PORTS],
    on: u16,
}

impl Ports {
    pub fn new(pr: &clap_process, active: u16) -> Option<Ports> {
        let mut p = Ports {
            ch: [[core::ptr::null_mut(); 2]; PORTS],
            on: 0,
        };
        let n = (pr.audio_outputs_count as usize).min(PORTS);
        let bufs: &[clap_audio_buffer] = if pr.audio_outputs.is_null() {
            &[]
        } else {
            unsafe { core::slice::from_raw_parts(pr.audio_outputs, n) }
        };
        // (ranges, not `enumerate`, whose counter is an overflow check)
        for (k, (b, ch)) in (0..PORTS as u32).zip(bufs.iter().zip(p.ch.iter_mut())) {
            let bit = 1u16.checked_shl(k).unwrap_or(0);
            if b.channel_count < 2 || b.data32.is_null() || (k > 0 && active & bit == 0) {
                continue;
            }
            // SAFETY: at least two channel pointers
            let (l, r) = unsafe { (*b.data32, *b.data32.add(1)) };
            if !l.is_null() && !r.is_null() {
                *ch = [l, r];
                p.on |= bit;
            }
        }
        (p.on & 1 != 0).then_some(p)
    }

    pub fn active(&self, k: usize) -> bool {
        k < PORTS && self.on.checked_shr(k as u32).is_some_and(|b| b & 1 != 0)
    }

    fn channel<'a>(p: *mut f32, at: usize, n: usize) -> &'a mut [f32] {
        // SAFETY: the host's channel holds the block's frames; `at + n` is within them
        unsafe { core::slice::from_raw_parts_mut(p.add(at), n) }
    }

    /// Zeros every active port's first `frames` frames.
    pub fn clear(&self, frames: usize) {
        for (_, ch) in (0..PORTS).zip(&self.ch).filter(|(k, _)| self.active(*k)) {
            for &c in ch {
                Self::channel(c, 0, frames).fill(0.0);
            }
        }
    }

    /// Port `k`'s frames `at..at + n` from the engine's pair (silence where nothing was written).
    pub fn write(&self, k: usize, at: usize, n: usize, pair: &[[f32; BLOCK]; 2], w: Written) {
        let Some([l, r]) = self.ch.get(k).filter(|_| self.active(k)) else {
            return;
        };
        let (left, right) = (has(w.left, k), has(w.right, k));
        let (dl, dr) = (Self::channel(*l, at, n), Self::channel(*r, at, n));
        match (
            pair.first().and_then(|x| x.get(..n)),
            pair.get(1).and_then(|x| x.get(..n)),
        ) {
            (Some(sl), Some(sr)) if left => {
                dl.copy_from_slice(sl);
                dr.copy_from_slice(if right { sr } else { sl });
            }
            _ => {
                dl.fill(0.0);
                dr.fill(0.0);
            }
        }
    }

    /// Adds the engine's pair `k` (written) into ring-out pair `which` of `ring`'s four channels.
    pub fn add_ring(
        ring: *const *mut f32,
        which: usize,
        k: usize,
        at: usize,
        n: usize,
        pair: &[[f32; BLOCK]; 2],
        w: Written,
    ) {
        if ring.is_null() || !has(w.left, k) {
            return;
        }
        let right = has(w.right, k);
        for c in 0..2 {
            // SAFETY: ring_out checked four channels
            let d = unsafe { *ring.add(which.saturating_mul(2).saturating_add(c).min(3)) };
            let s = pair
                .get(if right { c } else { 0 })
                .and_then(|x| x.get(..n))
                .unwrap_or(&[]);
            for (d, s) in Self::channel(d, at, n).iter_mut().zip(s) {
                *d += *s;
            }
        }
    }
}

// ---- streams ------------------------------------------------------------------------------------

fn write_all(s: *const clap_ostream, data: &[u8]) -> bool {
    let Some(f) = (unsafe { s.as_ref() }).and_then(|x| x.write) else {
        return false;
    };
    let mut at = 0usize;
    while let Some(rest) = data.get(at..).filter(|r| !r.is_empty()) {
        let n = unsafe { f(s, rest.as_ptr().cast(), rest.len() as u64) };
        if n <= 0 || n as usize > rest.len() {
            return false;
        }
        at += n as usize;
    }
    true
}

fn read_all(s: *const clap_istream) -> Option<String> {
    let f = unsafe { s.as_ref() }.and_then(|x| x.read)?;
    let (mut data, mut buf) = (Vec::new(), [0u8; 4096]);
    loop {
        let n = unsafe { f(s, buf.as_mut_ptr().cast(), buf.len() as u64) };
        if n == 0 {
            break;
        }
        let chunk = buf.get(..usize::try_from(n).ok()?)?;
        data.extend_from_slice(chunk);
        if data.len() > state::MAX_BYTES {
            return None;
        }
    }
    String::from_utf8(data).ok()
}

// ---- the plugin ---------------------------------------------------------------------------------

unsafe extern "C" fn init(_: *const clap_plugin) -> bool {
    true
}

unsafe extern "C" fn destroy(p: *const clap_plugin) {
    drop(unsafe { Box::from_raw(p.cast_mut().cast::<Instance>()) });
}

unsafe extern "C" fn activate(p: *const clap_plugin, rate: f64, _: u32, max: u32) -> bool {
    let s = unsafe { this(p) };
    s.activate(rate, max);
    s.active.store(true, Relaxed);
    true
}

unsafe extern "C" fn deactivate(p: *const clap_plugin) {
    unsafe { this(p) }.active.store(false, Relaxed);
}

unsafe extern "C" fn start_processing(_: *const clap_plugin) -> bool {
    true
}

unsafe extern "C" fn stop_processing(_: *const clap_plugin) {}

// The audio-thread callbacks (reset, process, set_active, params_flush while active, ring_out and
// the voice budget's) are the roots of the link-time panic check (tools/check-no-panic.sh).

unsafe extern "C" fn reset(p: *const clap_plugin) {
    let s = unsafe { this(p) };
    s.with_audio(|a| s.silence(a));
}

unsafe extern "C" fn process(p: *const clap_plugin, pr: *const clap_process) -> clap_process_status {
    let s = unsafe { this(p) };
    match (unsafe { pr.as_ref() }, s.ports.load(Relaxed)) {
        (Some(pr), active) => match Ports::new(pr, active) {
            // a block the main thread holds the engine through (a state load) is silent
            Some(ports) => s.with_audio(|a| s.process(a, pr, &ports)).unwrap_or_else(|| {
                ports.clear(pr.frames_count as usize);
                CLAP_PROCESS_CONTINUE
            }),
            None => CLAP_PROCESS_ERROR,
        },
        (None, _) => CLAP_PROCESS_ERROR,
    }
}

unsafe extern "C" fn on_main_thread(_: *const clap_plugin) {}

unsafe extern "C" fn get_extension(_: *const clap_plugin, id: *const c_char) -> *const c_void {
    let Some(id) = (!id.is_null()).then(|| unsafe { CStr::from_ptr(id) }) else {
        return core::ptr::null();
    };
    let table: *const c_void = match id {
        x if x == CLAP_EXT_AUDIO_PORTS => (&AUDIO_PORTS as *const clap_plugin_audio_ports).cast(),
        x if x == CLAP_EXT_AUDIO_PORTS_ACTIVATION || x == CLAP_EXT_AUDIO_PORTS_ACTIVATION_COMPAT => {
            (&ACTIVATION as *const clap_plugin_audio_ports_activation).cast()
        }
        x if x == CLAP_EXT_NOTE_PORTS => (&NOTE_PORTS as *const clap_plugin_note_ports).cast(),
        x if x == CLAP_EXT_PARAMS => (&PARAMS_EXT as *const clap_plugin_params).cast(),
        x if x == CLAP_EXT_STATE => (&STATE as *const clap_plugin_state).cast(),
        x if x == ids::OSCAIRN_EXT_SOUND_CONTEXTS => (&CONTEXTS.0 as *const oscairn_plugin_sound_contexts).cast(),
        x if x == ids::OSCAIRN_EXT_SOUND_SNAPSHOTS => (&SNAPSHOTS as *const oscairn_plugin_sound_snapshots).cast(),
        x if x == ids::OSCAIRN_EXT_VOICE_BUDGET => (&BUDGET as *const oscairn_plugin_voice_budget).cast(),
        x if x == ids::OSCAIRN_EXT_MEMORY => (&MEMORY as *const oscairn_plugin_memory).cast(),
        x if x == ids::OSCAIRN_EXT_UI || x == ids::OSCAIRN_EXT_UI_DRAFT_4 => (&UI as *const oscairn_plugin_ui).cast(),
        _ => core::ptr::null(),
    };
    table
}

// ---- audio ports, their activation, the note port ----------------------------------------------

unsafe extern "C" fn audio_ports_count(_: *const clap_plugin, input: bool) -> u32 {
    if input {
        0
    } else {
        PORTS as u32
    }
}

unsafe extern "C" fn audio_ports_get(
    _: *const clap_plugin,
    i: u32,
    input: bool,
    info: *mut clap_audio_port_info,
) -> bool {
    let Some(info) = (unsafe { info.as_mut() }).filter(|_| !input && (i as usize) < PORTS) else {
        return false;
    };
    info.id = i;
    copy(
        if i == 0 { "Main".into() } else { format!("Bus {i}") }.as_bytes(),
        &mut info.name,
    );
    info.flags = if i == 0 { CLAP_AUDIO_PORT_IS_MAIN } else { 0 };
    info.channel_count = 2;
    info.port_type = CLAP_PORT_STEREO.as_ptr();
    info.in_place_pair = CLAP_INVALID_ID;
    true
}

static AUDIO_PORTS: clap_plugin_audio_ports = clap_plugin_audio_ports {
    count: Some(audio_ports_count),
    get: Some(audio_ports_get),
};

unsafe extern "C" fn can_activate_while_processing(_: *const clap_plugin) -> bool {
    true
}

unsafe extern "C" fn set_active(p: *const clap_plugin, input: bool, i: u32, on: bool, _: u32) -> bool {
    let ok = !input && (i as usize) < PORTS;
    let b = if ok && i > 0 {
        1u16.checked_shl(i).unwrap_or(0)
    } else {
        0
    };
    let s = unsafe { this(p) };
    if on {
        s.ports.fetch_or(b, Relaxed);
    } else {
        s.ports.fetch_and(!b, Relaxed);
    }
    ok
}

static ACTIVATION: clap_plugin_audio_ports_activation = clap_plugin_audio_ports_activation {
    can_activate_while_processing: Some(can_activate_while_processing),
    set_active: Some(set_active),
};

unsafe extern "C" fn note_ports_count(_: *const clap_plugin, input: bool) -> u32 {
    u32::from(input)
}

unsafe extern "C" fn note_ports_get(
    _: *const clap_plugin,
    i: u32,
    input: bool,
    info: *mut clap_note_port_info,
) -> bool {
    let Some(info) = (unsafe { info.as_mut() }).filter(|_| input && i == 0) else {
        return false;
    };
    info.id = 0;
    info.supported_dialects = CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI;
    info.preferred_dialect = CLAP_NOTE_DIALECT_CLAP;
    copy(b"Notes", &mut info.name);
    true
}

static NOTE_PORTS: clap_plugin_note_ports = clap_plugin_note_ports {
    count: Some(note_ports_count),
    get: Some(note_ports_get),
};

// ---- parameters ---------------------------------------------------------------------------------

unsafe extern "C" fn params_count(_: *const clap_plugin) -> u32 {
    PARAM_COUNT as u32
}

unsafe extern "C" fn params_info(_: *const clap_plugin, i: u32, info: *mut clap_param_info) -> bool {
    let Some(info) = (unsafe { info.as_mut() }) else {
        return false;
    };
    let flags = CLAP_PARAM_IS_AUTOMATABLE | CLAP_PARAM_IS_STEPPED;
    let (id, name, module, min, max, def) = match PARAMS.get(i as usize) {
        Some(p) => {
            let module = match (p.op, p.id) {
                (Some(o), _) => format!("Operator {}", o + 1),
                (None, 0..100) => "Common".into(),
                (None, 100..200) => "Filter".into(),
                _ => "Part".into(),
            };
            let name = p.op.map_or(p.name.to_string(), |o| format!("op{}.{}", o + 1, p.name));
            (u32::from(p.id), name, module, p.min, p.max, p.default)
        }
        None if i as usize == PARAM_COUNT - 1 => (BUS_ID, "output_bus".into(), "Output".into(), 0, 8, 0),
        None => return false,
    };
    info.id = id;
    info.flags = flags;
    info.cookie = core::ptr::null_mut();
    copy(name.as_bytes(), &mut info.name);
    copy(module.as_bytes(), &mut info.module);
    (info.min_value, info.max_value, info.default_value) = (f64::from(min), f64::from(max), f64::from(def));
    true
}

unsafe extern "C" fn params_value(p: *const clap_plugin, id: clap_id, value: *mut f64) -> bool {
    match (unsafe { this(p) }.value(id), unsafe { value.as_mut() }) {
        (Some(v), Some(out)) => {
            *out = v;
            true
        }
        _ => false,
    }
}

fn known(id: clap_id) -> bool {
    id == BUS_ID || u16::try_from(id).ok().and_then(index_of).is_some()
}

unsafe extern "C" fn params_to_text(_: *const clap_plugin, id: clap_id, v: f64, out: *mut c_char, n: u32) -> bool {
    if !known(id) || out.is_null() || n == 0 || !v.is_finite() {
        return false;
    }
    let t = match v.round() as i64 {
        0 if id == BUS_ID => "Main".to_string(),
        b if id == BUS_ID => format!("Bus {b}"),
        x => x.to_string(),
    };
    copy(t.as_bytes(), unsafe {
        core::slice::from_raw_parts_mut(out, n as usize)
    });
    true
}

unsafe extern "C" fn params_from_text(_: *const clap_plugin, id: clap_id, t: *const c_char, v: *mut f64) -> bool {
    let (Some(t), Some(v)) = (unsafe { text(t) }, unsafe { v.as_mut() }) else {
        return false;
    };
    let t = t.trim();
    let x = match t {
        "Main" if id == BUS_ID => Some(0.0),
        _ => t.strip_prefix("Bus ").unwrap_or(t).parse::<f64>().ok(),
    };
    match x.filter(|_| known(id)) {
        Some(x) => {
            *v = x;
            true
        }
        None => false,
    }
}

unsafe extern "C" fn params_flush(
    p: *const clap_plugin,
    input: *const clap_input_events,
    _: *const clap_output_events,
) {
    let s = unsafe { this(p) };
    if let Some(e) = unsafe { input.as_ref() } {
        // the audio thread's while active, else the main thread's
        if s.active.load(Relaxed) {
            s.with_audio(|a| s.flush(a, e));
        } else {
            s.exclusive(|a| s.flush(a, e));
        }
    }
}

static PARAMS_EXT: clap_plugin_params = clap_plugin_params {
    count: Some(params_count),
    get_info: Some(params_info),
    get_value: Some(params_value),
    value_to_text: Some(params_to_text),
    text_to_value: Some(params_from_text),
    flush: Some(params_flush),
};

// ---- state (CLAP's, and the sound-contexts snapshot: the same text) -----------------------------

unsafe extern "C" fn state_save(p: *const clap_plugin, s: *const clap_ostream) -> bool {
    unsafe { this(p) }.save(None).is_some_and(|d| write_all(s, &d))
}

unsafe extern "C" fn state_load(p: *const clap_plugin, s: *const clap_istream) -> bool {
    match read_all(s).map(|t| state::read(&t)) {
        Some(Ok(st)) => {
            unsafe { this(p) }.load(st);
            true
        }
        _ => false,
    }
}

static STATE: clap_plugin_state = clap_plugin_state {
    save: Some(state_save),
    load: Some(state_load),
};

// ---- sound contexts and snapshots ---------------------------------------------------------------

unsafe extern "C" fn contexts_capacity(_: *const clap_plugin) -> u32 {
    SOUNDS as u32
}

unsafe extern "C" fn contexts_get(p: *const clap_plugin, slot: u32, info: *mut oscairn_sound_context_info) -> bool {
    let s = unsafe { this(p) };
    let m = unsafe { s.main() };
    let (Some(info), Some(c), Some(true), Some(name)) = (
        unsafe { info.as_mut() },
        s.slots.get(slot as usize),
        m.prepared.get(slot as usize),
        m.names.get(slot as usize),
    ) else {
        return false;
    };
    info.id = c.id.load(Relaxed);
    copy(name.as_bytes(), &mut info.name);
    info.selected_channels = c.channels.load(Relaxed);
    info.active_voices = c.voices.load(Relaxed);
    info.idle = c.users.load(core::sync::atomic::Ordering::Acquire) == 0;
    true
}

#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn contexts_prepare(
    p: *const clap_plugin,
    name: *const c_char,
    values: *const oscairn_sound_parameter,
    count: u32,
    patch: *const c_char,
    _: *const oscairn_asset_ref,
    message: *mut c_char,
    capacity: u32,
) -> u32 {
    let refuse = |why: &str| {
        fail(message, capacity, why);
        CLAP_INVALID_ID
    };
    let Some(name) = unsafe { text(name) }.filter(|n| n.len() <= 63) else {
        return refuse("invalid sound name");
    };
    let mut sound = Sound::default();
    if !patch.is_null() {
        let Some(t) = (unsafe { text(patch) }) else {
            return refuse("line 1: not UTF-8");
        };
        match Sound::from_text(t) {
            Ok(s) => sound = s,
            Err((line, e)) => return refuse(&format!("line {line}: {e:?}")),
        }
    }
    let values: &[oscairn_sound_parameter] = if values.is_null() || count == 0 {
        &[]
    } else {
        unsafe { core::slice::from_raw_parts(values, count as usize) }
    };
    let mut bus = 0u8;
    for v in values {
        let whole = v.value.is_finite() && v.value.fract() == 0.0;
        let ok = match u16::try_from(v.id) {
            _ if v.id == BUS_ID && whole && (0.0..PORTS as f64).contains(&v.value) => {
                bus = v.value as u8;
                true
            }
            Ok(id) => whole && sound.set(id, v.value as i16).is_ok(),
            Err(_) => false,
        };
        if !ok {
            return refuse("invalid parameter");
        }
    }
    unsafe { this(p) }
        .publish(name, &sound, bus)
        .unwrap_or_else(|| refuse("no free sound context"))
}

unsafe extern "C" fn contexts_select(p: *const clap_plugin, id: u32, channel: i16) -> bool {
    unsafe { this(p) }.select(id, channel)
}

unsafe extern "C" fn contexts_discard(p: *const clap_plugin, id: u32) -> bool {
    unsafe { this(p) }.discard(id)
}

struct Contexts(oscairn_plugin_sound_contexts);
// SAFETY: a table of functions and a pointer to a static table
unsafe impl Sync for Contexts {}
static CONTEXTS: Contexts = Contexts(oscairn_plugin_sound_contexts {
    capacity: Some(contexts_capacity),
    get: Some(contexts_get),
    prepare: Some(contexts_prepare),
    select: Some(contexts_select),
    discard: Some(contexts_discard),
    state: &STATE,
    prepare_preset: None,
});

unsafe extern "C" fn snapshot_prepare(
    p: *const clap_plugin,
    snapshot: *const clap_istream,
    selected: *mut u32,
    message: *mut c_char,
    capacity: u32,
) -> bool {
    if selected.is_null() {
        fail(message, capacity, "invalid arguments");
        return false;
    }
    let st = match read_all(snapshot).map(|t| state::read(&t)) {
        Some(Ok(st)) => st,
        Some(Err(why)) => {
            fail(message, capacity, &format!("invalid snapshot: {why}"));
            return false;
        }
        None => {
            fail(message, capacity, "invalid snapshot");
            return false;
        }
    };
    match unsafe { this(p) }.prepare_state(&st) {
        Ok(ids) => {
            // SAFETY: the host gives 16
            unsafe { core::slice::from_raw_parts_mut(selected, 16) }.copy_from_slice(&ids);
            true
        }
        Err(why) => {
            fail(message, capacity, why);
            false
        }
    }
}

unsafe extern "C" fn snapshot_ring_out(p: *const clap_plugin, buffer: *const clap_audio_buffer) {
    let s = unsafe { this(p) };
    let ring = unsafe { buffer.as_ref() }
        .filter(|b| b.channel_count >= 4 && !b.data32.is_null())
        .map(|b| b.data32.cast_const())
        .filter(|&d| (0..4).all(|c| !unsafe { *d.add(c) }.is_null()))
        .unwrap_or(core::ptr::null());
    s.with_audio(|a: &mut Audio| a.ring = ring);
}

unsafe extern "C" fn snapshot_choke_retiring(p: *const clap_plugin) -> bool {
    unsafe { this(p) }.choke_retiring()
}

unsafe extern "C" fn snapshot_save_sound(p: *const clap_plugin, id: u32, s: *const clap_ostream) -> bool {
    let pl = unsafe { this(p) };
    pl.slot_of(id)
        .and_then(|slot| pl.save(Some(slot)))
        .is_some_and(|d| write_all(s, &d))
}

unsafe extern "C" fn snapshot_values(
    p: *const clap_plugin,
    id: u32,
    out: *mut oscairn_sound_parameter,
    capacity: u32,
) -> u32 {
    let s = unsafe { this(p) };
    let Some(slot) = s.slot_of(id).and_then(|i| s.slots.get(i)) else {
        return 0;
    };
    if slot.channels.load(Relaxed) != 0 {
        return 0;
    }
    let out: &mut [oscairn_sound_parameter] = if out.is_null() {
        &mut []
    } else {
        unsafe { core::slice::from_raw_parts_mut(out, capacity as usize) }
    };
    let all = PARAMS
        .iter()
        .zip(&slot.values)
        .map(|(p, v)| (u32::from(p.id), f64::from(v.load(Relaxed))))
        .chain(core::iter::once((BUS_ID, f64::from(slot.bus.load(Relaxed)))));
    for (o, (id, value)) in out.iter_mut().zip(all) {
        *o = oscairn_sound_parameter { id, value };
    }
    PARAM_COUNT as u32
}

static SNAPSHOTS: oscairn_plugin_sound_snapshots = oscairn_plugin_sound_snapshots {
    prepare: Some(snapshot_prepare),
    ring_out: Some(snapshot_ring_out),
    choke_retiring: Some(snapshot_choke_retiring),
    save_sound: Some(snapshot_save_sound),
    sound_values: Some(snapshot_values),
};

// ---- voice budget, memory, editor hints ---------------------------------------------------------

unsafe extern "C" fn budget_honors_volume(_: *const clap_plugin) -> bool {
    false
}

unsafe extern "C" fn budget_set_limits(
    p: *const clap_plugin,
    limit: u32,
    list: *const oscairn_voice_budget_context,
    n: u32,
) {
    let list: &[oscairn_voice_budget_context] = if list.is_null() {
        &[]
    } else {
        unsafe { core::slice::from_raw_parts(list, (n as usize).min(16)) }
    };
    unsafe { this(p) }.with_audio(|a| Plugin::set_limits(a, limit, list));
}

unsafe extern "C" fn budget_active_voices(p: *const clap_plugin, cost: *mut f64) -> u32 {
    let s = unsafe { this(p) };
    let (n, c) = s.with_audio(|a| s.active_voices(a)).unwrap_or((0, 0.0));
    if let Some(x) = unsafe { cost.as_mut() } {
        *x = c;
    }
    n
}

unsafe extern "C" fn budget_notes(p: *const clap_plugin, out: *mut oscairn_voice_budget_note, capacity: u32) -> u32 {
    let s = unsafe { this(p) };
    let out: &mut [oscairn_voice_budget_note] = if out.is_null() {
        &mut []
    } else {
        unsafe { core::slice::from_raw_parts_mut(out, capacity as usize) }
    };
    s.with_audio(|a| s.notes(a, out)).unwrap_or(0)
}

static BUDGET: oscairn_plugin_voice_budget = oscairn_plugin_voice_budget {
    honors_volume: Some(budget_honors_volume),
    set_limits: Some(budget_set_limits),
    active_voices: Some(budget_active_voices),
    notes: Some(budget_notes),
};

unsafe extern "C" fn memory_estimate(_: *const clap_plugin, _: f64, _: u32, out: *mut oscairn_memory) -> bool {
    let Some(o) = (unsafe { out.as_mut() }) else {
        return false;
    };
    *o = oscairn_memory {
        shared_bytes: Plugin::memory(),
        context_bytes: 0,
        asset_bytes: 0,
    };
    true
}

unsafe extern "C" fn memory_current(p: *const clap_plugin, out: *mut oscairn_memory) -> bool {
    unsafe { memory_estimate(p, 0.0, 0, out) }
}

static MEMORY: oscairn_plugin_memory = oscairn_plugin_memory {
    estimate: Some(memory_estimate),
    current: Some(memory_current),
};

unsafe extern "C" fn ui_hint(_: *const clap_plugin, id: clap_id, h: *mut oscairn_param_hint) -> bool {
    let Some(h) = (unsafe { h.as_mut() }).filter(|_| known(id)) else {
        return false;
    };
    *h = oscairn_param_hint {
        range_high: CLAP_INVALID_ID,
        ..Default::default()
    };
    if id == BUS_ID {
        h.role = oscairn_role::OSCAIRN_ROLE_OUTPUT_BUS;
    }
    true
}

static UI: oscairn_plugin_ui = oscairn_plugin_ui {
    param_hint: Some(ui_hint),
    algorithm: None,
    seconds: None,
};

// ---- factory and entry --------------------------------------------------------------------------

unsafe extern "C" fn factory_count(_: *const clap_plugin_factory) -> u32 {
    1
}

unsafe extern "C" fn factory_descriptor(_: *const clap_plugin_factory, i: u32) -> *const clap_plugin_descriptor {
    if i == 0 {
        &DESCRIPTOR
    } else {
        core::ptr::null()
    }
}

unsafe extern "C" fn factory_create(
    _: *const clap_plugin_factory,
    host: *const clap_host,
    id: *const c_char,
) -> *const clap_plugin {
    if id.is_null() || unsafe { CStr::from_ptr(id) } != PLUGIN_ID {
        return core::ptr::null();
    }
    let i = Box::new(Instance {
        clap: clap_plugin {
            desc: &DESCRIPTOR,
            plugin_data: core::ptr::null_mut(),
            init: Some(init),
            destroy: Some(destroy),
            activate: Some(activate),
            deactivate: Some(deactivate),
            start_processing: Some(start_processing),
            stop_processing: Some(stop_processing),
            reset: Some(reset),
            process: Some(process),
            get_extension: Some(get_extension),
            on_main_thread: Some(on_main_thread),
        },
        plugin: Plugin::new(host),
    });
    Box::into_raw(i).cast_const().cast()
}

static FACTORY: clap_plugin_factory = clap_plugin_factory {
    get_plugin_count: Some(factory_count),
    get_plugin_descriptor: Some(factory_descriptor),
    create_plugin: Some(factory_create),
};

unsafe extern "C" fn entry_init(_: *const c_char) -> bool {
    true
}

unsafe extern "C" fn entry_deinit() {}

unsafe extern "C" fn entry_factory(id: *const c_char) -> *const c_void {
    if !id.is_null() && unsafe { CStr::from_ptr(id) } == CLAP_PLUGIN_FACTORY_ID {
        (&FACTORY as *const clap_plugin_factory).cast()
    } else {
        core::ptr::null()
    }
}

#[allow(non_upper_case_globals)]
#[no_mangle]
pub static clap_entry: clap_plugin_entry = clap_plugin_entry {
    clap_version: CLAP_VERSION,
    init: Some(entry_init),
    deinit: Some(entry_deinit),
    get_factory: Some(entry_factory),
};
