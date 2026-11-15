// SPDX-License-Identifier: AGPL-3.0-or-later
//! The instance: sound contexts, notes, parameters, ports and the voice budget around the core's
//! engine (docs/design.md, "The shell"; docs/plugin.md). Three kinds of data, by who may touch
//! them: atomics (either thread); [`Main`], the main thread's; [`Audio`], the audio thread's,
//! which the main thread takes only inside [`Plugin::exclusive`] (an activation or a state load:
//! the audio thread then skips its callbacks, a block of silence, and never waits).

use crate::ffi::{Out, Ports};
use crate::state::{Saved, State};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicI16, AtomicU16, AtomicU32, AtomicU8, Ordering::*};
use fm_core::engine::{Engine, Event, Kind, Limits, NoteEnd, Prepared, BLOCK, SOUNDS};
use fm_core::params::{index_of, COUNT, PARAMS};
use fm_core::simd::Native;
use fm_core::sound::Sound;
use oscairn_clap_sys::clap_sys::events::*;
use oscairn_clap_sys::clap_sys::process::*;
use oscairn_clap_sys::{oscairn_voice_budget_context, oscairn_voice_budget_note, OSCAIRN_VOICE_BUDGET_SELECTED};

/// Stereo output ports: 0 main, n the bus of sounds whose output bus is n.
pub const PORTS: usize = 9;
/// The engine's output pairs: the ports, then a retiring sound's ring-out pair and the pair a
/// second change's older tail fades out in.
const OUTS: usize = PORTS + 2;
/// The output bus parameter's id (the sound parameters' ids are the sound format's, below 10,000).
pub const BUS_ID: u32 = 10_000;
/// Parameters: every sound parameter, then the output bus.
pub const PARAM_COUNT: usize = COUNT + 1;
/// Notes the instance keeps track of (voices, notes waiting out a key-on delay, mono keys kept).
const NOTES: usize = 320;
/// Core events a stretch of a block takes (a stretch ends early before an event that might not fit).
const EVENTS: usize = 1024;
const QUEUE: u32 = 32;
/// The selection queue's entry for `choke_retiring`.
const CHOKE: u32 = u32::MAX;
/// Instructions a microsecond, to state a voice's cost in time: an assumption until the device's
/// timing gives the figure (docs/plugin.md).
const INSTRUCTIONS_A_MICROSECOND: f64 = 1000.0;

/// A sound context: either thread.
pub struct Slot {
    pub id: AtomicU32,
    /// Selected channels, queued selections and live notes, each one.
    pub users: AtomicU32,
    pub channels: AtomicU32,
    pub voices: AtomicU32,
    pub bus: AtomicU8,
    /// Its raw values (the audio thread writes the selected sounds', the main thread the others').
    pub values: [AtomicI16; COUNT],
    /// Prepared by the main thread while the slot is free, installed by the audio thread when it is
    /// first selected (`fresh`).
    prep: UnsafeCell<Prepared>,
    fresh: AtomicBool,
}

impl Slot {
    fn new(id: u32, users: u32, channels: u32) -> Slot {
        Slot {
            id: AtomicU32::new(id),
            users: AtomicU32::new(users),
            channels: AtomicU32::new(channels),
            voices: AtomicU32::new(0),
            bus: AtomicU8::new(0),
            values: core::array::from_fn(|i| AtomicI16::new(PARAMS.get(i).map_or(0, |p| p.default))),
            prep: UnsafeCell::new(Prepared::new(&Sound::default())),
            fresh: AtomicBool::new(false),
        }
    }

    /// Its sound, from its values.
    pub fn sound(&self) -> Sound {
        let mut s = Sound::default();
        for (p, v) in PARAMS.iter().zip(&self.values) {
            let _ = s.set(p.id, v.load(Relaxed));
        }
        s
    }

    fn store(&self, s: &Sound, bus: u8) {
        for (i, v) in self.values.iter().enumerate() {
            v.store(s.get(i), Relaxed);
        }
        self.bus.store(bus, Relaxed);
    }
}

/// The main thread's.
pub struct Main {
    pub prepared: [bool; SOUNDS],
    pub names: [String; SOUNDS],
    next_id: u32,
    pub rate: f64,
    pub max_frames: u32,
}

#[derive(Clone, Copy, Default)]
struct Note {
    live: bool,
    held: bool,
    slot: u8,
    key: i16,
    channel: i16,
    id: i32,
    age: u32,
}

/// The notes sounding, by the core's note id (an index here).
struct Notes {
    n: [Note; NOTES],
    live: usize,
    age: u32,
    /// Retiring sounds whose notes a second change faded out, a bit each.
    fading: u8,
}

