//! Thumbnail dimensions and the encoded input byte limit.

use timbre_edge_image::{Limits, failures::LIMIT, fuzzing};

/// Exercise bounded decoders and check the JPEG independently of the resize result.
///
/// # Panics
/// Panics if limits are ignored or successful output does not have the
/// requested dimensions.
pub fn image_thumbnail(bytes: &[u8]) {
    let Some((&request, source)) = bytes.split_first() else {
        return;
    };
    let size = u32::from(request) + 1;
    let limits = Limits {
        max_dimension: 1024,
        max_bytes: 256 * 1024,
    };
    if source.len() > limits.max_bytes {
        assert_eq!(
            fuzzing::thumbnail(source, size, limits).unwrap_err().code(),
            LIMIT
        );
        return;
    }
    if !source.is_empty() {
        let byte_cap = Limits {
            max_bytes: source.len() - 1,
            ..limits
        };
        assert_eq!(fuzzing::decode(source, byte_cap).unwrap_err().code(), LIMIT);
    }
    let Ok(decoded) = fuzzing::decode(source, limits) else {
        return;
    };
    let (width, height) = decoded.dimensions();
    drop(decoded);
    let longest = width.max(height);
    assert!((1..=limits.max_dimension).contains(&longest));
    let dimension_cap = Limits {
        max_dimension: longest - 1,
        ..limits
    };
    assert_eq!(
        fuzzing::decode(source, dimension_cap).unwrap_err().code(),
        LIMIT
    );
    if let Ok(jpeg) = fuzzing::thumbnail(source, size, limits) {
        assert!(jpeg.starts_with(&[0xff, 0xd8]));
        let output = fuzzing::decode(
            &jpeg,
            Limits {
                max_bytes: jpeg.len(),
                ..limits
            },
        )
        .unwrap();
        assert_eq!(
            output.dimensions(),
            (
                (width * size / longest).max(1),
                (height * size / longest).max(1)
            )
        );
    }
}
