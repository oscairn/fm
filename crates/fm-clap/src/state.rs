// SPDX-License-Identifier: AGPL-3.0-or-later
//! The instance's state as text (docs/plugin.md, "State"): every sound context with its name, its
//! output bus and its sound in the core's text form, and the sound each MIDI channel selects. The
//! same text is the CLAP state, the sound-contexts snapshot and a sound saved alone. Main thread.

use fm_core::engine::SOUNDS;
use fm_core::sound::Sound;

/// The first line; a later version adds a line, never changes one.
pub const HEADER: &str = "fm-state 1";
/// A state is at most this long (eight sounds of every value, as text, fit many times over).
pub const MAX_BYTES: usize = 1 << 20;
/// Bytes a sound's text form takes at most (every one of its values written).
const SOUND_TEXT: usize = 32 << 10;

/// A sound context as saved: its name (at most 63 bytes), its output bus and its sound.
#[derive(Clone, PartialEq, Debug)]
pub struct Saved {
    pub name: String,
    pub bus: u8,
    pub sound: Sound,
}

/// A whole state: the sounds by slot (slot 0 always present) and each channel's slot.
#[derive(Clone, PartialEq, Debug)]
pub struct State {
    pub sounds: [Option<Saved>; SOUNDS],
    pub selected: [u8; 16],
}

/// The text of `s`.
pub fn write(s: &State) -> Option<Vec<u8>> {
    let mut out = format!("{HEADER}\nselected").into_bytes();
    for c in s.selected {
        out.extend_from_slice(format!(" {c}").as_bytes());
    }
    let mut text = vec![0u8; SOUND_TEXT];
    for (slot, saved) in s.sounds.iter().enumerate() {
        let Some(v) = saved else { continue };
        out.extend_from_slice(format!("\nsound {slot} bus {} {}\n", v.bus, v.name).as_bytes());
        let n = v.sound.to_text(&mut text).ok()?;
        out.extend_from_slice(text.get(..n)?);
        out.extend_from_slice(b"end");
    }
    out.push(b'\n');
    Some(out)
}

/// Reads a whole state, refusing anything malformed (an error with its line, from 1).
pub fn read(text: &str) -> Result<State, String> {
    let mut lines = text.lines().enumerate().map(|(n, l)| (n + 1, l));
    let bad = |n: usize, why: &str| format!("line {n}: {why}");
    if lines.next().map(|l| l.1) != Some(HEADER) {
        return Err(bad(1, "not an fm-state 1"));
    }
    let (n, sel) = lines.next().ok_or_else(|| bad(2, "no selection"))?;
    let mut selected = [0u8; 16];
    let mut words = sel
        .strip_prefix("selected")
        .ok_or_else(|| bad(n, "no selection"))?
        .split_whitespace();
    for c in selected.iter_mut() {
        *c = words
            .next()
            .and_then(|w| w.parse().ok())
            .ok_or_else(|| bad(n, "a channel's sound"))?;
    }
    if words.next().is_some() {
        return Err(bad(n, "more than 16 channels"));
    }
    let mut sounds: [Option<Saved>; SOUNDS] = Default::default();
    while let Some((n, line)) = lines.next() {
        let mut w = line.splitn(5, ' ');
        let (Some("sound"), Some(slot), Some("bus"), Some(bus), name) =
            (w.next(), w.next(), w.next(), w.next(), w.next())
        else {
            return Err(bad(n, "not a sound"));
        };
        let name = name.unwrap_or("").to_string();
        let slot: usize = slot.parse().map_err(|_| bad(n, "a slot"))?;
        let bus: u8 = bus
            .parse()
            .ok()
            .filter(|&b| b <= 8)
            .ok_or_else(|| bad(n, "an output bus"))?;
        if name.len() > 63 || name.contains('\0') {
            return Err(bad(n, "a name"));
        }
        let mut body = String::new();
        loop {
            let (m, l) = lines.next().ok_or_else(|| bad(n, "a sound without its end"))?;
            if l == "end" {
                break;
            }
            body.push_str(l);
            body.push('\n');
            if body.len() > SOUND_TEXT {
                return Err(bad(m, "a sound too long"));
            }
        }
        let sound = Sound::from_text(&body).map_err(|(m, e)| bad(n + m, &format!("{e:?}")))?;
        match sounds.get_mut(slot) {
            Some(s @ None) => *s = Some(Saved { name, bus, sound }),
            _ => return Err(bad(n, "a slot past the last, or twice")),
        }
    }
    if sounds.first().is_none_or(Option::is_none) {
        return Err("no sound in slot 0".into());
    }
    if selected
        .iter()
        .any(|&c| sounds.get(usize::from(c)).is_none_or(Option::is_none))
    {
        return Err("a channel selects no sound".into());
    }
    Ok(State { sounds, selected })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_refusals() {
        let mut sounds: [Option<Saved>; SOUNDS] = Default::default();
        let lead = Sound::from_text("fm-sound/1\nalgorithm=12 feedback=5\nop1.level=80\n").unwrap();
        sounds[0] = Some(Saved {
            name: "Default".into(),
            bus: 0,
            sound: Sound::default(),
        });
        sounds[3] = Some(Saved {
            name: "A name with spaces".into(),
            bus: 2,
            sound: lead,
        });
        let mut selected = [0; 16];
        selected[5] = 3;
        let s = State { sounds, selected };
        let text = String::from_utf8(write(&s).unwrap()).unwrap();
        assert_eq!(read(&text), Ok(s.clone()));
        assert_eq!(write(&read(&text).unwrap()).unwrap(), text.as_bytes(), "deterministic");
        for broken in [
            text.replace("fm-state 1", "fm-state 2"),
            text.replace("selected 0 0 0 0 0 3", "selected 0 0 0 0 0 4"),
            text.replace("bus 2", "bus 9"),
            text.replace("end\n", ""),
            text.replace("op1.level=80", "op1.level=800"),
            text.replace("sound 3", "sound 0"),
            text.replace("sound 0 bus 0 Default\nfm-sound/1\nend\n", ""),
        ] {
            assert!(read(&broken).is_err(), "{broken}");
        }
    }
}
