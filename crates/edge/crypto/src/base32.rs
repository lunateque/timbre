//! Lowercase RFC 4648 base32 without padding: the text form of every secret
//! this server hands out.
//!
//! Base32 over base62 or hex: a secret is read aloud, retyped and pasted into
//! client fields, and this alphabet has one case, no padding to lose, and no
//! `0`/`O` or `1`/`l` pair to confuse; hex would make the same secret half as
//! long again, and base62's mixed case breaks the moment a client lowercases
//! a header value. The decoder is strict, so one secret has exactly one
//! spelling: non-alphabet bytes, lengths that could not come from encoding,
//! and non-zero padding bits are all refused rather than silently accepted,
//! which is what lets a lookup key be compared as text.

use crate::failures::CryptoFailure;
use timbre_kernel::Fallible;

/// The symbol of the low five bits of `value`: `a`-`z` then `2`-`7`.
const fn symbol(value: u8) -> u8 {
    match value & 0x1f {
        bits @ 0..=25 => b'a' + bits,
        bits => b'2' + (bits - 26),
    }
}

/// The five-bit value of one symbol, or `None` outside the alphabet.
const fn value(symbol: u8) -> Option<u8> {
    match symbol {
        b'a'..=b'z' => Some(symbol - b'a'),
        b'2'..=b'7' => Some(symbol - b'2' + 26),
        _ => None,
    }
}

/// Encode `bytes` as lowercase base32 without padding.
#[must_use]
pub fn encode(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let mut buffer: u16 = 0;
    let mut bits: u32 = 0;
    for byte in bytes {
        buffer = (buffer << 8) | u16::from(*byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            text.push(char::from(symbol(low_byte(buffer >> bits))));
        }
    }
    if bits > 0 {
        // The last group is padded with zero bits, which is what the decoder
        // insists on seeing.
        text.push(char::from(symbol(low_byte(buffer << (5 - bits)))));
    }
    text
}

/// Decode canonical lowercase base32.
///
/// # Errors
///
/// `crypto.encoding_invalid` for a byte outside the alphabet, a length no
/// encoding can produce, or non-zero padding bits.
pub fn decode(text: &str) -> Fallible<Vec<u8>> {
    let mut bytes = Vec::with_capacity(text.len() * 5 / 8);
    let mut buffer: u16 = 0;
    let mut bits: u32 = 0;
    for symbol in text.bytes() {
        let Some(value) = value(symbol) else {
            return Err(CryptoFailure::EncodingInvalid.boxed());
        };
        buffer = (buffer << 5) | u16::from(value);
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            bytes.push(low_byte(buffer >> bits));
        }
    }
    // Five or more leftover bits mean a symbol that encodes nothing, and
    // leftover bits that are not zero mean a spelling `encode` never writes.
    let leftover = buffer & mask(bits);
    if bits >= 5 || leftover != 0 {
        return Err(CryptoFailure::EncodingInvalid.boxed());
    }
    Ok(bytes)
}

/// The low eight bits of `value`; the mask makes the conversion total.
fn low_byte(value: u16) -> u8 {
    u8::try_from(value & 0xff).unwrap_or_default()
}

/// A mask of the low `bits` bits, for `bits` below 8.
const fn mask(bits: u32) -> u16 {
    (1u16 << bits) - 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_round_trips_every_length_up_to_a_full_group() {
        for length in 0..=10usize {
            let bytes: Vec<u8> = (0..length)
                .map(|index| u8::try_from(index * 37 % 251).unwrap_or(0))
                .collect();
            let text = encode(&bytes);
            assert_eq!(decode(&text).expect("canonical text decodes"), bytes);
        }
    }

    #[test]
    fn the_alphabet_is_lowercase_rfc_4648() {
        assert_eq!(encode(&[0x00]), "aa");
        assert_eq!(encode(&[0xff, 0xff]), "777q");
        assert_eq!(encode(b"timbre"), "oruw2ytsmu");
    }
}