impl Notes {
    fn start(&mut self, slots: &[Slot; SOUNDS], slot: u8, id: i32, channel: i16, key: i16) -> Option<u32> {
        let (i, n) = (0..NOTES as u32).zip(self.n.iter_mut()).find(|(_, n)| !n.live)?;
        self.age = self.age.wrapping_add(1);
        *n = Note {
            live: true,
            held: true,
            slot,
            key,
            channel,
            id,
            age: self.age,
        };
        self.live = self.live.saturating_add(1);
        if let Some(s) = slots.get(usize::from(slot)) {
            s.users.fetch_add(1, Relaxed);
            s.voices.fetch_add(1, Relaxed);
        }
        Some(i)
    }

    /// Note `id` ended: its NOTE_END at `time` (none without `out`).
    fn end(&mut self, slots: &[Slot; SOUNDS], out: Option<&Out>, time: u32, id: u32) {
        let Some(n) = self.n.get_mut(id as usize).filter(|n| n.live) else {
            return;
        };
        n.live = false;
        self.live = self.live.saturating_sub(1);
        if let Some(o) = out {
            o.note_end(time, n.id, n.channel, n.key);
        }
        if let Some(s) = slots.get(usize::from(n.slot)) {
            if s.voices.fetch_sub(1, Relaxed) == 1 {
                self.fading &= !bit(n.slot);
            }
            s.users.fetch_sub(1, Release);
        }
    }

    /// The live notes that a CLAP address matches (-1: any).
    fn matching(&self, id: i32, channel: i16, key: i16) -> impl Iterator<Item = (u32, &Note)> {
        (0..NOTES as u32).zip(self.n.iter()).filter(move |(_, n)| {
            n.live && (id < 0 || n.id == id) && (channel < 0 || n.channel == channel) && (key < 0 || n.key == key)
        })
    }
}

/// A stretch's core events.
struct Events {
    e: [Event; EVENTS],
    n: usize,
}

impl Events {
    fn push(&mut self, frame: usize, slot: u8, kind: Kind) {
        if let Some(e) = self.e.get_mut(self.n) {
            *e = Event {
                frame,
                sound: slot,
                kind,
            };
            self.n = self.n.saturating_add(1);
        }
    }
}

/// The audio thread's.
pub struct Audio {
    pub engine: Engine,
    out: [[[f32; BLOCK]; 2]; OUTS],
    events: Events,
    notes: Notes,
    /// NOTE_ENDs a state load owes, sent at the next block's start.
    owed: [(i32, i16, i16); NOTES],
    owed_n: usize,
    selected: [u8; 16],
    /// The sound whose values the parameters show (channel 0's).
    shown: u8,
    budget: (u32, [oscairn_voice_budget_context; 16], usize),
    /// Sounds whose sustain pedal is down, a bit each.
    pedal: u8,
    /// `ring_out`'s buffer for the next block (its four channels), or null.
    pub ring: *const *mut f32,
    /// The steady time the next block should start at (-1: unknown).
    next: i64,
    frames: u32,
    cost: f64,
    /// Sounds prepared again (each once a stretch at most): the coalescing test's count.
    pub prepares: u64,
}

pub struct Plugin {
    pub host: *const oscairn_clap_sys::clap_sys::host::clap_host,
    pub slots: [Slot; SOUNDS],
    queue: [AtomicU32; QUEUE as usize],
    head: AtomicU32,
    tail: AtomicU32,
    /// Each channel's sound, for the main thread's readers.
    selected: [AtomicU8; 16],
    /// Output ports the host activated, a bit each (main always plays).
    pub ports: AtomicU16,
    pub active: AtomicBool,
    loading: AtomicBool,
    in_audio: AtomicBool,
    main: UnsafeCell<Main>,
    audio: UnsafeCell<Audio>,
}

// the cells are reached as the module's comment says
unsafe impl Sync for Plugin {}
unsafe impl Send for Plugin {}

fn bit(i: u8) -> u8 {
    1u8.checked_shl(u32::from(i)).unwrap_or(0)
}

fn bit32(i: i16) -> u32 {
    1u32.checked_shl(i as u32).unwrap_or(0)
}

/// A host's double as a raw value in `min..=max` (`max`/`min`, not `clamp`, which may panic).
fn raw(value: f64, min: i16, max: i16) -> Option<i16> {
    value
        .is_finite()
        .then(|| value.round().max(f64::from(min)).min(f64::from(max)) as i16)
}

