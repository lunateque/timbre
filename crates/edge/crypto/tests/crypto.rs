//! What callers depend on: the decoder accepts exactly what the encoder
//! writes, random draws differ, and digests compare by their whole value.

use timbre_edge_crypto::failures::ENCODING_INVALID;
use timbre_edge_crypto::{base32, random_bytes, sha256};

#[test]
fn the_decoder_rejects_every_byte_outside_the_alphabet() {
    let good = base32::encode(&[1, 2, 3, 4, 5]);
    assert_eq!(good.len(), 8, "five bytes fill one group: {good}");
    for code in 0u8..=127 {
        let symbol = char::from(code);
        if symbol.is_ascii_lowercase() || ('2'..='7').contains(&symbol) {
            continue;
        }
        let text = format!("aaaaaaa{symbol}");
        let failure = base32::decode(&text).expect_err("a foreign byte is refused");
        assert_eq!(failure.code(), ENCODING_INVALID, "for {symbol:?}");
    }
    // Uppercase is foreign too: one secret has exactly one spelling.
    assert_eq!(
        base32::decode(&good.to_uppercase())
            .expect_err("uppercase is refused")
            .code(),
        ENCODING_INVALID
    );
}

#[test]
fn the_decoder_rejects_lengths_and_padding_bits_no_encoding_produces() {
    for length in [1usize, 3, 6, 9, 11] {
        let text = "a".repeat(length);
        assert_eq!(
            base32::decode(&text)
                .expect_err("a length no encoding writes is refused")
                .code(),
            ENCODING_INVALID,
            "for {length} symbols"
        );
    }
    // `aa` is one byte with three zero padding bits; `ab` sets one of them.
    assert_eq!(base32::decode("aa").expect("one byte"), vec![0u8]);
    assert_eq!(
        base32::decode("ab")
            .expect_err("non-zero padding bits are refused")
            .code(),
        ENCODING_INVALID
    );
    assert_eq!(
        base32::decode("").expect("nothing decodes to nothing"),
        Vec::<u8>::new()
    );
}

#[test]
fn two_draws_differ_and_fill_the_whole_buffer() {
    let first: [u8; 32] = random_bytes().expect("the system generator answers");
    let second: [u8; 32] = random_bytes().expect("the system generator answers");
    assert_ne!(first, second, "two 256-bit draws must not repeat");
    assert!(
        first.iter().any(|byte| *byte != 0),
        "a draw of zeros means the buffer was never filled"
    );
}

#[test]
fn a_digest_matches_only_its_own_value() {
    let digest = sha256(b"the-secret");
    assert!(digest.matches(&sha256(b"the-secret")));
    assert!(!digest.matches(&sha256(b"the-secrez")));
    let mut bytes = *digest.as_bytes();
    // Differing in the last byte only: a comparison that stops early would
    // still call these equal often enough to be useful to an attacker.
    let last = bytes.len() - 1;
    if let Some(byte) = bytes.get_mut(last) {
        *byte ^= 0x01;
    }
    assert!(!digest.matches(&timbre_edge_crypto::Digest::from_bytes(bytes)));
}

#[test]
fn a_digest_never_prints_its_bytes() {
    assert_eq!(format!("{:?}", sha256(b"x")), "***");
}
