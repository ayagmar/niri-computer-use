//! Image sizes read from encoded headers (plan §8), and the PPM pixels C3 inspects.

/// Width and height from a PNG's `IHDR` chunk, which must come first.
pub(crate) fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let rest = bytes.strip_prefix(b"\x89PNG\r\n\x1a\n")?;
    let [
        _,
        _,
        _,
        _,
        b'I',
        b'H',
        b'D',
        b'R',
        w0,
        w1,
        w2,
        w3,
        h0,
        h1,
        h2,
        h3,
        ..,
    ] = *rest
    else {
        return None;
    };
    Some((
        u32::from_be_bytes([w0, w1, w2, w3]),
        u32::from_be_bytes([h0, h1, h2, h3]),
    ))
}

/// Width and height from a JPEG's first start-of-frame segment.
pub(crate) fn jpeg_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut rest = bytes.strip_prefix(&[0xFF, 0xD8])?;
    loop {
        rest = rest.strip_prefix(&[0xFF])?;
        let start = rest.iter().position(|&byte| byte != 0xFF)?;
        let (&marker, after) = rest.get(start..)?.split_first()?;
        rest = after;
        if marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            continue;
        }
        let [l0, l1, ..] = *rest else {
            return None;
        };
        let length = usize::from(u16::from_be_bytes([l0, l1]));
        if is_start_of_frame(marker) {
            let [_, _, _, h0, h1, w0, w1, ..] = *rest.get(..length)? else {
                return None;
            };
            let size = |hi, lo| u32::from(u16::from_be_bytes([hi, lo]));
            return Some((size(w0, w1), size(h0, h1)));
        }
        if marker == 0xDA {
            return None;
        }
        rest = rest.get(length..)?;
    }
}

/// SOF0 to SOF15, except DHT (`C4`), JPG (`C8`) and DAC (`CC`).
fn is_start_of_frame(marker: u8) -> bool {
    (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC)
}

/// A binary PPM (`P6`, maximum value 255), as `grim -t ppm` writes it.
#[derive(Debug)]
pub(crate) struct Rgb<'a> {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pixels: &'a [u8],
}

pub(crate) fn ppm(bytes: &[u8]) -> Option<Rgb<'_>> {
    let mut rest = bytes.strip_prefix(b"P6")?;
    let mut fields = [0; 3];
    for field in &mut fields {
        let start = rest.iter().position(|byte| !byte.is_ascii_whitespace())?;
        let digits = rest.get(start..)?;
        let end = digits.iter().position(|byte| !byte.is_ascii_digit())?;
        *field = std::str::from_utf8(digits.get(..end)?).ok()?.parse().ok()?;
        rest = digits.get(end..)?;
    }
    let [width, height, 255] = fields else {
        return None;
    };
    // One whitespace byte separates the header from the pixels.
    let pixels = rest.get(1..)?;
    (pixels.len() == width * height * 3).then_some(Rgb {
        width,
        height,
        pixels,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rect {
    pub(crate) x: usize,
    pub(crate) y: usize,
    pub(crate) width: usize,
    pub(crate) height: usize,
}

/// How far a channel may be from the background colour and still count as background.
const BACKGROUND_TOLERANCE: u8 = 8;

/// The smallest rectangle holding every pixel that isn't the background colour.
pub(crate) fn bounding_box(image: &Rgb<'_>, background: [u8; 3]) -> Option<Rect> {
    let mut found: Option<(usize, usize, usize, usize)> = None;
    for (index, pixel) in image.pixels.as_chunks::<3>().0.iter().enumerate() {
        let is_background = pixel
            .iter()
            .zip(background)
            .all(|(&value, expected)| value.abs_diff(expected) <= BACKGROUND_TOLERANCE);
        if is_background {
            continue;
        }
        let (x, y) = (index % image.width, index / image.width);
        found = Some(found.map_or((x, y, x, y), |(left, top, right, bottom)| {
            (left.min(x), top.min(y), right.max(x), bottom.max(y))
        }));
    }
    found.map(|(left, top, right, bottom)| Rect {
        x: left,
        y: top,
        width: right - left + 1,
        height: bottom - top + 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_png_header() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend_from_slice(&960_u32.to_be_bytes());
        png.extend_from_slice(&720_u32.to_be_bytes());
        assert_eq!(png_size(&png), Some((960, 720)));
        assert_eq!(png_size(b"\x89PNG\r\n\x1a\n\0\0\0\x0dIDAT"), None);
    }

    #[test]
    fn reads_the_jpeg_frame_after_other_segments() {
        let jpeg = [
            0xFF, 0xD8, // SOI
            0xFF, 0xE0, 0x00, 0x04, 0xAA, 0xBB, // APP0, two bytes of payload
            0xFF, 0xFF, 0xDB, 0x00, 0x03, 0x00, // DQT after a fill byte
            0xFF, 0xC4, 0x00, 0x02, // DHT is not a frame header
            0xFF, 0xC0, 0x00, 0x08, 0x08, 0x01, 0x68, 0x01, 0xE0, 0x01, // SOF0 360x480
        ];
        assert_eq!(jpeg_size(&jpeg), Some((480, 360)));
        assert_eq!(jpeg_size(&[0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x02]), None);
        assert_eq!(jpeg_size(b"\x89PNG"), None);
    }

    fn image(width: usize, height: usize, marked: &[(usize, usize)]) -> Vec<u8> {
        let mut bytes = format!("P6\n{width} {height}\n255\n").into_bytes();
        for index in 0..width * height {
            let pixel = if marked.contains(&(index % width, index / width)) {
                [0x66, 0x66, 0x66]
            } else {
                [0xFC, 0x02, 0xFF]
            };
            bytes.extend_from_slice(&pixel);
        }
        bytes
    }

    #[test]
    fn bounding_box_covers_every_non_background_pixel() {
        let bytes = image(6, 5, &[(1, 1), (4, 3), (2, 2)]);
        let rgb = ppm(&bytes).unwrap();
        assert_eq!((rgb.width, rgb.height), (6, 5));
        assert_eq!(
            bounding_box(&rgb, [0xFF, 0x00, 0xFF]),
            Some(Rect {
                x: 1,
                y: 1,
                width: 4,
                height: 3,
            })
        );
        let empty = image(3, 3, &[]);
        assert_eq!(
            bounding_box(&ppm(&empty).unwrap(), [0xFF, 0x00, 0xFF]),
            None
        );
    }

    #[test]
    fn rejects_truncated_and_other_ppms() {
        let bytes = image(4, 4, &[]);
        assert!(ppm(bytes.get(..bytes.len() - 1).unwrap()).is_none());
        assert!(ppm(b"P3\n1 1\n255\n\0\0\0").is_none());
        assert!(ppm(b"P6\n1 1\n65535\n\0\0\0\0\0\0").is_none());
    }
}
