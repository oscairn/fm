// SPDX-License-Identifier: AGPL-3.0-or-later
//! A sound: every parameter's raw value (docs/sound-format.md), its compact binary form and its
//! text form, and the validation both share. No allocation and no formatting: the caller gives
//! the buffers.

use crate::params::{index_of, Param, COUNT, OPS, PARAMS};

/// Raw values, one per entry of [`PARAMS`], always valid.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Sound {
    v: [i16; COUNT],
}

/// Why a blob or a text was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Wrong magic, version, flags or reserved bytes.
    Header,
    /// The size field disagrees with the blob, or the buffer is too small to write into.
    Size,
    Crc,
    /// Ids not strictly increasing, or one given twice.
    Order,
    Unknown,
    Range,
    /// The cutoff scaling break points are not strictly ascending.
    Breaks,
    /// Text that is not `name=value` items (or a missing first line).
    Syntax,
}

pub const MAGIC: [u8; 4] = *b"OFMS";
pub const VERSION: u16 = 1;
const HEADER: usize = 20;
const TEXT_HEADER: &str = "fm-sound/1";
/// Index of the first cutoff break point (four in a row).
const BREAKS: usize = 41 + 26;

impl Default for Sound {
    fn default() -> Self {
        let mut v = [0; COUNT];
        for (x, p) in v.iter_mut().zip(PARAMS.iter()) {
            *x = p.default;
        }
        Sound { v }
    }
}

/// CRC-32 (IEEE, reflected), bitwise: sounds are small and this runs off the audio thread.
fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = (c >> 1) ^ (0xedb8_8320 & (c & 1).wrapping_neg());
        }
    }
    !c
}

impl Sound {
    /// The raw value at `index` in [`PARAMS`] (0 past the end).
    pub fn get(&self, index: usize) -> i16 {
        self.v.get(index).copied().unwrap_or(0)
    }

    /// Sets the parameter with `id`, refusing unknown ids and values out of its range.
    pub fn set(&mut self, id: u16, value: i16) -> Result<(), Error> {
        let i = index_of(id).ok_or(Error::Unknown)?;
        let (Some(p), Some(x)) = (PARAMS.get(i), self.v.get_mut(i)) else {
            return Err(Error::Unknown);
        };
        if value < p.min || value > p.max {
            return Err(Error::Range);
        }
        *x = value;
        Ok(())
    }

