//! Binary PPM (P6) images, as grim writes them with `-t ppm`: a text header, then the
//! pixels as raw RGB, three bytes each (pure). The settle wait compares two captures pixel
//! by pixel with them, without decoding anything.

use crate::coords::{ImagePx, ImageRect};

/// A P6 image: its size, and its pixels, row by row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Pixmap<'a> {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pixels: &'a [u8],
}

/// Bytes compared at once before looking at single pixels: 16 pixels.
const BLOCK: usize = 48;

/// Reads `bytes` as a P6 image: `P6`, the width, the height and a maximum value of 255,
/// separated by whitespace and `#` comments, one whitespace byte, then exactly three bytes
/// for each pixel. `None` for anything else, an empty image included.
pub(crate) fn parse(bytes: &[u8]) -> Option<Pixmap<'_>> {
    let rest = bytes.strip_prefix(b"P6")?;
    let (width, rest) = number(rest)?;
    let (height, rest) = number(rest)?;
    let (max, rest) = number(rest)?;
    let (&separator, pixels) = rest.split_first()?;
    let length = (u64::from(width) * u64::from(height)).checked_mul(3)?;
    let fits = usize::try_from(length).is_ok_and(|length| length == pixels.len());
    (max == 255 && separator.is_ascii_whitespace() && fits && length > 0).then_some(Pixmap {
        width,
        height,
        pixels,
    })
}

/// The width and height of a P6 image, as `image_header` reads other formats'.
pub(crate) fn size(bytes: &[u8]) -> Option<(u32, u32)> {
    parse(bytes).map(|image| (image.width, image.height))
}

/// After whitespace and comments, a decimal number, and what follows it.
fn number(mut rest: &[u8]) -> Option<(u32, &[u8])> {
    loop {
        let (&first, after) = rest.split_first()?;
        if first.is_ascii_whitespace() {
            rest = after;
        } else if first == b'#' {
            let end = after.iter().position(|&byte| byte == b'\n')?;
            rest = after.get(end..)?;
        } else {
            break;
        }
    }
    let digits = rest.iter().take_while(|byte| byte.is_ascii_digit()).count();
    let (number, after) = rest.split_at_checked(digits)?;
    let number = std::str::from_utf8(number).ok()?.parse().ok()?;
    Some((number, after))
}

/// How many pixels differ between `a` and `b`, or `None` when their sizes do. Equal blocks
/// of 16 pixels are skipped with one comparison, so a mostly still screen costs about as
/// much as comparing the bytes.
pub(crate) fn changed(a: &Pixmap<'_>, b: &Pixmap<'_>) -> Option<u64> {
    if (a.width, a.height) != (b.width, b.height) {
        return None;
    }
    u64::try_from(differing(a.pixels, b.pixels)).ok()
}

/// How many pixels outside `mask` differ between `a` and `b`, and how many were compared,
/// or `None` when their sizes differ. A mask that leaves no pixel to compare, or covers
/// none, is ignored: every pixel is compared.
pub(crate) fn changed_outside(
    a: &Pixmap<'_>,
    b: &Pixmap<'_>,
    mask: Option<ImageRect>,
) -> Option<(u64, u64)> {
    let all = pixels(a);
    let Some((mask, left)) = mask.and_then(|mask| kept(a, mask)) else {
        return Some((changed(a, b)?, all));
    };
    if (a.width, a.height) != (b.width, b.height) {
        return None;
    }
    let row = usize::try_from(a.width).ok()?.checked_mul(3)?;
    let (start, end) = (to_bytes(mask.left)?, to_bytes(mask.right)?);
    let (top, bottom) = (
        usize::try_from(mask.top).ok()?,
        usize::try_from(mask.bottom).ok()?,
    );
    let rows = a.pixels.chunks(row).zip(b.pixels.chunks(row)).enumerate();
    let count = rows.map(|(y, (p, q))| {
        if (top..bottom).contains(&y) {
            let before = differing(p.get(..start)?, q.get(..start)?);
            Some(before + differing(p.get(end..)?, q.get(end..)?))
        } else {
            Some(differing(p, q))
        }
    });
    Some((u64::try_from(count.sum::<Option<usize>>()?).ok()?, left))
}

/// The columns `left..right` of rows `top..bottom` a mask covers.
struct Span {
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
}

