// SPDX-License-Identifier: AGPL-3.0-or-later
//! A sound's plan (docs/design.md, "Plans"): which operators can be heard at all, which modulate
//! which among those, and the feedback path's kind, decided once per sound so the block path renders
//! only what can be heard and a sound that uses less costs less; each operator's form is in the
//! plan, so a sound pays only for the forms it uses.

use crate::forms::Wave;
use crate::params::OPS;
use crate::routing::Routing;

/// The feedback path as the kernel renders it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feedback {
    None,
    /// An operator into itself.
    Own(u8),
    /// From operator `from` back into operator `to` (`to < from`) through the operators between:
    /// those are rendered interleaved, sample by sample.
    Loop {
        to: u8,
        from: u8,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    /// Operators rendered: audible, and a carrier or modulating one that is rendered.
    pub live: u8,
    /// Live carriers.
    pub carriers: u8,
    /// Per operator, its live modulators.
    pub mods: [u8; OPS],
    pub feedback: Feedback,
    /// Per operator, how its form is played (a sine where not live).
    pub waves: [Wave; OPS],
    /// A live operator is not a sine.
    pub forms: bool,
    /// A feedback loop through operators of which one is not a sine.
    pub loop_forms: bool,
}

/// Instructions a voice-block (callgrind, aarch64, Cortex-A53): the fixed part and each rendered
/// operator, from the cost cases in tools/cost; an estimate for the voice budget, not a bound.
const VOICE_COST: u32 = 600;
const OP_COST: u32 = 2300;
const LOOP_OP_COST: u32 = 900;
const TABLE_COST: u32 = 300;
const TWO_READ_COST: u32 = 1100;

impl Plan {
    /// `audible`: operators whose level and envelope levels are not all silent.
    pub fn new(r: &Routing, audible: u8, feedback: bool, waves: [Wave; OPS]) -> Plan {
        let mut live = 0u8;
        for i in (0..OPS).rev() {
            let bit = 1u8 << i;
            let feeds_live = r
                .mods
                .iter()
                .zip(0..OPS)
                .any(|(&m, j)| j > i && m & bit != 0 && live >> j & 1 != 0);
            if audible & bit != 0 && (r.carriers & bit != 0 || feeds_live) {
                live |= bit;
            }
        }
        let on = |op: u8| live.checked_shr(u32::from(op)).is_some_and(|m| m & 1 != 0);
        let feedback = match (feedback && on(r.fb_from) && on(r.fb_to), r.fb_from == r.fb_to) {
            (false, _) => Feedback::None,
            (true, true) => Feedback::Own(r.fb_from),
            (true, false) => Feedback::Loop {
                to: r.fb_to,
                from: r.fb_from,
            },
        };
        let waves: [Wave; OPS] = core::array::from_fn(|i| {
            if on(i as u8) {
                waves.get(i).copied().unwrap_or(Wave::Sine)
            } else {
                Wave::Sine
            }
        });
        let loop_forms = match feedback {
            Feedback::Loop { to, from } => waves
                .iter()
                .skip(usize::from(to))
                .take(usize::from(from.saturating_sub(to)) + 1)
                .any(|&w| w != Wave::Sine),
            _ => false,
        };
        Plan {
            live,
            carriers: r.carriers & live,
            mods: r.mods.map(|m| m & live),
            feedback,
            forms: waves.iter().any(|&w| w != Wave::Sine),
            waves,
            loop_forms,
        }
    }

    /// The estimated cost of one voice a block, in instructions.
    pub fn cost(&self) -> u32 {
        let loop_ops = match self.feedback {
            Feedback::Loop { to, from } => u32::from(from.saturating_sub(to)) + 1,
            _ => 0,
        };
        let forms = self.waves.iter().fold(0, |c, w| match w {
            Wave::Sine => c,
            Wave::Table(_) => c + TABLE_COST,
            _ => c + TWO_READ_COST,
        });
        VOICE_COST + OP_COST * self.live.count_ones() + LOOP_OP_COST * loop_ops + forms
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::{routing, ROUTINGS};

    const SINES: [Wave; OPS] = [Wave::Sine; OPS];

    #[test]
    fn only_what_can_be_heard_is_rendered() {
        // routing 2: 1 > 2 > 3 > 4, carriers 4-8; operator 1 feeds back on itself
        let r = routing(2);
        let all = Plan::new(&r, 0xff, true, SINES);
        assert_eq!((all.live, all.carriers, all.feedback), (0xff, 0xf8, Feedback::Own(0)));
        // operator 3 silent: 1 and 2 are not heard either
        let p = Plan::new(&r, !4, true, SINES);
        assert_eq!((p.live, p.mods[3], p.feedback), (0xf8, 0, Feedback::None));
        // only operator 8 (the model's init sound)
        let p = Plan::new(&routing(69), 0x80, true, SINES);
        assert_eq!((p.live, p.carriers), (0x80, 0x80));
        assert!(p.cost() < all.cost());
        assert_eq!(Plan::new(&r, 0xff, false, SINES).feedback, Feedback::None);
        assert_eq!(
            Plan::new(&routing(12), 0xff, true, SINES).feedback,
            Feedback::Loop { to: 2, from: 4 }
        );
    }

    #[test]
    fn loops_end_on_a_carrier() {
        // the liveness pass runs top-down, which is complete only if no operator is heard through
        // the feedback path alone
        for r in ROUTINGS.iter().filter(|r| r.fb_from != r.fb_to) {
            assert_ne!(r.carriers >> r.fb_from & 1, 0);
        }
    }
}