    /// The parameters at non-default values, with their values, in id order.
    pub fn changed(&self) -> impl Iterator<Item = (&'static Param, i16)> + '_ {
        PARAMS
            .iter()
            .zip(self.v.iter())
            .filter(|(p, &v)| v != p.default)
            .map(|(p, &v)| (p, v))
    }

    fn check(&self) -> Result<(), Error> {
        let b = self.v.get(BREAKS..BREAKS + 4).ok_or(Error::Breaks)?;
        if b.windows(2).all(|w| matches!(w, [a, c] if a < c)) {
            Ok(())
        } else {
            Err(Error::Breaks)
        }
    }

    /// The binary form's length.
    pub fn blob_len(&self) -> usize {
        HEADER + 4 * self.changed().count()
    }

    /// Writes the binary form into `out`; returns its length.
    ///
    /// Layout (little-endian): `OFMS`, u16 version, u16 flags (0), u32 size of the whole blob,
    /// u32 CRC-32 of the bytes from offset 16 to the end, u16 count, u16 0, then count x (u16 id,
    /// i16 value), ids strictly increasing, only values that differ from the default.
    pub fn to_blob(&self, out: &mut [u8]) -> Result<usize, Error> {
        let len = self.blob_len();
        let out = out.get_mut(..len).ok_or(Error::Size)?;
        out.fill(0);
        {
            let mut put = |o: usize, b: &[u8]| {
                if let Some(d) = out.get_mut(o..o + b.len()) {
                    d.copy_from_slice(b)
                }
            };
            put(0, &MAGIC);
            put(4, &VERSION.to_le_bytes());
            put(8, &(len as u32).to_le_bytes());
            put(16, &(((len - HEADER) / 4) as u16).to_le_bytes());
            for (k, (p, v)) in self.changed().enumerate() {
                put(HEADER + 4 * k, &p.id.to_le_bytes());
                put(HEADER + 4 * k + 2, &v.to_le_bytes());
            }
        }
        let crc = crc32(out.get(16..).unwrap_or(&[]));
        if let Some(c) = out.get_mut(12..16) {
            c.copy_from_slice(&crc.to_le_bytes());
        }
        Ok(len)
    }

    /// Reads and validates a binary form.
    pub fn from_blob(b: &[u8]) -> Result<Sound, Error> {
        let u16_at = |o: usize| b.get(o..o + 2).and_then(|x| x.try_into().ok()).map(u16::from_le_bytes);
        let u32_at = |o: usize| b.get(o..o + 4).and_then(|x| x.try_into().ok()).map(u32::from_le_bytes);
        if b.get(..4) != Some(&MAGIC[..]) || u16_at(4) != Some(VERSION) || u16_at(6) != Some(0) || u16_at(18) != Some(0)
        {
            return Err(Error::Header);
        }
        let count = usize::from(u16_at(16).ok_or(Error::Size)?);
        if u32_at(8).map(|s| s as usize) != Some(b.len()) || b.len() != HEADER + 4 * count {
            return Err(Error::Size);
        }
        if u32_at(12) != Some(crc32(b.get(16..).unwrap_or(&[]))) {
            return Err(Error::Crc);
        }
        let mut s = Sound::default();
        let mut last = None;
        for item in b.get(HEADER..).unwrap_or(&[]).chunks_exact(4) {
            let &[a, b, c, d] = item else { continue };
            let (id, v) = (u16::from_le_bytes([a, b]), i16::from_le_bytes([c, d]));
            if last.is_some_and(|l| id <= l) {
                return Err(Error::Order);
            }
            last = Some(id);
            s.set(id, v)?;
        }
        s.check()?;
        Ok(s)
    }

    /// Reads and validates the text form: a first line `fm-sound/1`, then `name=value` items
    /// separated by spaces or new lines, operator parameters as `op1.name` to `op8.name`; a line
    /// starting with `#` is a comment. On an error, also the line it is on (from 1).
    pub fn from_text(t: &str) -> Result<Sound, (usize, Error)> {
        let mut lines = t.lines();
        if lines.next().map(str::trim) != Some(TEXT_HEADER) {
            return Err((1, Error::Syntax));
        }
        let mut s = Sound::default();
        let mut seen = [false; COUNT];
        for (n, line) in lines.enumerate() {
            let at = |e| (n.saturating_add(2), e);
            if line.trim_start().starts_with('#') {
                continue;
            }
            for item in line.split_whitespace() {
                let (name, value) = item.split_once('=').ok_or(at(Error::Syntax))?;
                let value: i16 = value.parse().map_err(|_| at(Error::Syntax))?;
                let (op, name) = match name.strip_prefix("op").and_then(|r| r.split_once('.')) {
                    Some((o, rest)) => match o.parse::<u8>() {
                        Ok(o @ 1..=8) => (Some(o - 1), rest),
                        _ => return Err(at(Error::Unknown)),
                    },
                    None => (None, name),
                };
                let i = PARAMS
                    .iter()
                    .position(|p| p.name == name && p.op == op)
                    .ok_or(at(Error::Unknown))?;
                match seen.get_mut(i) {
                    Some(f) if !*f => *f = true,
                    _ => return Err(at(Error::Order)),
                }
                let id = PARAMS.get(i).map_or(0, |p| p.id);
                s.set(id, value).map_err(at)?;
            }
        }
        s.check().map_err(|e| (0, e))?;
        Ok(s)
    }

    /// Writes the text form into `out` (part-wide values on one line, each operator's on its
    /// own); returns its length.
    pub fn to_text(&self, out: &mut [u8]) -> Result<usize, Error> {
        let mut w = Writer { out, n: 0 };
        w.put(TEXT_HEADER.as_bytes());
        for op in core::iter::once(None).chain((0..OPS as u8).map(Some)) {
            let mut first = true;
            for (p, v) in self.changed().filter(|(p, _)| p.op == op) {
                w.put(if first { b"\n" } else { b" " });
                first = false;
                if let Some(o) = op {
                    w.put(b"op");
                    w.int(i32::from(o) + 1);
                    w.put(b".");
                }
                w.put(p.name.as_bytes());
                w.put(b"=");
                w.int(i32::from(v));
            }
        }
        w.put(b"\n");
        if w.n > w.out.len() {
            Err(Error::Size)
        } else {
            Ok(w.n)
        }
    }
}

/// Bytes into a fixed buffer; counts past its end so the caller learns it was too small.
struct Writer<'a> {
    out: &'a mut [u8],
    n: usize,
}

