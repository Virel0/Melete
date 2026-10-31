//! The standard's penalty score for a masked code: the mask with the lowest is used, so the
//! code has no large blocks of one colour, nothing a scanner could take for a finder pattern,
//! and about as much dark as light.

use super::QrCode;

/// Five or more modules of one colour in a row or column: 3, and 1 more for each beyond five.
const RUN: u32 = 3;
/// Each 2 × 2 block of one colour.
const BLOCK: u32 = 3;
/// Each dark-light-dark-dark-dark-light-dark pattern with four light modules on one side.
const FINDER_LIKE: u32 = 40;
/// Each 5% the share of dark modules is away from half.
const BALANCE: u32 = 10;

/// The total penalty of `code` as it's masked.
pub(super) fn penalty(code: &QrCode) -> u32 {
    let size = code.size;
    let mut score = 0;
    for i in 0..size {
        score += line_penalty(size, |j| code.dark(i, j));
        score += line_penalty(size, |j| code.dark(j, i));
    }
    for row in 0..size - 1 {
        for col in 0..size - 1 {
            let colour = code.dark(row, col);
            if colour == code.dark(row, col + 1)
                && colour == code.dark(row + 1, col)
                && colour == code.dark(row + 1, col + 1)
            {
                score += BLOCK;
            }
        }
    }
    let dark = (0..size * size)
        .filter(|&i| code.dark(i / size, i % size))
        .count() as i64;
    let total = (size * size) as i64;
    let away = (((dark * 20 - total * 10).abs() + total - 1) / total - 1).max(0);
    score + away as u32 * BALANCE
}

/// The run and finder-like penalties along one row or column, `dark(j)` giving its modules.
/// The light margin around the code counts as light.
pub(super) fn line_penalty(size: usize, dark: impl Fn(usize) -> bool) -> u32 {
    let mut score = 0;
    let mut runs = Runs {
        size,
        history: [0; 7],
    };
    let mut colour = false;
    let mut length = 0;
    for j in 0..size {
        if dark(j) == colour {
            length += 1;
            if length == 5 {
                score += RUN;
            } else if length > 5 {
                score += 1;
            }
        } else {
            runs.push(length);
            if !colour {
                score += runs.finder_like() * FINDER_LIKE;
            }
            colour = dark(j);
            length = 1;
        }
    }
    if colour {
        runs.push(length);
        length = 0;
    }
    runs.push(length + size);
    score + runs.finder_like() * FINDER_LIKE
}

/// The last seven runs' lengths, newest first.
struct Runs {
    size: usize,
    history: [usize; 7],
}

impl Runs {
    fn push(&mut self, mut length: usize) {
        // The first run starts in the margin.
        if self.history[0] == 0 {
            length += self.size;
        }
        self.history.copy_within(0..6, 1);
        self.history[0] = length;
    }

    /// How many finder-like patterns end at the newest light run: 1:1:3:1:1, with light at
    /// least four times the unit on one side.
    fn finder_like(&self) -> u32 {
        let h = &self.history;
        let n = h[1];
        let core = n > 0 && h[2] == n && h[3] == n * 3 && h[4] == n && h[5] == n;
        u32::from(core && h[0] >= n * 4 && h[6] >= n)
            + u32::from(core && h[6] >= n * 4 && h[0] >= n)
    }
}