/// The part of `image` that `mask` covers, and how many pixels it leaves, unless that is
/// none or all of them.
fn kept(image: &Pixmap<'_>, mask: ImageRect) -> Option<(Span, u64)> {
    let ImagePx { x, y } = mask.origin;
    let span = Span {
        left: x.min(image.width),
        top: y.min(image.height),
        right: x.saturating_add(mask.width).min(image.width),
        bottom: y.saturating_add(mask.height).min(image.height),
    };
    let width = span.right - span.left;
    let height = span.bottom - span.top;
    let covered = u64::from(width) * u64::from(height);
    let left = pixels(image) - covered;
    (covered > 0 && left > 0).then_some((span, left))
}

fn to_bytes(column: u32) -> Option<usize> {
    usize::try_from(column).ok()?.checked_mul(3)
}

/// How many three-byte pixels differ between `a` and `b`, of the same length.
fn differing(a: &[u8], b: &[u8]) -> usize {
    let blocks = a.chunks(BLOCK).zip(b.chunks(BLOCK));
    blocks
        .filter(|(x, y)| x != y)
        .map(|(x, y)| {
            let pixels = x.as_chunks::<3>().0.iter().zip(y.as_chunks::<3>().0);
            pixels.filter(|(p, q)| p != q).count()
        })
        .sum()
}

/// The pixels of an image as a count, for fractions of it.
pub(crate) fn pixels(image: &Pixmap<'_>) -> u64 {
    u64::from(image.width) * u64::from(image.height)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    fn image(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
        [format!("P6\n{width} {height}\n255\n").as_bytes(), pixels].concat()
    }

    #[test]
    fn reads_grims_header_comments_and_nothing_malformed() {
        let pixels = [1, 2, 3, 4, 5, 6];
        let grim = image(2, 1, &pixels);
        assert_eq!(size(&grim), Some((2, 1)));
        let commented = [b"P6 # grim\n2\t1 # size\n255 ".as_slice(), &pixels].concat();
        assert_eq!(parse(&commented), parse(&grim));
        for bad in [
            image(2, 1, &pixels[..5]),
            image(0, 0, &[]),
            [b"P6\n2 1\n65535\n".as_slice(), &pixels].concat(),
            [b"P3\n2 1\n255\n".as_slice(), &pixels].concat(),
            b"P6\n2 1\n255".to_vec(),
            b"P6\n99999999999 1\n255\n".to_vec(),
            b"P6\n4294967295 4294967295\n255\n".to_vec(),
            b"\xff\xd8\xff".to_vec(),
        ] {
            assert_eq!(parse(&bad), None, "{bad:?}");
        }
    }

    #[test]
    fn counts_the_pixels_that_differ_in_any_channel() {
        let before: Vec<u8> = (0..60u8).collect();
        let mut after = before.clone();
        // Pixels 0, 15 and 19: in the first block, at its end, and in the second.
        for byte in [2, 45, 58] {
            after[byte] ^= 1;
        }
        let (before, after) = (image(20, 1, &before), image(20, 1, &after));
        let (a, b) = (parse(&before).unwrap(), parse(&after).unwrap());
        assert_eq!(changed(&a, &b), Some(3));
        assert_eq!(changed(&a, &a), Some(0));
        let other = image(10, 2, &(0..60u8).collect::<Vec<_>>());
        assert_eq!(changed(&a, &parse(&other).unwrap()), None);
        assert_eq!(pixels(&a), 20);
    }

    /// What reading and comparing two captures costs, for the size the settle wait samples
    /// at (1280x720) and a native 2560x1440 output: a still screen, a 100x100 clock and
    /// every pixel changed. Printed, and checked only with `NCU_PERF=1` in a release build.
    #[test]
    fn comparing_two_captures_is_cheap() {
        for (width, height) in [(1280, 720), (2560, 1440)] {
            let still = image(width, height, &vec![7; (width * height * 3) as usize]);
            let header = still.len() - (width * height * 3) as usize;
            let mut clock = still.clone();
            for row in 0..100 {
                let start = header + ((row * width + width / 2) * 3) as usize;
                clock[start..start + 300].fill(9);
            }
            let mut all = still.clone();
            all[header..].fill(8);
            for (name, other) in [("still", &still), ("clock", &clock), ("all", &all)] {
                time_comparison(&format!("{width}x{height} {name}"), &still, other);
            }
        }
    }

    #[expect(clippy::print_stderr, reason = "the benchmark prints what it measured")]
    fn time_comparison(name: &str, a: &[u8], b: &[u8]) {
        let started = Instant::now();
        let count = changed(&parse(a).unwrap(), &parse(b).unwrap()).unwrap();
        let took = started.elapsed();
        eprintln!("{name}: {count} pixels changed in {took:?}");
        if std::env::var_os("NCU_PERF").is_some_and(|perf| perf == "1") {
            assert!(took < Duration::from_millis(20), "{name}: {took:?}");
        }
    }
}
