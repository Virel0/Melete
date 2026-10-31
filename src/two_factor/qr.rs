//! A QR code of the setup link, so a phone's authenticator app can scan it: drawn here, from
//! ISO/IEC 18004, so the secret it holds never goes to anything outside the server to be drawn.
//!
//! Only what that link needs: byte mode, error correction level M (15% of the code can be
//! damaged and it still reads), versions 1 to 10 (up to 213 bytes, where a setup link is at
//! most about 175). The mask is chosen by the standard's penalty rules.

/// The largest version made: 57 × 57 modules.
const MAX_VERSION: usize = 10;

/// Level M's error correction for each version, 1 to 10: codewords of correction per block,
/// and the blocks as (how many, data codewords in each).
const BLOCKS: [(usize, [(usize, usize); 2]); MAX_VERSION] = [
    (10, [(1, 16), (0, 0)]),
    (16, [(1, 28), (0, 0)]),
    (26, [(1, 44), (0, 0)]),
    (18, [(2, 32), (0, 0)]),
    (24, [(2, 43), (0, 0)]),
    (16, [(4, 27), (0, 0)]),
    (18, [(4, 31), (0, 0)]),
    (22, [(2, 38), (2, 39)]),
    (22, [(3, 36), (2, 37)]),
    (26, [(4, 43), (1, 44)]),
];

/// The centres of the alignment patterns along each axis, for each version (none in version 1).
const ALIGNMENT: [&[usize]; MAX_VERSION] = [
    &[],
    &[6, 18],
    &[6, 22],
    &[6, 26],
    &[6, 30],
    &[6, 34],
    &[6, 22, 38],
    &[6, 24, 42],
    &[6, 26, 46],
    &[6, 28, 50],
];

/// A drawn code: `size` × `size` modules, row by row, true for dark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrCode {
    /// Its version, 1 to 10.
    pub version: usize,
    /// Modules along each side: 17 + 4 × version.
    pub size: usize,
    /// The mask chosen, 0 to 7.
    pub mask: u8,
    modules: Vec<bool>,
    /// Finder, timing, alignment, format and version modules, which masks leave alone.
    function: Vec<bool>,
}

impl QrCode {
    /// Whether the module at `row`, `col` is dark.
    pub fn dark(&self, row: usize, col: usize) -> bool {
        self.modules[row * self.size + col]
    }

    fn set(&mut self, row: usize, col: usize, dark: bool) {
        self.modules[row * self.size + col] = dark;
        self.function[row * self.size + col] = true;
    }

    /// The code as an SVG image, dark on light whatever the page's colours, with the four
    /// modules of light margin around it that scanners need. Scales to whatever box holds it.
    pub fn svg(&self) -> String {
        let side = self.size + 8;
        let mut path = String::new();
        for row in 0..self.size {
            let mut col = 0;
            while col < self.size {
                if !self.dark(row, col) {
                    col += 1;
                    continue;
                }
                let start = col;
                while col < self.size && self.dark(row, col) {
                    col += 1;
                }
                path.push_str(&format!(
                    "M{},{}h{}v1h-{}z",
                    start + 4,
                    row + 4,
                    col - start,
                    col - start
                ));
            }
        }
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {side} {side}\" shape-rendering=\"crispEdges\">\
             <rect width=\"{side}\" height=\"{side}\" fill=\"#fff\"/><path fill=\"#000\" d=\"{path}\"/></svg>"
        )
    }
}

/// The smallest code at level M that holds `data`, or None when it's too long for version 10.
pub fn encode(data: &[u8]) -> Option<QrCode> {
    let unmasked = unmasked(data)?;
    let best = (0..8)
        .min_by_key(|&mask| penalty(&with_mask(unmasked.clone(), mask)))
        .expect("there are eight masks");
    Some(with_mask(unmasked, best))
}

/// The smallest code that holds `data`, its data placed but not yet masked.
fn unmasked(data: &[u8]) -> Option<QrCode> {
    let version = (1..=MAX_VERSION).find(|&v| data.len() <= capacity(v))?;
    let codewords = with_correction(version, &data_codewords(version, data));
    let size = 17 + 4 * version;
    let mut code = QrCode {
        version,
        size,
        mask: 0,
        modules: vec![false; size * size],
        function: vec![false; size * size],
    };
    draw_function_patterns(&mut code);
    place_data(&mut code, &codewords);
    Some(code)
}

/// `code` masked with `mask`, and its format information saying so.
fn with_mask(mut code: QrCode, mask: u8) -> QrCode {
    apply_mask(&mut code, mask);
    draw_format(&mut code, mask);
    code.mask = mask;
    code
}

/// Data codewords in `version` at level M.
fn data_capacity(version: usize) -> usize {
    BLOCKS[version - 1].1.iter().map(|(n, k)| n * k).sum()
}

