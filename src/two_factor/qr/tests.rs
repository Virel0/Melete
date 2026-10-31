use super::*;

/// A setup link as Melete makes it, with a well-known example secret.
const ALICE: &str = "otpauth://totp/Melete:alice?secret=JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP&issuer=Melete&algorithm=SHA1&digits=6&period=30";

fn long_text() -> String {
    "The quick brown fox jumps over the lazy dog. ".repeat(5)[..200].to_owned()
}

/// The code's modules as rows of 0 and 1.
fn rows(code: &QrCode) -> Vec<String> {
    (0..code.size)
        .map(|r| {
            (0..code.size)
                .map(|c| if code.dark(r, c) { '1' } else { '0' })
                .collect()
        })
        .collect()
}

#[test]
fn correction_matches_the_standards_worked_example() {
    // ISO/IEC 18004's worked example: "01234567" as version 1-M, its 16 data codewords and
    // the 10 correction codewords the standard gives for them.
    let data = [
        0x10, 0x20, 0x0C, 0x56, 0x61, 0x80, 0xEC, 0x11, 0xEC, 0x11, 0xEC, 0x11, 0xEC, 0x11, 0xEC,
        0x11,
    ];
    assert_eq!(
        correction(&data, 10),
        [0xA5, 0x24, 0xD4, 0xC1, 0xED, 0x36, 0xC7, 0x87, 0x2C, 0x55]
    );
}

#[test]
fn format_and_version_bits_match_the_standards_tables() {
    let format = [
        0b101010000010010,
        0b101000100100101,
        0b101111001111100,
        0b101101101001011,
        0b100010111111001,
        0b100000011001110,
        0b100111110010111,
        0b100101010100000,
    ];
    for (mask, bits) in format.into_iter().enumerate() {
        assert_eq!(format_bits(mask as u8), bits, "level M, mask {mask}");
    }
    let version = [
        (7, 0b000111110010010100),
        (8, 0b001000010110111100),
        (9, 0b001001101010011001),
        (10, 0b001010010011010011),
    ];
    for (v, bits) in version {
        assert_eq!(version_bits(v), bits, "version {v}");
    }
}

#[test]
fn codes_match_a_reference_encoder_module_for_module() {
    // Made with libqrencode 4.1.1: `qrencode -8 -l M -t ASCII -m 0`, dark as 1. Encoders read
    // the penalty rules a little differently, so each picks its own mask; with the mask it
    // picked (from the format bits), every module is the same.
    let cases = [
        (
            "Melete".to_owned(),
            1,
            7,
            include_str!("../../../tests/fixtures/qr/melete.txt"),
        ),
        (
            ALICE.to_owned(),
            7,
            4,
            include_str!("../../../tests/fixtures/qr/otpauth-alice.txt"),
        ),
        (
            long_text(),
            10,
            1,
            include_str!("../../../tests/fixtures/qr/long-200.txt"),
        ),
    ];
    for (text, version, mask, expected) in cases {
        let code = with_mask(unmasked(text.as_bytes()).expect("fits"), mask);
        assert_eq!(code.version, version, "{text}");
        assert_eq!(rows(&code), expected.lines().collect::<Vec<_>>(), "{text}");
        assert_eq!(encode(text.as_bytes()).map(|c| c.version), Some(version));
    }
}

#[test]
fn the_mask_with_the_lowest_penalty_is_chosen() {
    for text in ["Melete".to_owned(), ALICE.to_owned(), long_text()] {
        let code = encode(text.as_bytes()).expect("fits");
        let scores: Vec<u32> = (0..8)
            .map(|m| penalty(&with_mask(unmasked(text.as_bytes()).expect("fits"), m)))
            .collect();
        let lowest = *scores.iter().min().expect("eight");
        assert_eq!(scores[usize::from(code.mask)], lowest, "{text}: {scores:?}");
        assert_eq!(
            scores.iter().position(|&s| s == lowest),
            Some(usize::from(code.mask)),
            "the first of equals"
        );
    }
}

/// A code of `rows`, every module counted as data.
fn grid(rows: &[&str]) -> QrCode {
    let size = rows.len();
    QrCode {
        version: 1,
        size,
        mask: 0,
        modules: rows
            .iter()
            .flat_map(|r| r.bytes().map(|b| b == b'1'))
            .collect(),
        function: vec![false; size * size],
    }
}