impl Plugin {
    pub fn new(host: *const oscairn_clap_sys::clap_sys::host::clap_host) -> Box<Plugin> {
        Box::new(Plugin {
            host,
            slots: core::array::from_fn(|i| {
                if i == 0 {
                    Slot::new(0, 16, 0xffff)
                } else {
                    Slot::new(0, 0, 0)
                }
            }),
            queue: core::array::from_fn(|_| AtomicU32::new(0)),
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
            selected: core::array::from_fn(|_| AtomicU8::new(0)),
            ports: AtomicU16::new(1),
            active: AtomicBool::new(false),
            loading: AtomicBool::new(false),
            in_audio: AtomicBool::new(false),
            main: UnsafeCell::new(Main {
                prepared: core::array::from_fn(|i| i == 0),
                names: core::array::from_fn(|i| if i == 0 { "Default".into() } else { String::new() }),
                next_id: 1,
                rate: 48000.0,
                max_frames: 0,
            }),
            audio: UnsafeCell::new(Audio {
                engine: Engine::new(48000.0),
                out: [[[0.0; BLOCK]; 2]; OUTS],
                events: Events {
                    e: [Event {
                        frame: 0,
                        sound: 0,
                        kind: Kind::Off { id: 0 },
                    }; EVENTS],
                    n: 0,
                },
                notes: Notes {
                    n: [Note::default(); NOTES],
                    live: 0,
                    age: 0,
                    fading: 0,
                },
                owed: [(0, 0, 0); NOTES],
                owed_n: 0,
                selected: [0; 16],
                shown: 0,
                budget: (u32::MAX, [oscairn_voice_budget_context::default(); 16], 0),
                pedal: 0,
                ring: core::ptr::null(),
                next: -1,
                frames: BLOCK as u32,
                cost: 0.0,
                prepares: 0,
            }),
        })
    }

