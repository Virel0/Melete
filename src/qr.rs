//! QR codes drawn as SVG on the server, for the page to show: a tunnel invite's WireGuard
//! config, which a phone's WireGuard app scans. Drawn here, so nothing is sent anywhere to make
//! one, and the page needs no library of its own for it.

use qrcode::render::svg;
use qrcode::{EcLevel, QrCode};

/// `text` as a QR code in SVG: one unit of its viewBox per module, with the quiet zone around
/// it, black on white as scanners expect whatever the page's colours, and no XML prolog, so it
/// goes into a page as it is. None when `text` is too long for one.
pub fn svg(text: &str) -> Option<String> {
    let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::M).ok()?;
    let drawn = code
        .render::<svg::Color<'_>>()
        .module_dimensions(1, 1)
        .quiet_zone(true)
        .dark_color(svg::Color("#000000"))
        .light_color(svg::Color("#ffffff"))
        .build();
    let svg = drawn.find("<svg").map(|at| drawn[at..].to_owned())?;
    Some(svg)
}

#[cfg(test)]
pub mod testing {
    /// What a scanner reads from `svg` as [`super::svg`] draws it: the dark modules are
    /// rectangles `M{left} {top}h{width}v{height}H{left}V{top}` in one path.
    pub fn read(svg: &str) -> String {
        let size = |name: &str| -> usize {
            let at = svg.find(&format!(" {name}=\"")).unwrap() + name.len() + 3;
            svg[at..].split('"').next().unwrap().parse().unwrap()
        };
        let (width, height) = (size("width"), size("height"));
        let mut dark = vec![false; width * height];
        let path = svg
            .split(" d=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        for rect in path.split('M').filter(|r| !r.is_empty()) {
            let numbers: Vec<usize> = rect
                .split(|c: char| c.is_ascii_alphabetic() || c == ' ')
                .filter(|n| !n.is_empty())
                .map(|n| n.parse().unwrap())
                .collect();
            let [left, top, w, h, ..] = numbers[..] else {
                panic!("not a rectangle: {rect}");
            };
            for y in top..top + h {
                for x in left..left + w {
                    dark[y * width + x] = true;
                }
            }
        }
        // Each module made four pixels a side, as a camera would see it.
        let scale = 4;
        let mut image =
            rqrr::PreparedImage::prepare_from_greyscale(width * scale, height * scale, |x, y| {
                if dark[(y / scale) * width + x / scale] {
                    0
                } else {
                    255
                }
            });
        let grids = image.detect_grids();
        assert_eq!(grids.len(), 1, "one code");
        grids[0].decode().unwrap().1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wireguard_config_reads_back_from_its_svg() {
        let config = "[Interface]\nPrivateKey = aGVsbG8gdGhpcyBpcyBub3QgYSByZWFsIGtleSBhdCBhbGw=\nAddress = 10.73.0.2/32\n\n[Peer]\nPublicKey = c29tZSBwdWJsaWMga2V5IGZvciB0aGUgdGVzdHMgb25seSE=\nAllowedIPs = 10.73.0.1/32\nEndpoint = melete.example:51820\nPersistentKeepalive = 25\n";
        let drawn = svg(config).unwrap();
        assert!(
            drawn.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""),
            "{drawn}"
        );
        assert!(drawn.contains(" viewBox=\"0 0 ") && drawn.ends_with("</svg>"));
        assert!(!drawn.contains("<?xml"));
        assert_eq!(testing::read(&drawn), config);
    }

    #[test]
    fn too_much_for_one_code_is_none() {
        assert_eq!(svg(&"x".repeat(5000)), None);
    }
}