#[test]
fn penalty_rules_count_as_the_standard_says() {
    let line = |s: &str| penalty::line_penalty(s.len(), |j| s.as_bytes()[j] == b'1');
    // Five of one colour in a row: 3, and 1 for each more.
    assert_eq!(line("11111"), 3);
    assert_eq!(line("1111111"), 5);
    assert_eq!(line("0101010"), 0);
    // Dark, light, three dark, light, dark, with four light before or after it: 40.
    assert_eq!(line("11000010111010011"), 40);
    assert_eq!(line("11001011101000011"), 40);
    // Not when a dark run is longer than the pattern's.
    assert_eq!(line("11000010111011011"), 0);
    // The margin around the code counts as light.
    assert_eq!(line("1011101011"), 40);
    // Each 2 × 2 block of one colour: 3; all dark is 50% away from half: 9 × 10.
    assert_eq!(penalty(&grid(&["11", "11"])), 3 + 90);
    // Half dark, no blocks, no runs: nothing.
    assert_eq!(penalty(&grid(&["1010", "0101", "1010", "0101"])), 0);
}

#[test]
fn the_smallest_version_is_used_and_version_10_is_the_limit() {
    assert_eq!(encode(&[b'a'; 14]).map(|c| c.version), Some(1));
    assert_eq!(encode(&[b'a'; 15]).map(|c| c.version), Some(2));
    assert_eq!(encode(&[b'a'; 213]).map(|c| c.version), Some(10));
    assert_eq!(encode(&[b'a'; 214]), None);
    // The longest setup link: a 64-character username.
    let longest = ALICE.replace("alice", &"a".repeat(64));
    assert!(encode(longest.as_bytes()).is_some());
}

/// The modules an SVG from `QrCode::svg` draws, read back from its path.
fn modules_drawn(svg: &str, size: usize) -> Vec<String> {
    let d = svg
        .split(" d=\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .expect("a path");
    let mut grid = vec![vec!['0'; size]; size];
    for shape in d.split('M').filter(|s| !s.is_empty()) {
        let (at, rest) = shape.split_once('h').expect("M x,y h");
        let (x, y) = at.split_once(',').expect("x,y");
        let width: usize = rest
            .split('v')
            .next()
            .expect("width")
            .parse()
            .expect("a number");
        let (x, y): (usize, usize) = (x.parse().expect("x"), y.parse().expect("y"));
        assert_eq!(rest, format!("{width}v1h-{width}z"), "one row of modules");
        for col in x..x + width {
            grid[y - 4][col - 4] = '1';
        }
    }
    grid.into_iter().map(|r| r.into_iter().collect()).collect()
}

#[test]
fn the_svg_draws_exactly_the_codes_modules_with_a_margin() {
    let code = encode(ALICE.as_bytes()).expect("fits");
    let svg = code.svg();
    let side = code.size + 8;
    assert!(svg.starts_with(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {side} {side}\""
    )));
    assert!(svg.contains(&format!(
        "<rect width=\"{side}\" height=\"{side}\" fill=\"#fff\"/>"
    )));
    assert_eq!(modules_drawn(&svg, code.size), rows(&code));
}

/// What zbarimg reads from `svg` drawn as a black-and-white image, 8 pixels a module.
fn scanned(svg: &str, size: usize) -> String {
    let grid = modules_drawn(svg, size);
    let scale = 8;
    let side = (size + 8) * scale;
    let mut image = format!("P1\n{side} {side}\n");
    for y in 0..side {
        for x in 0..side {
            let (row, col) = ((y / scale).wrapping_sub(4), (x / scale).wrapping_sub(4));
            let dark = grid.get(row).and_then(|r| r.as_bytes().get(col)) == Some(&b'1');
            image.push(if dark { '1' } else { '0' });
        }
        image.push('\n');
    }
    let dir = tempfile::tempdir().expect("a scratch folder");
    let path = dir.path().join("code.pbm");
    std::fs::write(&path, image).expect("written");
    let out = std::process::Command::new("zbarimg")
        .args(["--quiet", "--raw"])
        .arg(&path)
        .output()
        .expect("zbarimg runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim_end().to_owned()
}

#[test]
fn a_scanner_reads_the_svg_back_as_the_link() {
    // zbarimg is a decoder some machines have; nothing depends on it, so without it this
    // test says so and checks nothing more than the tests above.
    let Ok(found) = std::process::Command::new("zbarimg")
        .arg("--version")
        .output()
    else {
        eprintln!("zbarimg isn't installed: the scan-back check didn't run");
        return;
    };
    assert!(found.status.success());
    let code = encode(ALICE.as_bytes()).expect("fits");
    assert_eq!(scanned(&code.svg(), code.size), ALICE);
    // Every version, filled to the last byte it holds.
    for version in 1..=MAX_VERSION {
        let text: String = (0..capacity(version))
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect();
        let code = encode(text.as_bytes()).expect("fits");
        assert_eq!(code.version, version);
        assert_eq!(scanned(&code.svg(), code.size), text, "version {version}");
    }
}
