#![expect(
    clippy::unwrap_used,
    reason = "fixture setup failures must fail the test"
)]

use timbre_edge_image::{Limits, decode, failures, thumbnail};

const LIMITS: Limits = Limits {
    max_dimension: 64,
    max_bytes: 4096,
};

#[test]
fn rejects_oversized_header_before_reading_truncated_pixels() {
    // The scan header is complete, but its compressed pixels are missing.
    let header = [
        0xff, 0xd8, 0xff, 0xc0, 0, 11, 8, 0, 128, 1, 0, 1, 1, 0x11, 0, 0xff, 0xda, 0, 8, 1, 1, 0,
        0, 63, 0,
    ];
    assert_eq!(decode(&header, LIMITS).unwrap_err().code(), failures::LIMIT);
    assert_eq!(
        decode(
            &header,
            Limits {
                max_bytes: 8,
                ..LIMITS
            }
        )
        .unwrap_err()
        .code(),
        failures::LIMIT
    );
    assert_eq!(
        decode(
            &header,
            Limits {
                max_dimension: 512,
                ..LIMITS
            }
        )
        .unwrap_err()
        .code(),
        failures::DECODE
    );
}

#[test]
fn names_unsupported_formats() {
    for data in [
        b"not an image".as_slice(),
        b"\0\0\0\x18ftypavif\0\0\0\0avifmif1",
    ] {
        assert_eq!(
            decode(data, LIMITS).unwrap_err().code(),
            failures::UNSUPPORTED
        );
    }
}

#[test]
fn decodes_supported_formats_and_encodes_jpeg_at_the_requested_size() {
    let pixels = image::RgbImage::from_pixel(12, 8, image::Rgb([255, 64, 32]));
    for format in [
        image::ImageFormat::Jpeg,
        image::ImageFormat::Png,
        image::ImageFormat::WebP,
    ] {
        let mut data = std::io::Cursor::new(Vec::new());
        pixels.write_to(&mut data, format).unwrap();
        let jpeg = thumbnail(data.get_ref(), 6, LIMITS).unwrap();
        assert!(jpeg.starts_with(&[0xff, 0xd8]));
        assert_eq!(decode(&jpeg, LIMITS).unwrap().dimensions(), (6, 4));
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

fn chunk(bytes: &mut Vec<u8>, name: &[u8], data: &[u8]) {
    let mut body = name.to_vec();
    body.extend_from_slice(data);
    let size = u32::try_from(data.len()).unwrap();
    bytes.extend_from_slice(&size.to_be_bytes());
    let crc = crc32(&body);
    bytes.extend_from_slice(&body);
    bytes.extend_from_slice(&crc.to_be_bytes());
}

fn png_header(width: u32, height: u32, color: u8) -> Vec<u8> {
    let mut header = width.to_be_bytes().to_vec();
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, color, 0, 0, 0]);
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    chunk(&mut bytes, b"IHDR", &header);
    chunk(&mut bytes, b"IDAT", &[0x78, 0x01, 0x01, 0x00]);
    chunk(&mut bytes, b"IEND", &[]);
    bytes
}

#[test]
fn rejects_pixels_that_do_not_fit_the_budget_with_their_rgb_copy() {
    // 8192x8192 RGBA decodes to 256 MiB and then copies 192 MiB more.
    let limits = Limits {
        max_dimension: 8192,
        max_bytes: 4096,
    };
    let header = png_header(8192, 8192, 6);
    assert_eq!(decode(&header, limits).unwrap_err().code(), failures::LIMIT);
    // The same dimensions in RGB8 fit, so the header is read and the pixels are missing.
    let header = png_header(4096, 4096, 2);
    assert_eq!(
        decode(&header, limits).unwrap_err().code(),
        failures::DECODE
    );
}
