//! Image sizes read from encoded PNG and JPEG headers, so a capture's real size is
//! checked rather than assumed. The harness includes this file too.

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
}
