//! Lowercase RFC 4648 base32 — the encoding an Onion Service v3 address is
//! written in.
//!
//! The alphabet holds 26 letters and the digits `2`-`7`; there is no `0`, `1`,
//! `8` or `9`, so an address containing one is impossible. A filter with such a
//! digit is rejected at parse time rather than searched for forever.

pub const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// A base32 parse error, carrying the offending character's position.
#[derive(Debug, PartialEq, Eq)]
pub struct DecodeError {
    pub position: usize,
    pub character: char,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "invalid character {:?} at position {}: the base32 alphabet is a-z and 2-7",
            self.character, self.position
        )
    }
}

impl std::error::Error for DecodeError {}

/// Reverse alphabet lookup: the symbol's index, or `None`.
#[inline]
pub fn symbol_value(c: u8) -> Option<u8> {
    match c {
        b'a'..=b'z' => Some(c - b'a'),
        b'2'..=b'7' => Some(c - b'2' + 26),
        _ => None,
    }
}

/// Encodes an arbitrary number of bytes. The tail is padded with zero bits
/// and no padding characters are emitted: the lengths an address works with
/// divide evenly into five-bit groups.
pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let mut acc: u16 = 0;
    let mut bits: u8 = 0;
    for &b in bytes {
        acc = (acc << 8) | b as u16;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            let idx = ((acc >> bits) & 0x1f) as usize;
            out.push(ALPHABET[idx] as char);
        }
    }
    if bits > 0 {
        let idx = ((acc << (5 - bits)) & 0x1f) as usize;
        out.push(ALPHABET[idx] as char);
    }
    out
}

/// Encodes `out.len()` symbols of `bytes` into `out`, allocating nothing.
///
/// The hot loop needs the text of a candidate to search it for a substring, and
/// it needs it for every candidate. Building a `String` each time measured four
/// times the cost of this and was mostly the allocator.
///
/// Panics if `bytes` is too short for the symbols asked for.
#[inline]
pub fn encode_into(bytes: &[u8], out: &mut [u8]) {
    assert!(out.len() * 5 <= bytes.len() * 8 + 4, "not enough bytes");
    for (i, slot) in out.iter_mut().enumerate() {
        let bit = i * 5;
        let byte = bit / 8;
        let shift = bit % 8;
        // The five bits start `shift` bits into `byte` and may run into the
        // next one. The last symbol of an odd-sized buffer takes its missing
        // low bits as zero, which is what `encode` pads with.
        let mut value = (bytes[byte] << shift) >> 3;
        if shift > 3 {
            value |= bytes.get(byte + 1).copied().unwrap_or(0) >> (11 - shift);
        }
        *slot = ALPHABET[(value & 31) as usize];
    }
}

/// Decodes into bytes together with the number of significant bits.
///
/// The bit count is returned separately because a filter of arbitrary symbol
/// length almost never ends on a byte boundary: `m` symbols are `5m` bits, and
/// the last byte is only partially significant.
pub fn decode_bits(s: &str) -> Result<(Vec<u8>, usize), DecodeError> {
    let mut out = Vec::with_capacity(s.len() * 5 / 8 + 1);
    let mut acc: u16 = 0;
    let mut bits: u8 = 0;
    for (position, c) in s.bytes().enumerate() {
        let v = symbol_value(c).ok_or(DecodeError {
            position,
            character: s[position..].chars().next().unwrap_or('?'),
        })?;
        acc = (acc << 5) | v as u16;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    if bits > 0 {
        out.push(((acc << (8 - bits)) & 0xff) as u8);
    }
    Ok((out, s.len() * 5))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alphabet_excludes_confusable_digits() {
        for c in *b"0189" {
            assert!(
                symbol_value(c).is_none(),
                "{} must not be accepted",
                c as char
            );
        }
        assert_eq!(symbol_value(b'a'), Some(0));
        assert_eq!(symbol_value(b'z'), Some(25));
        assert_eq!(symbol_value(b'2'), Some(26));
        assert_eq!(symbol_value(b'7'), Some(31));
    }

    #[test]
    fn roundtrip_on_whole_blocks() {
        // 35 bytes = 56 symbols exactly — precisely the v3 address case.
        for seed in 0u8..64 {
            let data: Vec<u8> = (0..35)
                .map(|i| seed.wrapping_mul(31).wrapping_add(i))
                .collect();
            let encoded = encode(&data);
            assert_eq!(encoded.len(), 56);
            let (decoded, bits) = decode_bits(&encoded).unwrap();
            assert_eq!(bits, 280);
            assert_eq!(&decoded[..35], &data[..], "seed {seed}");
        }
    }

    #[test]
    fn roundtrip_on_partial_blocks() {
        for len in 1..=8usize {
            let data: Vec<u8> = (0..len as u8).map(|i| i.wrapping_mul(37)).collect();
            let encoded = encode(&data);
            let (decoded, _) = decode_bits(&encoded).unwrap();
            assert_eq!(&decoded[..len], &data[..], "length {len}");
        }
    }

    #[test]
    fn rejects_invalid_character_with_position() {
        let err = decode_bits("ab0cd").unwrap_err();
        assert_eq!(err.position, 2);
        assert_eq!(err.character, '0');
        let err = decode_bits("zzzz1").unwrap_err();
        assert_eq!(err.position, 4);
    }

    #[test]
    fn partial_symbol_counts_bits_not_bytes() {
        // Two symbols are 10 bits, i.e. two bytes, the second significant in 2 bits.
        let (bytes, bits) = decode_bits("zz").unwrap();
        assert_eq!(bits, 10);
        assert_eq!(bytes.len(), 2);
    }

    /// The allocating encoder is the reference; the one that writes into a
    /// buffer must agree with it symbol for symbol.
    #[test]
    fn encoding_into_a_buffer_agrees_with_encoding_to_a_string() {
        let mut bytes = [0u8; 35];
        for seed in 0u32..256 {
            for (i, b) in bytes.iter_mut().enumerate() {
                *b = (seed as u8).wrapping_mul(31).wrapping_add(i as u8 * 7);
            }
            let reference = encode(&bytes);
            for symbols in [1usize, 5, 51, 56] {
                let mut out = vec![0u8; symbols];
                encode_into(&bytes, &mut out);
                assert_eq!(
                    std::str::from_utf8(&out).unwrap(),
                    &reference[..symbols],
                    "seed {seed}, {symbols} symbols"
                );
            }
        }
    }
}
