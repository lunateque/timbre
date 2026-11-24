//! Bounded art decoding and JPEG thumbnails.
#![warn(missing_docs)]

pub mod failures;

use std::io::Cursor;
use std::sync::{Condvar, Mutex, PoisonError};

use fast_image_resize::images::{Image as ResizeImage, ImageRef};
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use image::codecs::jpeg::JpegEncoder;
use image::{
    ColorType, DynamicImage, ExtendedColorType, ImageDecoder, ImageError, ImageFormat, ImageReader,
    RgbImage,
};
use timbre_kernel::{Failure, Fallible};

use failures::ImageFailure;

const MAX_PIXEL_BYTES: u64 = 256 * 1024 * 1024;

/// Limits for art handled by the edge.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Maximum width or height of the source and result.
    pub max_dimension: u32,
    /// Maximum compressed source length, in bytes.
    pub max_bytes: usize,
}

/// Decoded art in RGB8, ready for resizing or JPEG encoding.
#[derive(Debug)]
pub struct DecodedImage(RgbImage);

impl DecodedImage {
    /// The width and height, in pixels.
    pub fn dimensions(&self) -> (u32, u32) {
        self.0.dimensions()
    }
}

fn limit() -> Box<dyn Failure> {
    Box::new(ImageFailure::Limit)
}

fn decode_error(error: &ImageError) -> Box<dyn Failure> {
    match error {
        ImageError::Limits(_) => limit(),
        _ => Box::new(ImageFailure::Decode(error.to_string())),
    }
}

/// Read JPEG, PNG or WebP art after checking the encoded size and pixel dimensions.
///
/// # Errors
/// Returns a typed unsupported, size-limit or decode failure.
pub fn decode(bytes: &[u8], limits: Limits) -> Fallible<DecodedImage> {
    let _permit = Permit::acquire();
    decode_held(bytes, limits)
}

fn decode_held(bytes: &[u8], limits: Limits) -> Fallible<DecodedImage> {
    if bytes.len() > limits.max_bytes {
        return Err(limit());
    }
    let Ok(format @ (ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::WebP)) =
        image::guess_format(bytes)
    else {
        return Err(Box::new(ImageFailure::Unsupported));
    };
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut decoder_limits = image::Limits::default();
    decoder_limits.max_image_width = Some(limits.max_dimension);
    decoder_limits.max_image_height = Some(limits.max_dimension);
    decoder_limits.max_alloc = Some(MAX_PIXEL_BYTES);
    reader.limits(decoder_limits);
    let decoder = reader
        .into_decoder()
        .map_err(|error| decode_error(&error))?;
    let (width, height) = decoder.dimensions();
    // The decoded source and the RGB8 copy are alive at the same time, so the
    // budget covers both: 8192x8192 RGBA needs 256 MiB plus 192 MiB.
    let total = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(3))
        .and_then(|rgb| rgb.checked_add(decoder.total_bytes()));
    if width == 0
        || height == 0
        || width > limits.max_dimension
        || height > limits.max_dimension
        || total.is_none_or(|total| total > MAX_PIXEL_BYTES)
    {
        return Err(limit());
    }
    if decoder.color_type() == ColorType::Rgb8 {
        let size = usize::try_from(decoder.total_bytes()).map_err(|_| limit())?;
        let mut pixels = vec![0; size];
        decoder
            .read_image(&mut pixels)
            .map_err(|error| decode_error(&error))?;
        return RgbImage::from_raw(width, height, pixels)
            .map(DecodedImage)
            .ok_or_else(limit);
    }
    DynamicImage::from_decoder(decoder)
        .map(|image| DecodedImage(image.into_rgb8()))
        .map_err(|error| decode_error(&error))
}