    /// The main thread's data.
    ///
    /// # Safety
    /// Main thread only, one reference at a time.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn main(&self) -> &mut Main {
        unsafe { &mut *self.main.get() }
    }

    /// Runs `f` on the audio thread's data unless the main thread holds it (then `None`).
    pub fn with_audio<R>(&self, f: impl FnOnce(&mut Audio) -> R) -> Option<R> {
        self.in_audio.store(true, SeqCst);
        // the main thread sets `loading` before it looks at `in_audio`: at most one of the two goes on
        let r = (!self.loading.load(SeqCst)).then(|| f(unsafe { &mut *self.audio.get() }));
        self.in_audio.store(false, SeqCst);
        r
    }

    /// Main thread: `f` on the audio thread's data, the audio thread's callbacks skipped meanwhile.
    pub fn exclusive<R>(&self, f: impl FnOnce(&mut Audio) -> R) -> R {
        self.loading.store(true, SeqCst);
        while self.in_audio.load(SeqCst) {
            std::thread::yield_now();
        }
        let r = f(unsafe { &mut *self.audio.get() });
        self.loading.store(false, SeqCst);
        r
    }

    pub fn wake(&self) {
        // SAFETY: the host outlives the plugin
        if let Some(h) = unsafe { self.host.as_ref() } {
            if let Some(f) = h.request_process {
                unsafe { f(self.host) };
            }
        }
    }

    /// The sound a channel selects (for the main thread).
    pub fn selected(&self, channel: usize) -> usize {
        self.selected.get(channel).map_or(0, |s| usize::from(s.load(Relaxed)))
    }

    // ---- activation and reset ------------------------------------------------------------------

    /// Main thread: a new engine at `rate`, every prepared sound installed, every note gone.
    pub fn activate(&self, rate: f64, max_frames: u32) {
        let m = unsafe { self.main() };
        (m.rate, m.max_frames) = (rate, max_frames);
        let prepared = m.prepared;
        self.exclusive(|a| {
            a.engine = Engine::new(rate);
            for (s, slot) in self
                .slots
                .iter()
                .enumerate()
                .filter(|(s, _)| prepared.get(*s) == Some(&true))
            {
                a.engine.set_sound(s, &Prepared::new(&slot.sound()));
                slot.fresh.store(false, Relaxed);
            }
            self.silence(a);
        });
    }

    /// Every note ends at once and silently (CLAP reset: no NOTE_END for them afterwards).
    pub fn silence(&self, a: &mut Audio) {
        a.engine.reset();
        for i in 0..NOTES as u32 {
            a.notes.end(&self.slots, None, 0, i);
        }
        (a.owed_n, a.notes.fading, a.pedal, a.next) = (0, 0, 0, -1);
    }

    // ---- sound contexts ------------------------------------------------------------------------

    pub fn slot_of(&self, id: u32) -> Option<usize> {
        let m = unsafe { self.main() };
        (0..SOUNDS)
            .find(|&s| m.prepared.get(s) == Some(&true) && self.slots.get(s).is_some_and(|x| x.id.load(Relaxed) == id))
    }

    fn push(&self, entry: u32, pin: Option<&Slot>) -> bool {
        let head = self.head.load(Relaxed);
        if head.wrapping_sub(self.tail.load(Acquire)) >= QUEUE {
            return false;
        }
        if let Some(s) = pin {
            s.users.fetch_add(1, Release);
        }
        if let Some(q) = self.queue.get((head % QUEUE) as usize) {
            q.store(entry, Relaxed);
        }
        self.head.store(head.wrapping_add(1), Release);
        true
    }

    /// Main thread: queues a selection for the next block (oscairn.sound-contexts `select`).
    pub fn select(&self, id: u32, channel: i16) -> bool {
        let Some(s) = self.slot_of(id).filter(|_| (-1..=15).contains(&channel)) else {
            return false;
        };
        self.push((s as u32) << 16 | u32::from(channel as u16), self.slots.get(s))
    }

    pub fn choke_retiring(&self) -> bool {
        self.push(CHOKE, None)
    }

    /// Main thread: a free slot made `sound` with its bus, named; its id.
    pub fn publish(&self, name: &str, sound: &Sound, bus: u8) -> Option<u32> {
        let m = unsafe { self.main() };
        let s = (1..SOUNDS).find(|&s| m.prepared.get(s) == Some(&false))?;
        let slot = self.slots.get(s)?;
        let id = m.next_id;
        m.next_id = id.checked_add(1)?;
        slot.store(sound, bus);
        // SAFETY: a free slot: the audio thread reads `prep` only after a selection of it
        unsafe { *slot.prep.get() = Prepared::new(sound) };
        slot.fresh.store(true, Release);
        slot.id.store(id, Relaxed);
        *m.prepared.get_mut(s)? = true;
        *m.names.get_mut(s)? = name.into();
        Some(id)
    }

    pub fn discard(&self, id: u32) -> bool {
        let Some(s) = self.slot_of(id).filter(|&s| s > 0) else {
            return false;
        };
        if self.slots.get(s).is_none_or(|x| x.users.load(Acquire) != 0) {
            return false;
        }
        if let Some(p) = unsafe { self.main() }.prepared.get_mut(s) {
            *p = false;
        }
        true
    }

    /// Applies the queued selections (the block's start, before its events).
    fn apply_selections(&self, a: &mut Audio, out: Option<&Out>) {
        let head = self.head.load(Acquire);
        let mut tail = self.tail.load(Relaxed);
        while tail != head {
            let e = self
                .queue
                .get((tail % QUEUE) as usize)
                .map_or(CHOKE, |q| q.load(Relaxed));
            tail = tail.wrapping_add(1);
            if e == CHOKE {
                // every retiring sound's notes fade out, into the second ring-out pair
                let Audio { engine, notes, .. } = a;
                for i in 0..NOTES as u32 {
                    let Some(n) = notes.n.get(i as usize).copied().filter(|n| n.live) else {
                        continue;
                    };
                    if self
                        .slots
                        .get(usize::from(n.slot))
                        .is_some_and(|s| s.channels.load(Relaxed) == 0)
                    {
                        notes.fading |= bit(n.slot);
                        engine.choke_note(usize::from(n.slot), i, &mut |x: NoteEnd| {
                            notes.end(&self.slots, out, 0, x.id)
                        });
                    }
                }
                continue;
            }
            let (s, channel) = ((e >> 16) as u8, e as u16 as i16);
            let Some(next) = self.slots.get(usize::from(s)) else {
                continue;
            };
            if next.fresh.swap(false, Acquire) {
                // SAFETY: published before the selection that pinned it
                a.engine.set_sound(usize::from(s), unsafe { &*next.prep.get() });
            }
            for (c, sel) in (0..16i16).zip(a.selected.iter_mut()) {
                if (channel >= 0 && channel != c) || *sel == s {
                    continue;
                }
                next.users.fetch_add(1, Relaxed);
                next.channels.fetch_or(bit32(c), Relaxed);
                if let Some(old) = self.slots.get(usize::from(*sel)) {
                    old.channels.fetch_and(!bit32(c), Relaxed);
                    old.users.fetch_sub(1, Release);
                }
                *sel = s;
                if let Some(x) = self.selected.get(c as usize) {
                    x.store(s, Relaxed);
                }
            }
            next.users.fetch_sub(1, Release);
        }
        self.tail.store(tail, Release);
    }

    // ---- state ---------------------------------------------------------------------------------

    /// Main thread: the whole instance as a state (`only`: one sound alone, on every channel and
    /// the main bus).
    pub fn save(&self, only: Option<usize>) -> Option<Vec<u8>> {
        let m = unsafe { self.main() };
        let mut st = State {
            sounds: Default::default(),
            selected: [0; 16],
        };
        for (s, saved) in st.sounds.iter_mut().enumerate() {
            let slot = self.slots.get(s)?;
            let keep = only.map_or(m.prepared.get(s) == Some(&true), |o| o == s);
            if keep {
                let bus = if only.is_some() { 0 } else { slot.bus.load(Relaxed) };
                *saved = Some(Saved {
                    name: m.names.get(s)?.clone(),
                    bus,
                    sound: slot.sound(),
                });
            }
        }
        if let Some(o) = only {
            st.sounds.swap(0, o);
        } else {
            for (c, x) in st.selected.iter_mut().enumerate() {
                *x = self.selected(c) as u8;
            }
        }
        crate::state::write(&st)
    }

    /// Main thread: loads a whole state; the notes sounding end (their NOTE_ENDs at the next block).
    pub fn load(&self, st: State) {
        let m = unsafe { self.main() };
        self.exclusive(|a| {
            // queued selections are dropped with their pins (the slots' counts are set below)
            self.tail.store(self.head.load(Acquire), Release);
            let mut owed = 0;
            for (o, n) in a.owed.iter_mut().zip(a.notes.n.iter().filter(|n| n.live)) {
                (*o, owed) = ((n.id, n.channel, n.key), owed + 1);
            }
            self.silence(a);
            a.owed_n = owed;
            for (s, (saved, slot)) in st.sounds.iter().zip(&self.slots).enumerate() {
                let (sound, bus, name) = saved
                    .as_ref()
                    .map_or((Sound::default(), 0, ""), |v| (v.sound, v.bus, &v.name));
                slot.store(&sound, bus);
                slot.fresh.store(false, Relaxed);
                a.engine.set_sound(s, &Prepared::new(&sound));
                let id = if s == 0 { 0 } else { m.next_id };
                if saved.is_some() && s > 0 {
                    m.next_id = m.next_id.saturating_add(1);
                }
                slot.id.store(id, Relaxed);
                let channels = (0..16)
                    .filter(|&c| st.selected.get(c) == Some(&(s as u8)))
                    .fold(0u32, |b, c| b | 1 << c);
                slot.channels.store(channels, Relaxed);
                slot.users.store(channels.count_ones(), Release);
                slot.voices.store(0, Relaxed);
                if let (Some(p), Some(n)) = (m.prepared.get_mut(s), m.names.get_mut(s)) {
                    (*p, *n) = (saved.is_some(), name.into());
                }
            }
            a.selected = st.selected;
            for (x, &s) in self.selected.iter().zip(&st.selected) {
                x.store(s, Relaxed);
            }
            a.shown = a.selected[0];
        });
        // the NOTE_ENDs owed go out in the next block
        self.wake();
    }

    /// Main thread: every sound of a state prepared beside the playing ones (all or none); each
    /// channel's sound's id.
    pub fn prepare_state(&self, st: &State) -> Result<[u32; 16], &'static str> {
        let m = unsafe { self.main() };
        let free = (1..SOUNDS).filter(|&s| m.prepared.get(s) == Some(&false)).count();
        if st.sounds.iter().flatten().count() > free {
            return Err("no free sound contexts for the snapshot");
        }
        let mut ids = [0u32; SOUNDS];
        for (id, saved) in ids.iter_mut().zip(&st.sounds) {
            if let Some(v) = saved {
                *id = self.publish(&v.name, &v.sound, v.bus).ok_or("no free sound context")?;
            }
        }
        Ok(st.selected.map(|s| ids.get(usize::from(s)).copied().unwrap_or(0)))
    }

    // ---- parameters ----------------------------------------------------------------------------

    /// A parameter's value as the host sees it: channel 0's sound's.
    pub fn value(&self, id: u32) -> Option<f64> {
        let slot = self.slots.get(self.selected(0))?;
        if id == BUS_ID {
            return Some(f64::from(slot.bus.load(Relaxed)));
        }
        let i = index_of(u16::try_from(id).ok()?)?;
        Some(f64::from(slot.values.get(i)?.load(Relaxed)))
    }

    /// A PARAM_VALUE for the sound `channel` selects (-1: every sound some channel selects).
    fn param(&self, a: &mut Audio, id: u32, channel: i16, value: f64) {
        let targets = if (0..16).contains(&channel) {
            a.selected.get(channel as usize).map_or(0, |&s| bit(s))
        } else {
            a.selected.iter().fold(0, |m, &s| m | bit(s))
        };
        for s in (0..SOUNDS as u8).filter(|&s| targets & bit(s) != 0) {
            let Some(slot) = self.slots.get(usize::from(s)) else {
                continue;
            };
            if id == BUS_ID {
                if let Some(v) = raw(value, 0, PORTS as i16 - 1) {
                    slot.bus.store(v as u8, Relaxed);
                }
                continue;
            }
            let Some(i) = u16::try_from(id).ok().and_then(index_of) else {
                return;
            };
            let Some(p) = PARAMS.get(i) else { return };
            let Some(v) = raw(value, p.min, p.max) else { return };
            // the cutoff scaling break points stay in ascending order (else no state could hold them)
            if p.name.starts_with("cutoff_break") {
                let at = |j: Option<usize>| j.and_then(|j| PARAMS.get(j).zip(slot.values.get(j)));
                let below = at(i.checked_sub(1)).filter(|(q, _)| q.name.starts_with("cutoff_break"));
                let above = at(i.checked_add(1)).filter(|(q, _)| q.name.starts_with("cutoff_break"));
                if below.is_some_and(|(_, x)| x.load(Relaxed) >= v) || above.is_some_and(|(_, x)| x.load(Relaxed) <= v)
                {
                    continue;
                }
            }
            if a.engine.stage(usize::from(s), p.id, v) {
                if let Some(x) = slot.values.get(i) {
                    x.store(v, Relaxed);
                }
            }
        }
    }

    /// Parameter events outside a block (params `flush`), on the audio thread or, inactive, the main.
    pub fn flush(&self, a: &mut Audio, events: &oscairn_clap_sys::clap_sys::events::clap_input_events) {
        for h in crate::ffi::events(events) {
            if let Some(e) = crate::ffi::param_value(h) {
                self.param(a, e.param_id, e.channel, e.value);
            }
        }
    }

    // ---- processing ----------------------------------------------------------------------------

    /// Whether nothing sounds, is owed or is queued: the plugin may sleep.
    fn idle(&self, a: &Audio) -> bool {
        a.notes.live == 0
            && a.owed_n == 0
            && self.head.load(Acquire) == self.tail.load(Relaxed)
            && a.engine.active() == 0
    }

    pub fn process(&self, a: &mut Audio, pr: &clap_process, ports: &Ports) -> clap_process_status {
        let out = crate::ffi::out(pr);
        let frames = pr.frames_count as usize;
        a.frames = pr.frames_count;
        // blocks the host skipped while the plugin slept: the engine's clock moves on over them
        if pr.steady_time >= 0 && a.next >= 0 && pr.steady_time > a.next {
            a.engine.skip(pr.steady_time.wrapping_sub(a.next) as u64);
        }
        a.next = if pr.steady_time >= 0 {
            pr.steady_time.saturating_add(i64::from(pr.frames_count))
        } else {
            -1
        };
        let ring = core::mem::replace(&mut a.ring, core::ptr::null());
        self.apply_selections(a, out.as_ref());
        for &(id, channel, key) in a.owed.iter().take(a.owed_n) {
            if let Some(o) = &out {
                o.note_end(0, id, channel, key);
            }
        }
        a.owed_n = 0;
        let count = crate::ffi::events_in(pr).map_or(0, crate::ffi::count);
        if count == 0 && self.idle(a) {
            ports.clear(frames);
            a.engine.skip(frames as u64);
            return CLAP_PROCESS_SLEEP;
        }
        self.limits(a);
        self.show(a, out.as_ref());
        let events = crate::ffi::events_in(pr);
        let mut e = 0usize;
        let mut at = 0usize;
        loop {
            let mut n = frames.saturating_sub(at).min(BLOCK);
            a.events.n = 0;
            let last = at.saturating_add(n) >= frames;
            while let Some(h) = events.and_then(|x| crate::ffi::event(x, e)) {
                let t = (h.time as usize).min(frames);
                // (the block's last stretch takes every event left)
                if t >= at.saturating_add(n) && !last {
                    break;
                }
                if a.events.n.saturating_add(NOTES + 2) > EVENTS {
                    // (a stretch of no frames still applies its events)
                    n = t.saturating_sub(at);
                    break;
                }
                self.event(a, h, t.saturating_sub(at), out.as_ref());
                e = e.saturating_add(1);
            }
            self.render(a, ports, ring, at, n, out.as_ref());
            at = at.saturating_add(n);
            if at >= frames && events.and_then(|x| crate::ffi::event(x, e)).is_none() {
                break;
            }
        }
        if self.idle(a) {
            CLAP_PROCESS_SLEEP
        } else {
            CLAP_PROCESS_CONTINUE
        }
    }

    /// One stretch of `n` frames from `at`: the core renders it, the outputs take it.
    fn render(&self, a: &mut Audio, ports: &Ports, ring: *const *mut f32, at: usize, n: usize, out: Option<&Out>) {
        let mut bus = [0u8; SOUNDS];
        for ((b, slot), s) in bus.iter_mut().zip(&self.slots).zip(0..SOUNDS as u8) {
            let to = usize::from(slot.bus.load(Relaxed));
            *b = if to == 0 && !ring.is_null() && slot.channels.load(Relaxed) == 0 {
                if a.notes.fading & bit(s) != 0 {
                    PORTS as u8 + 1
                } else {
                    PORTS as u8
                }
            } else if to < PORTS && ports.active(to) {
                to as u8
            } else {
                0
            };
        }
        a.prepares = a.prepares.wrapping_add(u64::from(a.engine.staged().count_ones()));
        let Audio {
            engine,
            out: o,
            events,
            notes,
            ..
        } = a;
        let time = at.saturating_add(n).saturating_sub(1) as u32;
        let ev = events.e.get(..events.n).unwrap_or(&[]);
        let w = engine.render_buses::<Native>(o, &bus, n, ev, &mut |x: NoteEnd| {
            notes.end(&self.slots, out, time, x.id)
        });
        for (k, pair) in (0..OUTS).zip(o.iter()) {
            if k < PORTS {
                ports.write(k, at, n, pair, w);
            } else {
                Ports::add_ring(ring, k.saturating_sub(PORTS), k, at, n, pair, w);
            }
        }
    }

    /// One input event at `frame` of the stretch.
    fn event(&self, a: &mut Audio, h: &clap_event_header, frame: usize, out: Option<&Out>) {
        if h.space_id != CLAP_CORE_EVENT_SPACE_ID {
            return;
        }
        let t = h.time;
        if let Some(e) = crate::ffi::param_value(h) {
            return self.param(a, e.param_id, e.channel, e.value);
        }
        if let Some((kind, n)) = crate::ffi::note(h) {
            if n.port_index > 0 {
                return;
            }
            return match kind {
                CLAP_EVENT_NOTE_ON => self.note_on(a, frame, n.id, n.channel, n.key, n.velocity * 127.0, t, out),
                CLAP_EVENT_NOTE_OFF => self.note_off(a, frame, n.id, n.channel, n.key, false),
                _ => self.choke(a, frame, n.id, n.channel, n.key),
            };
        }
        let Some(m) = crate::ffi::midi(h) else { return };
        let ch = i16::from(m[0] & 15);
        let slot = a.selected.get(ch as usize).copied().unwrap_or(0);
        match (m[0] & 0xf0, m[1], m[2]) {
            (0x90, key, v) if v > 0 => self.note_on(a, frame, -1, ch, i16::from(key & 127), f64::from(v), t, out),
            (0x80 | 0x90, key, _) => self.note_off(a, frame, -1, ch, i16::from(key & 127), true),
            (0xe0, lo, hi) => {
                let value = (i16::from(hi & 127) << 7 | i16::from(lo & 127)).wrapping_sub(8192);
                a.events.push(frame, slot, Kind::Bend { value });
            }
            (0xb0, 64, v) => {
                a.pedal = if v >= 64 {
                    a.pedal | bit(slot)
                } else {
                    a.pedal & !bit(slot)
                };
                a.events.push(frame, slot, Kind::Pedal { down: v >= 64 });
            }
            (0xb0, 120, _) => self.choke(a, frame, -1, ch, -1),
            (0xb0, 123, _) => self.note_off(a, frame, -1, ch, -1, false),
            // the others the host forwards (CC 1, 2, 4, 11, 65-67) wait for the controller sets (M7)
            _ => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn note_on(
        &self,
        a: &mut Audio,
        frame: usize,
        id: i32,
        channel: i16,
        key: i16,
        velocity: f64,
        t: u32,
        out: Option<&Out>,
    ) {
        let slot = a.selected.get(channel as usize).copied();
        let started = slot
            .filter(|_| (0..128).contains(&key))
            .and_then(|s| Some((s, a.notes.start(&self.slots, s, id, channel, key)?)));
        // a note that cannot start still ends, so the host can let go of it
        let Some((s, i)) = started else {
            if let Some(o) = out {
                o.note_end(t, id, channel, key);
            }
            return;
        };
        let velocity = velocity.clamp(1.0, 127.0) as f32;
        a.events.push(
            frame,
            s,
            Kind::On {
                id: i,
                key: key as u8,
                velocity,
            },
        );
    }

    /// Releases the held notes an address matches; raw MIDI (no note ids) the oldest held one of
    /// its key; a release of every key also lets go of what the pedal holds.
    fn note_off(&self, a: &mut Audio, frame: usize, id: i32, channel: i16, key: i16, midi: bool) {
        let Audio {
            notes, events, pedal, ..
        } = a;
        let oldest = midi.then(|| {
            notes
                .matching(id, channel, key)
                .filter(|x| x.1.held)
                .min_by_key(|x| x.1.age)
                .map(|x| x.0)
        });
        let mut slots = 0u8;
        for i in 0..NOTES as u32 {
            let Some(n) = notes.n.get_mut(i as usize) else { continue };
            let hit = n.live
                && n.held
                && (id < 0 || n.id == id)
                && (channel < 0 || n.channel == channel)
                && (key < 0 || n.key == key);
            if hit && oldest.is_none_or(|o| o == Some(i)) {
                n.held = false;
                slots |= bit(n.slot);
                events.push(frame, n.slot, Kind::Off { id: i });
            }
        }
        if key < 0 {
            for s in (0..SOUNDS as u8).filter(|&s| slots & *pedal & bit(s) != 0) {
                events.push(frame, s, Kind::Pedal { down: false });
                events.push(frame, s, Kind::Pedal { down: true });
            }
        }
    }

    fn choke(&self, a: &mut Audio, frame: usize, id: i32, channel: i16, key: i16) {
        let Audio { notes, events, .. } = a;
        for (i, n) in notes.matching(id, channel, key) {
            events.push(frame, n.slot, Kind::Choke { id: i });
        }
    }

    /// After an all-channel selection, the parameters show channel 0's sound: its values that
    /// differ go out as PARAM_VALUEs.
    fn show(&self, a: &mut Audio, out: Option<&Out>) {
        let now = a.selected[0];
        if now == a.shown {
            return;
        }
        let (Some(old), Some(new)) = (self.slots.get(usize::from(a.shown)), self.slots.get(usize::from(now))) else {
            return;
        };
        a.shown = now;
        let Some(o) = out else { return };
        for ((p, x), y) in PARAMS.iter().zip(&old.values).zip(&new.values) {
            let v = y.load(Relaxed);
            if x.load(Relaxed) != v {
                o.param_value(u32::from(p.id), f64::from(v));
            }
        }
        if old.bus.load(Relaxed) != new.bus.load(Relaxed) {
            o.param_value(BUS_ID, f64::from(new.bus.load(Relaxed)));
        }
    }

    // ---- voice budget --------------------------------------------------------------------------

    pub fn set_limits(a: &mut Audio, limit: u32, contexts: &[oscairn_voice_budget_context]) {
        let n = contexts.len().min(16);
        a.budget.0 = limit;
        a.budget.2 = n;
        for (d, s) in a.budget.1.iter_mut().zip(contexts) {
            *d = *s;
        }
    }

    /// The host's limits as the core keeps them: per sound, the entry naming it, else (selected)
    /// the entry for the selected sounds.
    fn limits(&self, a: &mut Audio) {
        let (limit, list, n) = a.budget;
        if limit == u32::MAX {
            return; // never set
        }
        let list = list.get(..n).unwrap_or(&[]);
        let mut lim = [Limits::default(); SOUNDS];
        for (l, slot) in lim.iter_mut().zip(&self.slots) {
            let id = slot.id.load(Relaxed);
            let selected = slot.channels.load(Relaxed) != 0;
            let e = list.iter().find(|c| c.context == id).or_else(|| {
                list.iter()
                    .find(|c| selected && c.context == OSCAIRN_VOICE_BUDGET_SELECTED)
            });
            if let Some(e) = e {
                let b = |x: u32| x.min(255) as u8;
                *l = Limits {
                    reserve: b(e.reserve),
                    priority: e.priority,
                    limit: b(e.limit),
                };
            }
        }
        a.engine.set_limits(limit as usize, &lim);
    }

    /// Voices sounding, and one voice's cost a block in microseconds: the running mean of the
    /// sounding voices' plans (the costliest selected sound's before any).
    pub fn active_voices(&self, a: &mut Audio) -> (u32, f64) {
        let (mut n, mut sum) = (0u32, 0f64);
        let engine = &a.engine;
        engine.voices(|s, _, _| {
            n = n.saturating_add(1);
            sum += f64::from(engine.cost(usize::from(s)));
        });
        let now = if n > 0 {
            sum / f64::from(n)
        } else {
            let sel = a.selected.iter().fold(0u8, |m, &s| m | bit(s));
            (0..SOUNDS)
                .filter(|&s| sel & bit(s as u8) != 0)
                .map(|s| f64::from(engine.cost(s)))
                .fold(0.0, f64::max)
        };
        a.cost = if a.cost == 0.0 {
            now
        } else {
            a.cost + (now - a.cost) / 16.0
        };
        (
            n,
            a.cost * f64::from(a.frames) / BLOCK as f64 / INSTRUCTIONS_A_MICROSECOND,
        )
    }

    /// Each note sounding, by the host's address, with its loudness.
    pub fn notes(&self, a: &Audio, out: &mut [oscairn_voice_budget_note]) -> u32 {
        let mut k = 0usize;
        a.engine.voices(|_, id, loud| {
            let Some(n) = a.notes.n.get(id as usize).filter(|n| n.live) else {
                return;
            };
            if let Some(o) = out.get_mut(k) {
                *o = oscairn_voice_budget_note {
                    note_id: n.id,
                    port_index: 0,
                    channel: n.channel,
                    key: n.key,
                    voices: 1,
                    loudness: loud,
                };
                k = k.saturating_add(1);
            }
        });
        k as u32
    }

    /// Bytes the instance holds (its one allocation, the tables shared).
    pub fn memory() -> u64 {
        core::mem::size_of::<Plugin>() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameter_ids_are_unique_and_the_bus_is_apart() {
        let mut ids: Vec<u32> = PARAMS.iter().map(|p| u32::from(p.id)).collect();
        ids.push(BUS_ID);
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!((ids.len(), n), (PARAM_COUNT, PARAM_COUNT));
        assert!(PARAMS.iter().all(|p| u32::from(p.id) < BUS_ID));
    }
}
