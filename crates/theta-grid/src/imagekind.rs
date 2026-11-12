//! Recognising an image in a binary cell from its magic bytes. Pure.
//!
//! The viewer only needs to know *whether* the bytes are a picture GPUI can decode and how large it says
//! it is (to label it and to refuse absurd sizes before decoding). Reading the dimensions out of the
//! header by hand costs a few lines per format and spares the grid crate an image-decoding dependency;
//! the actual decoding is GPUI's, at paint time.

/// The formats we sniff. GPUI decodes all four.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    Png,
    Jpeg,
    Gif,
    Webp,
}

impl ImageKind {
    pub fn label(self) -> &'static str {
        match self {
            ImageKind::Png => "PNG",
            ImageKind::Jpeg => "JPEG",
            ImageKind::Gif => "GIF",
            ImageKind::Webp => "WebP",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageInfo {
    pub kind: ImageKind,
    /// `None` when the header is cut short or uses a layout we do not read; the image may still decode.
    pub size: Option<(u32, u32)>,
}

impl ImageInfo {
    /// `PNG 640×480`, or just `PNG`.
    pub fn label(&self) -> String {
        match self.size {
            Some((w, h)) => format!("{} {w}×{h}", self.kind.label()),
            None => self.kind.label().to_string(),
        }
    }
}

/// `Some` when `bytes` start like a PNG, JPEG, GIF or WebP file. Only the signature decides the
/// kind; a damaged body is GPUI's problem to report when it decodes.
pub fn sniff_image(bytes: &[u8]) -> Option<ImageInfo> {
    let (kind, size) = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        (ImageKind::Png, png_size(bytes))
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        (ImageKind::Jpeg, jpeg_size(bytes))
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        (ImageKind::Gif, gif_size(bytes))
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        (ImageKind::Webp, webp_size(bytes))
    } else {
        return None;
    };
    Some(ImageInfo { kind, size: size.filter(|&(w, h)| w > 0 && h > 0) })
}

fn be16(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from(u16::from_be_bytes([*b.get(at)?, *b.get(at + 1)?])))
}

fn le16(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from(u16::from_le_bytes([*b.get(at)?, *b.get(at + 1)?])))
}

fn le24(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from(*b.get(at)?) | u32::from(*b.get(at + 1)?) << 8 | u32::from(*b.get(at + 2)?) << 16)
}

/// The IHDR chunk is always first: 4 bytes length, `IHDR`, then width and height as big-endian u32.
fn png_size(b: &[u8]) -> Option<(u32, u32)> {
    if b.get(12..16)? != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(b.get(16..20)?.try_into().ok()?);
    let h = u32::from_be_bytes(b.get(20..24)?.try_into().ok()?);
    Some((w, h))
}

fn gif_size(b: &[u8]) -> Option<(u32, u32)> {
    Some((le16(b, 6)?, le16(b, 8)?))
}

/// Walk the marker segments up to the first start-of-frame marker (SOF0..SOF15 except DHT, JPG and
/// DAC), which carries the dimensions. Stops at start-of-scan: no frame header after it.
fn jpeg_size(b: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2;
    loop {
        if *b.get(i)? != 0xFF {
            return None;
        }
        // Any number of 0xFF fill bytes may precede a marker.
        while *b.get(i + 1)? == 0xFF {
            i += 1;
        }
        let marker = *b.get(i + 1)?;
        match marker {
            0xC0..=0xCF if !matches!(marker, 0xC4 | 0xC8 | 0xCC) => {
                return Some((be16(b, i + 7)?, be16(b, i + 5)?));
            }
            0xDA | 0xD9 => return None,
            // Markers without a length field.
            0x01 | 0xD0..=0xD8 => i += 2,
            _ => i += 2 + be16(b, i + 2)? as usize,
        }
    }
}

/// Three layouts: lossy (`VP8 `), lossless (`VP8L`) and extended (`VP8X`, canvas size).
fn webp_size(b: &[u8]) -> Option<(u32, u32)> {
    match b.get(12..16)? {
        b"VP8 " => {
            if b.get(23..26)? != [0x9d, 0x01, 0x2a] {
                return None;
            }
            Some((le16(b, 26)? & 0x3fff, le16(b, 28)? & 0x3fff))
        }
        b"VP8L" => {
            if *b.get(20)? != 0x2f {
                return None;
            }
            let bits = u32::from_le_bytes(b.get(21..25)?.try_into().ok()?);
            Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
        }
        b"VP8X" => Some((le24(b, 24)? + 1, le24(b, 27)? + 1)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        v.extend(w.to_be_bytes());
        v.extend(h.to_be_bytes());
        v
    }

    #[test]
    fn png_dimensions() {
        let i = sniff_image(&png(640, 480)).unwrap();
        assert_eq!(i.kind, ImageKind::Png);
        assert_eq!(i.size, Some((640, 480)));
        assert_eq!(i.label(), "PNG 640×480");
    }

    #[test]
    fn gif_dimensions() {
        let mut v = b"GIF89a".to_vec();
        v.extend([0x10, 0x01, 0x20, 0x00]);
        assert_eq!(sniff_image(&v).unwrap().size, Some((272, 32)));
    }

    #[test]
    fn jpeg_skips_to_the_frame_header() {
        // SOI, APP0 (length 16), a fill byte, SOF0 with 300 x 200.
        let mut v = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        v.extend([0u8; 14]);
        v.extend([0xFF, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0xC8, 0x01, 0x2C]);
        let i = sniff_image(&v).unwrap();
        assert_eq!(i.kind, ImageKind::Jpeg);
        assert_eq!(i.size, Some((300, 200)));
    }

    #[test]
    fn jpeg_without_frame_header_has_no_size() {
        let i = sniff_image(&[0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x02]).unwrap();
        assert_eq!((i.kind, i.size), (ImageKind::Jpeg, None));
    }

    #[test]
    fn webp_three_layouts() {
        let mut lossy = b"RIFF\0\0\0\0WEBPVP8 \0\0\0\0\0\0\0".to_vec();
        lossy.extend([0x9d, 0x01, 0x2a, 0x40, 0x01, 0xF0, 0x00]);
        assert_eq!(sniff_image(&lossy).unwrap().size, Some((320, 240)));

        // 1x1 lossless: width-1 = 0 and height-1 = 0 in 14 bits each.
        let mut lossless = b"RIFF\0\0\0\0WEBPVP8L\0\0\0\0".to_vec();
        lossless.extend([0x2f, 0, 0, 0, 0]);
        assert_eq!(sniff_image(&lossless).unwrap().size, Some((1, 1)));

        let mut ext = b"RIFF\0\0\0\0WEBPVP8X\0\0\0\0\0\0\0\0".to_vec();
        ext.extend([0x7f, 0x01, 0x00, 0x63, 0x00, 0x00]);
        assert_eq!(sniff_image(&ext).unwrap().size, Some((384, 100)));
    }

    #[test]
    fn other_bytes_are_not_images() {
        assert!(sniff_image(b"").is_none());
        assert!(sniff_image(b"PK\x03\x04 zip").is_none());
        assert!(sniff_image(b"\x89PNG").is_none());
        assert!(sniff_image(b"RIFF\0\0\0\0WAVEfmt ").is_none());
    }

    #[test]
    fn truncated_header_still_names_the_kind() {
        let i = sniff_image(b"\x89PNG\r\n\x1a\n\0\0").unwrap();
        assert_eq!((i.kind, i.size), (ImageKind::Png, None));
    }

    #[test]
    fn zero_dimensions_are_discarded() {
        assert_eq!(sniff_image(&png(0, 10)).unwrap().size, None);
    }
}
