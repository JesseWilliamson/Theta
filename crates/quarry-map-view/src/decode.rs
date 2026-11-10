//! Tile image decoding into GPUI's `RenderImage`.

use gpui_kit::RenderImage;
use image::{Frame, ImageBuffer};
use std::sync::Arc;

/// Decode PNG or JPEG bytes (format sniffed from content) into a GPUI image.
///
/// GPUI's atlas wants BGRA, so red and blue are swapped here, on the worker thread,
/// instead of on the UI thread at upload time.
pub fn decode_tile(bytes: &[u8]) -> Result<Arc<RenderImage>, String> {
    let img = image::load_from_memory(bytes).map_err(|e| format!("decode: {e}"))?;
    let mut rgba = img.into_rgba8();
    for px in rgba.pixels_mut() {
        px.0.swap(0, 2);
    }
    let (w, h) = rgba.dimensions();
    let buf: ImageBuffer<image::Rgba<u8>, Vec<u8>> =
        ImageBuffer::from_raw(w, h, rgba.into_raw()).ok_or_else(|| "decode: bad buffer".to_string())?;
    Ok(Arc::new(RenderImage::new(vec![Frame::new(buf)])))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::DevicePixels;

    fn png(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba(rgba));
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn decodes_png_to_bgra() {
        let r = decode_tile(&png(4, 2, [10, 20, 30, 255])).unwrap();
        assert_eq!(r.size(0).width, DevicePixels(4));
        assert_eq!(r.size(0).height, DevicePixels(2));
        assert_eq!(&r.as_bytes(0).unwrap()[..4], &[30, 20, 10, 255]);
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(decode_tile(b"<html>429 Too Many Requests</html>").is_err());
        assert!(decode_tile(&[]).is_err());
    }
}