/// Bits of the length field in byte mode: 8 up to version 9, 16 from 10.
fn count_bits(version: usize) -> usize {
    if version <= 9 { 8 } else { 16 }
}

/// The most bytes `version` holds in byte mode.
fn capacity(version: usize) -> usize {
    (data_capacity(version) * 8 - 4 - count_bits(version)) / 8
}

/// Appends the `count` low bits of `value`, most significant first.
fn push_bits(bits: &mut Vec<bool>, value: usize, count: usize) {
    bits.extend((0..count).rev().map(|i| (value >> i) & 1 == 1));
}

/// The data codewords: byte mode, the length, the bytes, a terminator, then padding.
fn data_codewords(version: usize, data: &[u8]) -> Vec<u8> {
    let room = data_capacity(version) * 8;
    let mut bits = Vec::with_capacity(room);
    push_bits(&mut bits, 0b0100, 4);
    push_bits(&mut bits, data.len(), count_bits(version));
    for &byte in data {
        push_bits(&mut bits, byte.into(), 8);
    }
    let terminator = (room - bits.len()).min(4);
    push_bits(&mut bits, 0, terminator);
    let to_byte = (8 - bits.len() % 8) % 8;
    push_bits(&mut bits, 0, to_byte);
    let mut words: Vec<u8> = bits
        .chunks(8)
        .map(|byte| {
            byte.iter()
                .fold(0u8, |acc, &bit| (acc << 1) | u8::from(bit))
        })
        .collect();
    for pad in [0xEC, 0x11].into_iter().cycle() {
        if words.len() == room / 8 {
            break;
        }
        words.push(pad);
    }
    words
}

/// Multiplication in GF(256) with the standard's polynomial, x⁸ + x⁴ + x³ + x² + 1.
fn gf_mul(a: u8, b: u8) -> u8 {
    let mut product = 0u8;
    for i in (0..8).rev() {
        let carry = product & 0x80 != 0;
        product <<= 1;
        if carry {
            product ^= 0x1D;
        }
        if (b >> i) & 1 == 1 {
            product ^= a;
        }
    }
    product
}

/// The Reed-Solomon correction codewords for `data`, `count` of them.
pub(super) fn correction(data: &[u8], count: usize) -> Vec<u8> {
    // The generator polynomial (x - α⁰)(x - α¹)…(x - α^(count-1)), highest power left out.
    let mut generator = vec![0u8; count];
    generator[count - 1] = 1;
    let mut root = 1u8;
    for _ in 0..count {
        for j in 0..count {
            generator[j] = gf_mul(generator[j], root);
            if j + 1 < count {
                generator[j] ^= generator[j + 1];
            }
        }
        root = gf_mul(root, 2);
    }
    let mut remainder = vec![0u8; count];
    for &byte in data {
        let factor = byte ^ remainder.remove(0);
        remainder.push(0);
        for (r, g) in remainder.iter_mut().zip(&generator) {
            *r ^= gf_mul(*g, factor);
        }
    }
    remainder
}

/// The codewords in the order they're placed: the blocks' data interleaved, then their
/// correction interleaved.
fn with_correction(version: usize, data: &[u8]) -> Vec<u8> {
    let (ec, groups) = BLOCKS[version - 1];
    let mut blocks = Vec::new();
    let mut at = 0;
    for &(count, len) in &groups {
        for _ in 0..count {
            blocks.push(&data[at..at + len]);
            at += len;
        }
    }
    let corrections: Vec<Vec<u8>> = blocks.iter().map(|b| correction(b, ec)).collect();
    let longest = blocks.iter().map(|b| b.len()).max().unwrap_or(0);
    let mut out = Vec::new();
    for i in 0..longest {
        out.extend(blocks.iter().filter_map(|b| b.get(i)));
    }
    for i in 0..ec {
        out.extend(corrections.iter().map(|c| c[i]));
    }
    out
}

/// The finder patterns with their separators, the timing patterns, the alignment patterns,
/// the dark module, the version information, and the format areas kept free.
fn draw_function_patterns(code: &mut QrCode) {
    let size = code.size;
    for i in 0..size {
        code.set(6, i, i % 2 == 0);
        code.set(i, 6, i % 2 == 0);
    }
    for (row, col) in [(3, 3), (3, size - 4), (size - 4, 3)] {
        draw_square(code, row, col, 4, |ring| ring != 2 && ring != 4);
    }
    let centres = ALIGNMENT[code.version - 1];
    let last = centres.last().copied();
    for &row in centres {
        for &col in centres {
            // Those that would sit on a finder pattern are left out.
            let on_finder = (row == 6 || Some(row) == last)
                && (col == 6 || Some(col) == last)
                && (row == 6 || col == 6);
            if !on_finder {
                draw_square(code, row, col, 2, |ring| ring != 1);
            }
        }
    }
    // Reserved now, drawn once the mask is chosen.
    draw_format(code, 0);
    draw_version(code);
}

