//! JPEGs decoded small, through the vendored libjpeg
//! (`native/jpeg_c_api.h`): libjpeg scales in the inverse DCT, so a
//! picture asked for small is never decoded at full size. Built with the
//! DNG SDK.

#[repr(C)]
struct DtJpegImage {
    _opaque: [u8; 0],
}

unsafe extern "C" {
    fn dt_jpeg_decode_scaled(data: *const u8, size: usize, min_side: u32) -> *mut DtJpegImage;
    fn dt_jpeg_image_rgb(image: *const DtJpegImage) -> *const u8;
    fn dt_jpeg_image_width(image: *const DtJpegImage) -> u32;
    fn dt_jpeg_image_height(image: *const DtJpegImage) -> u32;
    fn dt_jpeg_image_free(image: *mut DtJpegImage);
}

/// The JPEG in `data` as RGB8, `(pixels, width, height)`, at the smallest
/// n/8 of its stored size (n from 1 to 8) whose long side is at least
/// `min_side`: rows as stored, not yet turned upright. `None` for one
/// libjpeg can't read, or a CMYK one.
pub fn decode_scaled(data: &[u8], min_side: u32) -> Option<(Vec<u8>, u32, u32)> {
    // SAFETY: `data` is read for `data.len()` bytes during the call.
    let image = unsafe { dt_jpeg_decode_scaled(data.as_ptr(), data.len(), min_side) };
    if image.is_null() {
        return None;
    }
    // SAFETY: `image` is live until freed below, and its pixels are
    // width * height * 3 bytes.
    let decoded = unsafe {
        let (width, height) = (dt_jpeg_image_width(image), dt_jpeg_image_height(image));
        let len = width as usize * height as usize * 3;
        let rgb = std::slice::from_raw_parts(dt_jpeg_image_rgb(image), len).to_vec();
        dt_jpeg_image_free(image);
        (rgb, width, height)
    };
    Some(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_jpeg_decodes_at_the_smallest_eighth_that_fits() {
        let (w, h) = (1600u32, 1200u32);
        let image =
            image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x / 7) as u8, (y / 5) as u8, 90]));
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95)
            .encode_image(&image)
            .unwrap();
        // 1600 × 1/8 = 200 is under 256; 2/8 = 400 is the first that fits.
        let (rgb, sw, sh) = decode_scaled(&jpeg, 256).unwrap();
        assert_eq!((sw, sh), (400, 300));
        assert_eq!(rgb.len(), 400 * 300 * 3);
        // About the colors of its block of the original.
        let at = |x: u32, y: u32| &rgb[((y * sw + x) * 3) as usize..][..3];
        let [r, g, b] = [at(100, 100)[0], at(100, 100)[1], at(100, 100)[2]];
        assert!((i32::from(r) - 57).abs() <= 3 && (i32::from(g) - 80).abs() <= 3, "{r} {g}");
        assert!((i32::from(b) - 90).abs() <= 3);
        assert!(decode_scaled(&jpeg, 5000).is_some_and(|(_, w, _)| w == 1600), "whole at most");
        assert!(decode_scaled(b"not a jpeg", 10).is_none());
    }

    /// A JPEG libjpeg stops on, in its header or as it starts the scan, is
    /// `None`, not the end of the process.
    #[test]
    fn a_corrupt_jpeg_is_none() {
        let jpeg = corrupt::jpeg(64, 48);
        assert!(decode_scaled(&jpeg, 64).is_some(), "the JPEG as made");
        for (name, bad) in corrupt::kinds(&jpeg) {
            assert!(decode_scaled(&bad, 64).is_none(), "{name}");
        }
        // A scan cut short decodes, its missing rows filled in.
        assert!(decode_scaled(&jpeg[..jpeg.len() * 2 / 3], 64).is_some());
    }
}

/// JPEGs libjpeg stops on, for this crate's tests.
#[cfg(test)]
pub(crate) mod corrupt {
    /// A `width` × `height` RGB JPEG.
    pub(crate) fn jpeg(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x * 4) as u8, (y * 5) as u8, 90])
        });
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90)
            .encode_image(&image)
            .unwrap();
        jpeg
    }

    /// `jpeg` broken three ways, each named: no image after its header,
    /// a Huffman table of more than 256 codes (an error reading the
    /// header), and its first table moved to a slot its scan doesn't use
    /// (an error starting the scan).
    pub(crate) fn kinds(jpeg: &[u8]) -> Vec<(&'static str, Vec<u8>)> {
        let dht = jpeg.windows(2).position(|w| w == [0xFF, 0xC4]).expect("a DHT");
        let sos = jpeg.windows(2).position(|w| w == [0xFF, 0xDA]).expect("an SOS");
        let header_only = [&jpeg[..sos], &[0xFF, 0xD9]].concat();
        let mut too_many = jpeg.to_vec();
        too_many[dht + 5..dht + 21].fill(0xFF);
        let mut moved = jpeg.to_vec();
        moved[dht + 4] = (moved[dht + 4] & 0xF0) | 3;
        vec![("header only", header_only), ("too many codes", too_many), ("table moved", moved)]
    }
}
