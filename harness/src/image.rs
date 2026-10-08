//! The PPM pixels C3 inspects. Image sizes come from the server's `image_header`.

pub(crate) use crate::image_header::{jpeg_size, png_size};

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
    let marked = pixels(image).map(|&pixel| !is_colour(pixel, background));
    bounds(image.width, marked)
}

/// How many pixels are the background colour.
pub(crate) fn count(image: &Rgb<'_>, background: [u8; 3]) -> usize {
    pixels(image)
        .filter(|&&pixel| is_colour(pixel, background))
        .count()
}

fn is_colour(pixel: [u8; 3], colour: [u8; 3]) -> bool {
    pixel
        .iter()
        .zip(colour)
        .all(|(&value, expected)| value.abs_diff(expected) <= BACKGROUND_TOLERANCE)
}

/// Pixels that differ between two captures of the same size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Difference {
    pub(crate) pixels: usize,
    /// The smallest rectangle holding them.
    pub(crate) area: Option<Rect>,
}

/// `None` if the images have different sizes.
pub(crate) fn difference(before: &Rgb<'_>, after: &Rgb<'_>) -> Option<Difference> {
    if (before.width, before.height) != (after.width, after.height) {
        return None;
    }
    let changed = || pixels(before).zip(pixels(after)).map(|(a, b)| a != b);
    Some(Difference {
        pixels: changed().filter(|&changed| changed).count(),
        area: bounds(before.width, changed()),
    })
}

fn pixels<'a>(image: &Rgb<'a>) -> impl Iterator<Item = &'a [u8; 3]> {
    image.pixels.as_chunks::<3>().0.iter()
}

/// The smallest rectangle holding every marked pixel, in row-major order.
fn bounds(width: usize, marked: impl Iterator<Item = bool>) -> Option<Rect> {
    let mut found: Option<(usize, usize, usize, usize)> = None;
    for (index, _) in marked.enumerate().filter(|&(_, marked)| marked) {
        let (x, y) = (index % width, index / width);
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
    fn difference_counts_changed_pixels_and_bounds_them() {
        let before = image(6, 5, &[]);
        let after = image(6, 5, &[(1, 1), (4, 3)]);
        let (before, after) = (ppm(&before).unwrap(), ppm(&after).unwrap());
        assert_eq!(
            difference(&before, &after),
            Some(Difference {
                pixels: 2,
                area: Some(Rect {
                    x: 1,
                    y: 1,
                    width: 4,
                    height: 3,
                }),
            })
        );
        assert_eq!(
            difference(&before, &before),
            Some(Difference {
                pixels: 0,
                area: None,
            })
        );
        let other = image(5, 6, &[]);
        assert_eq!(difference(&before, &ppm(&other).unwrap()), None);
    }

    #[test]
    fn counts_background_pixels() {
        let bytes = image(6, 5, &[(1, 1), (4, 3)]);
        assert_eq!(count(&ppm(&bytes).unwrap(), [0xFF, 0x00, 0xFF]), 28);
    }

    #[test]
    fn rejects_truncated_and_other_ppms() {
        let bytes = image(4, 4, &[]);
        assert!(ppm(bytes.get(..bytes.len() - 1).unwrap()).is_none());
        assert!(ppm(b"P3\n1 1\n255\n\0\0\0").is_none());
        assert!(ppm(b"P6\n1 1\n65535\n\0\0\0\0\0\0").is_none());
    }
}