/// The square of modules up to `reach` from `row`, `col` (those inside the code), each dark
/// when `dark` says so of its ring, its distance from the centre.
fn draw_square(
    code: &mut QrCode,
    row: usize,
    col: usize,
    reach: usize,
    dark: impl Fn(usize) -> bool,
) {
    for r in row.saturating_sub(reach)..=(row + reach).min(code.size - 1) {
        for c in col.saturating_sub(reach)..=(col + reach).min(code.size - 1) {
            let ring = r.abs_diff(row).max(c.abs_diff(col));
            code.set(r, c, dark(ring));
        }
    }
}

/// The 15 format bits for level M and `mask`: BCH(15,5), then XORed with 101010000010010.
pub(super) fn format_bits(mask: u8) -> u16 {
    // Level M is 00.
    let data = u16::from(mask);
    let mut rem = data;
    for _ in 0..10 {
        rem = (rem << 1) ^ ((rem >> 9) * 0x537);
    }
    ((data << 10) | rem) ^ 0x5412
}

/// Draws the format information twice: around the top-left finder, and split between the
/// other two, with the dark module beside the bottom-left one.
fn draw_format(code: &mut QrCode, mask: u8) {
    let bits = format_bits(mask);
    let bit = |i: usize| (bits >> i) & 1 == 1;
    let size = code.size;
    for i in 0..=5 {
        code.set(i, 8, bit(i));
    }
    code.set(7, 8, bit(6));
    code.set(8, 8, bit(7));
    code.set(8, 7, bit(8));
    for i in 9..15 {
        code.set(8, 14 - i, bit(i));
    }
    for i in 0..8 {
        code.set(8, size - 1 - i, bit(i));
    }
    for i in 8..15 {
        code.set(size - 15 + i, 8, bit(i));
    }
    code.set(size - 8, 8, true);
}

/// The 18 version bits, BCH(18,6), for versions 7 and up.
pub(super) fn version_bits(version: usize) -> u32 {
    let data = version as u32;
    let mut rem = data;
    for _ in 0..12 {
        rem = (rem << 1) ^ ((rem >> 11) * 0x1F25);
    }
    (data << 12) | rem
}

/// The version information, in two 6 × 3 blocks beside the top-right and bottom-left finders.
fn draw_version(code: &mut QrCode) {
    if code.version < 7 {
        return;
    }
    let bits = version_bits(code.version);
    let size = code.size;
    for i in 0..18 {
        let dark = (bits >> i) & 1 == 1;
        let (a, b) = (size - 11 + i % 3, i / 3);
        code.set(a, b, dark);
        code.set(b, a, dark);
    }
}

/// Places the codewords' bits up and down two-module columns from the bottom right, skipping
/// the vertical timing pattern and every function module.
fn place_data(code: &mut QrCode, codewords: &[u8]) {
    let size = code.size;
    let total = codewords.len() * 8;
    let mut i = 0;
    let mut right = size - 1;
    loop {
        if right == 6 {
            right = 5;
        }
        let upward = ((right + 1) & 2) == 0;
        for step in 0..size {
            let row = if upward { size - 1 - step } else { step };
            for col in [right, right - 1] {
                if code.function[row * size + col] {
                    continue;
                }
                if i < total {
                    code.modules[row * size + col] = (codewords[i / 8] >> (7 - i % 8)) & 1 == 1;
                    i += 1;
                }
            }
        }
        if right < 2 {
            break;
        }
        right -= 2;
    }
}

/// Whether `mask` flips the module at `row`, `col`.
fn masked(mask: u8, row: usize, col: usize) -> bool {
    match mask {
        0 => (row + col).is_multiple_of(2),
        1 => row.is_multiple_of(2),
        2 => col.is_multiple_of(3),
        3 => (row + col).is_multiple_of(3),
        4 => (row / 2 + col / 3).is_multiple_of(2),
        5 => (row * col) % 2 + (row * col) % 3 == 0,
        6 => ((row * col) % 2 + (row * col) % 3).is_multiple_of(2),
        _ => ((row + col) % 2 + (row * col) % 3).is_multiple_of(2),
    }
}

/// Flips the data modules `mask` picks.
fn apply_mask(code: &mut QrCode, mask: u8) {
    for row in 0..code.size {
        for col in 0..code.size {
            let at = row * code.size + col;
            if !code.function[at] && masked(mask, row, col) {
                code.modules[at] = !code.modules[at];
            }
        }
    }
}

mod penalty;
use penalty::penalty;

#[cfg(test)]
mod tests;