/// Resize with Lanczos3, preserving aspect ratio and setting the longer edge to `size`.
/// A shorter edge rounds down to a minimum of one pixel.
///
/// # Errors
/// Returns a size-limit or thumbnail failure.
pub fn resize(source: DecodedImage, size: u32, limits: Limits) -> Fallible<DecodedImage> {
    if size == 0 || size > limits.max_dimension {
        return Err(limit());
    }
    let (width, height) = source.dimensions();
    let long_edge = width.max(height);
    if size == long_edge {
        return Ok(source);
    }
    let scaled = |dimension| {
        u32::try_from((u64::from(dimension) * u64::from(size) / u64::from(long_edge)).max(1))
            .map_err(|_| limit())
    };
    let (out_width, out_height) = (scaled(width)?, scaled(height)?);
    if u64::from(out_width) * u64::from(out_height) > MAX_PIXEL_BYTES / 3 {
        return Err(limit());
    }
    let resize_failure =
        |diagnostic| -> Box<dyn Failure> { Box::new(ImageFailure::Thumbnail(diagnostic)) };
    let input = ImageRef::new(width, height, source.0.as_raw(), PixelType::U8x3)
        .map_err(|error| resize_failure(error.to_string()))?;
    let mut output = ResizeImage::new(out_width, out_height, PixelType::U8x3);
    let options = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3));
    Resizer::new()
        .resize(&input, &mut output, &options)
        .map_err(|error| resize_failure(error.to_string()))?;
    RgbImage::from_raw(out_width, out_height, output.into_vec())
        .map(DecodedImage)
        .ok_or_else(|| resize_failure("unexpected resize buffer length".to_owned()))
}

/// Encode RGB art as JPEG at quality 80.
///
/// # Errors
/// Returns a thumbnail failure if the encoder cannot represent the image.
pub fn encode_jpeg(source: &DecodedImage) -> Fallible<Vec<u8>> {
    let (width, height) = source.dimensions();
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, 80)
        .encode(source.0.as_raw(), width, height, ExtendedColorType::Rgb8)
        .map_err(|error| {
            Box::new(ImageFailure::Thumbnail(error.to_string())) as Box<dyn Failure>
        })?;
    Ok(bytes)
}

/// Decode, resize and encode a JPEG thumbnail. At most
/// [`MAX_CONCURRENT`] threads hold decoded pixels at once.
///
/// # Errors
/// Returns a typed failure from decoding, resizing or encoding.
pub fn thumbnail(bytes: &[u8], size: u32, limits: Limits) -> Fallible<Vec<u8>> {
    let _permit = Permit::acquire();
    encode_jpeg(&resize(decode_held(bytes, limits)?, size, limits)?)
}

/// Threads allowed to hold decoded pixels at the same time. Every scan worker
/// decoding its own cover at once would multiply the pixel budget by the pool.
pub const MAX_CONCURRENT: u32 = 2;

static DECODING: Mutex<u32> = Mutex::new(0);
static FINISHED: Condvar = Condvar::new();

struct Permit;

impl Permit {
    fn acquire() -> Self {
        let mut decoding = DECODING.lock().unwrap_or_else(PoisonError::into_inner);
        while *decoding >= MAX_CONCURRENT {
            decoding = FINISHED
                .wait(decoding)
                .unwrap_or_else(PoisonError::into_inner);
        }
        *decoding += 1;
        Self
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut decoding = DECODING.lock().unwrap_or_else(PoisonError::into_inner);
        *decoding = decoding.saturating_sub(1);
        FINISHED.notify_one();
    }
}

/// Stable content identity for original artwork and generated thumbnails.
pub fn content_hash(bytes: &[u8]) -> u128 {
    xxhash_rust::xxh3::xxh3_128(bytes)
}

/// Determine the media type from the encoded bytes, without allocating pixels.
pub fn mime(bytes: &[u8]) -> Option<&'static str> {
    match image::guess_format(bytes).ok()? {
        ImageFormat::Jpeg => Some("image/jpeg"),
        ImageFormat::Png => Some("image/png"),
        ImageFormat::WebP => Some("image/webp"),
        _ => None,
    }
}

/// The shipped image parser entry points.
#[cfg(feature = "fuzzing")]
pub mod fuzzing {
    pub use crate::{decode, thumbnail};
}
