//! How well two records suit each other: the five factors, their weighted total, and the length
//! that total earns.

use super::*;

/// How well two records suit each other: one number and the five it's made of.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MixScore {
    /// How close the tempos are without bending: 1 within a 2% bend, falling to 0 at the stretch
    /// limit.
    pub tempo: f64,
    /// 1 for compatible keys, 0 for a clash, 0.5 when either is unknown.
    pub key: f64,
    /// 1 when neither record sings over the mix, 0.6 when one or both do, 0.5 when unknown.
    pub vocals: f64,
    /// How close the energy is where the mix leaves and enters, 1 for the same; 0.5 when unknown.
    pub energy: f64,
    /// How well the sections it leaves and enters go together.
    pub sections: f64,
}

/// Where a bend stops being free.
pub(super) const EASY_BEND: f64 = 0.02;

impl MixScore {
    /// The weighted total, in the clients' order of operations.
    pub fn total(&self) -> f64 {
        self.tempo * 0.2
            + self.key * 0.2
            + self.vocals * 0.15
            + self.energy * 0.2
            + self.sections * 0.25
    }

    /// How long the pair should run together, from the score alone; None earns nothing.
    pub(super) fn bars(&self) -> Option<u32> {
        let total = self.total();
        if total >= 0.75 {
            Some(32)
        } else if total >= 0.55 {
            Some(16)
        } else if total >= 0.35 {
            Some(8)
        } else {
            None
        }
    }

    /// The tempo factor for bending a record by `rate`: 1 up to a 2% bend, then falling in a
    /// straight line to 0 at [`MAXIMUM_STRETCH`].
    pub fn tempo_factor(rate: f64) -> f64 {
        let bend = (rate - 1.0).abs();
        if bend <= EASY_BEND + 1e-9 {
            return 1.0;
        }
        (1.0 - (bend - EASY_BEND) / (MAXIMUM_STRETCH - EASY_BEND)).max(0.0)
    }

    /// The key factor: 1 for compatible Camelot keys, 0 for a clash, 0.5 when either is unknown.
    pub fn key_factor(a: Option<&str>, b: Option<&str>) -> f64 {
        match (a, b) {
            (Some(a), Some(b)) if compatible(Some(a), Some(b)) => 1.0,
            (Some(_), Some(_)) => 0.0,
            _ => 0.5,
        }
    }

    pub(super) fn vocals_factor(outgoing_sings: Option<bool>, incoming_sings: Option<bool>) -> f64 {
        match (outgoing_sings, incoming_sings) {
            (Some(false), Some(false)) => 1.0,
            (Some(_), Some(_)) => 0.6,
            _ => 0.5,
        }
    }

    pub(super) fn energy_factor(exit: Option<f64>, entry: Option<f64>) -> f64 {
        match (exit, entry) {
            (Some(from), Some(to)) => (1.0 - (from - to).abs()).max(0.0),
            _ => 0.5,
        }
    }

    pub(super) fn sections_factor(exit: Option<&str>, entry: Option<&str>) -> f64 {
        let (Some(exit), Some(entry)) = (exit, entry) else {
            return 0.5;
        };
        let leaving = match exit {
            "outro" | "breakdown" => 1.0,
            "build" | "verse" | "chorus" | "intro" => 0.7,
            "drop" => 0.4,
            _ => 0.6,
        };
        let arriving = match entry {
            "build" | "intro" | "breakdown" => 1.0,
            "verse" | "chorus" | "outro" => 0.7,
            "drop" => 0.2,
            _ => 0.6,
        };
        (leaving + arriving) / 2.0
    }
}