impl Writer<'_> {
    fn put(&mut self, b: &[u8]) {
        for &c in b {
            if let Some(o) = self.out.get_mut(self.n) {
                *o = c;
            }
            self.n = self.n.saturating_add(1);
        }
    }

    fn int(&mut self, v: i32) {
        if v < 0 {
            self.put(b"-");
        }
        let mut digits = [0u8; 10];
        let mut m = v.unsigned_abs();
        let mut k = digits.len();
        loop {
            k -= 1;
            if let Some(d) = digits.get_mut(k) {
                *d = b'0' + (m % 10) as u8;
            }
            m /= 10;
            if m == 0 || k == 0 {
                break;
            }
        }
        self.put(digits.get(k..).unwrap_or(&[]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "fm-sound/1\n# a comment\nalgorithm=12 feedback=5 velocity_depth=90\n\
                        op1.level=99 op1.coarse=2 op1.eg_time_decay1=40\nop8.level=70 op8.detune=-7\n";

    fn sample() -> Sound {
        Sound::from_text(TEXT).unwrap()
    }

    #[test]
    fn text_and_blob_round_trip() {
        let s = sample();
        assert_eq!(s.get(crate::params::Common::Algorithm as usize), 12);
        let mut t = [0u8; 4096];
        let n = s.to_text(&mut t).unwrap();
        let text = core::str::from_utf8(&t[..n]).unwrap();
        assert_eq!(
            text,
            "fm-sound/1\nalgorithm=12 feedback=5 velocity_depth=90\nop1.coarse=2 op1.eg_time_decay1=40 op1.level=99\n\
             op8.detune=-7 op8.level=70\n"
        );
        assert_eq!(Sound::from_text(text).unwrap(), s);
        let mut b = [0u8; 256];
        let n = s.to_blob(&mut b).unwrap();
        assert_eq!(n, 20 + 4 * 8);
        assert_eq!(Sound::from_blob(&b[..n]).unwrap(), s);
        // the default sound is the bare header
        let n = Sound::default().to_blob(&mut b).unwrap();
        assert_eq!((n, Sound::from_blob(&b[..n]).unwrap()), (20, Sound::default()));
        assert_eq!(s.to_blob(&mut b[..30]), Err(Error::Size));
        assert_eq!(s.to_text(&mut t[..30]), Err(Error::Size));
    }

    #[test]
    fn every_single_byte_corruption_and_truncation_is_refused() {
        let mut b = [0u8; 256];
        let n = sample().to_blob(&mut b).unwrap();
        let blob = &b[..n];
        for i in 0..n {
            for x in 1..=255u8 {
                let mut c = blob.to_vec();
                c[i] ^= x;
                assert!(Sound::from_blob(&c).is_err(), "byte {i} ^ {x:#x} accepted");
            }
        }
        for len in 0..n {
            assert!(Sound::from_blob(&blob[..len]).is_err(), "truncated to {len} accepted");
        }
        let mut longer = blob.to_vec();
        longer.push(0);
        assert!(Sound::from_blob(&longer).is_err());
    }

    /// A blob with a valid CRC but a bad value inside, to check the checks behind the CRC.
    fn forged(items: &[(u16, i16)]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&MAGIC);
        b.extend_from_slice(&[1, 0, 0, 0]);
        b.extend_from_slice(&((20 + 4 * items.len()) as u32).to_le_bytes());
        b.extend_from_slice(&[0; 4]);
        b.extend_from_slice(&(items.len() as u16).to_le_bytes());
        b.extend_from_slice(&[0, 0]);
        for (id, v) in items {
            b.extend_from_slice(&id.to_le_bytes());
            b.extend_from_slice(&v.to_le_bytes());
        }
        let crc = crc32(&b[16..]);
        b[12..16].copy_from_slice(&crc.to_le_bytes());
        b
    }

    #[test]
    fn values_are_validated() {
        assert!(Sound::from_blob(&forged(&[(30, 12), (31, 5)])).is_ok());
        assert_eq!(Sound::from_blob(&forged(&[(30, 0)])), Err(Error::Range));
        assert_eq!(Sound::from_blob(&forged(&[(30, 89)])), Err(Error::Range));
        assert_eq!(Sound::from_blob(&forged(&[(41, 0)])), Err(Error::Unknown));
        assert_eq!(Sound::from_blob(&forged(&[(31, 5), (30, 12)])), Err(Error::Order));
        assert_eq!(Sound::from_blob(&forged(&[(30, 12), (30, 13)])), Err(Error::Order));
        assert_eq!(Sound::from_blob(&forged(&[(127, 60)])), Err(Error::Breaks));
        assert_eq!(Sound::from_blob(&forged(&[(1764, 100)])), Err(Error::Range));
        assert_eq!(Sound::from_text("fm-sound/1\nop9.level=1\n"), Err((2, Error::Unknown)));
        assert_eq!(
            Sound::from_text("fm-sound/1\n\nop1.level=100\n"),
            Err((3, Error::Range))
        );
        assert_eq!(Sound::from_text("fm-sound/1\nlevel=1\n"), Err((2, Error::Unknown)));
        assert_eq!(
            Sound::from_text("fm-sound/1\nfeedback=1 feedback=2\n"),
            Err((2, Error::Order))
        );
        assert_eq!(Sound::from_text("fm-sound/1\nfeedback\n"), Err((2, Error::Syntax)));
        assert_eq!(Sound::from_text("fm-sound/2\n"), Err((1, Error::Syntax)));
        assert_eq!(
            Sound::from_text("fm-sound/1\ncutoff_break2=30\n"),
            Err((0, Error::Breaks))
        );
    }
}
