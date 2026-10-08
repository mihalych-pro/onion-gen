//! Key pair and Onion Service v3 address derivation.
//!
//! The chain is `seed -> SHA-512 -> clamping -> sk -> A = sk*B -> address`.

use crate::base32;
use crate::curve::{self, Point};
use crate::field::Fe;
use sha2::{Digest, Sha512};
use sha3::Sha3_256;

/// The Onion Service v3 version byte. It is also what makes the address tail
/// predictable: every address ends with `d`.
pub const VERSION: u8 = 0x03;

/// The checksum salt fixed by the protocol.
const CHECKSUM_SALT: &[u8] = b".onion checksum";

/// Address length without the `.onion` suffix: `35 bytes * 8 / 5 = 56`.
pub const ADDRESS_LEN: usize = 56;

/// The secret scalar derived from a seed: the first half of SHA-512 with the
/// mandatory clamping applied. Without it tor rejects the key.
pub fn secret_scalar(seed: &[u8; 32]) -> [u8; 32] {
    let digest = Sha512::digest(seed);
    let mut sk = [0u8; 32];
    sk.copy_from_slice(&digest[..32]);
    clamp(&mut sk);
    sk
}

/// The full secret key in tor's format: 64 bytes, both halves of SHA-512,
/// the first one clamped.
pub fn expanded_secret_key(seed: &[u8; 32]) -> [u8; 64] {
    let digest = Sha512::digest(seed);
    let mut sk = [0u8; 64];
    sk.copy_from_slice(&digest);
    clamp_slice(&mut sk[..32]);
    sk
}

#[inline]
fn clamp(sk: &mut [u8; 32]) {
    clamp_slice(&mut sk[..]);
}

#[inline]
fn clamp_slice(sk: &mut [u8]) {
    sk[0] &= 248;
    sk[31] &= 63;
    sk[31] |= 64;
}

/// Packs a point into 32 bytes: the `y` coordinate plus the sign bit of `x`
/// in the top bit of the last byte.
pub fn pack(p: &Point) -> [u8; 32] {
    let zinv = p.z.invert();
    let y = p.y.mul(&zinv);
    let x = p.x.mul(&zinv);
    let mut out = y.to_bytes();
    out[31] ^= x.is_negative() << 7;
    out
}

/// Whether this expanded secret really produces this public key, according to
/// an implementation that shares no code with ours.
///
/// A found key is the product. Everything before it — the vectorised field, the
/// group chain, the device kernel, the offset bookkeeping — is machinery whose
/// failure mode is not a crash but a key that does not open its address. Our own
/// arithmetic confirming our own arithmetic would prove nothing, so this asks
/// `curve25519-dalek`.
///
/// It runs once per hit and never in the hot loop: one hit is a thousand
/// million candidates apart from the next, and a hit already costs a directory
/// and three files on disk.
pub fn secret_produces(secret: &[u8; 64], public_key: &[u8; 32]) -> bool {
    public_key_from_secret(secret) == *public_key
}

/// The public key an expanded secret really produces, according to
/// `curve25519-dalek`.
///
/// Split out from [`secret_produces`] so that a failure can say what the other
/// implementation got, not only that it disagreed.
pub fn public_key_from_secret(secret: &[u8; 64]) -> [u8; 32] {
    use curve25519_dalek::edwards::EdwardsPoint;
    use curve25519_dalek::scalar::Scalar;

    let mut scalar = [0u8; 32];
    scalar.copy_from_slice(&secret[..32]);
    EdwardsPoint::mul_base(&Scalar::from_bytes_mod_order(scalar))
        .compress()
        .to_bytes()
}

/// The public key for a seed. This one-key-at-a-time path serves checks and
/// hit recovery; the hot loop goes through the batch engine instead.
pub fn public_key(seed: &[u8; 32]) -> [u8; 32] {
    let sk = secret_scalar(seed);
    let point = curve::scalar_base_mult(&sk, &Point::basepoint(), &curve::two_d());
    pack(&point)
}

/// The address checksum: the first two bytes of
/// `SHA3-256(".onion checksum" || pubkey || version)`.
pub fn checksum(public_key: &[u8; 32]) -> [u8; 2] {
    let mut hasher = Sha3_256::new();
    hasher.update(CHECKSUM_SALT);
    hasher.update(public_key);
    hasher.update([VERSION]);
    let digest = hasher.finalize();
    [digest[0], digest[1]]
}

/// The address without the `.onion` suffix — 56 base32 symbols.
pub fn address(public_key: &[u8; 32]) -> String {
    base32::encode(&address_bytes(public_key))
}

/// The host name in the form tor expects.
pub fn hostname(public_key: &[u8; 32]) -> String {
    format!("{}.onion", address(public_key))
}

/// The 35 encoded bytes: `pubkey || checksum || version`.
pub fn address_bytes(public_key: &[u8; 32]) -> [u8; 35] {
    let mut buf = [0u8; 35];
    buf[..32].copy_from_slice(public_key);
    buf[32..34].copy_from_slice(&checksum(public_key));
    buf[34] = VERSION;
    buf
}

/// The public key as stored in `hs_ed25519_public_key`, without the prefix.
pub fn public_key_bytes(seed: &[u8; 32]) -> [u8; 32] {
    public_key(seed)
}

/// Parses an address back into a public key, verifying the checksum. Used to
/// cross-check hits and in tests.
pub fn public_key_from_address(address: &str) -> Result<[u8; 32], String> {
    let address = address.strip_suffix(".onion").unwrap_or(address);
    if address.len() != ADDRESS_LEN {
        return Err(format!(
            "an address must be {ADDRESS_LEN} symbols long, got {}",
            address.len()
        ));
    }
    let (bytes, _) = base32::decode_bits(address).map_err(|e| e.to_string())?;
    let mut pk = [0u8; 32];
    pk.copy_from_slice(&bytes[..32]);
    if bytes[34] != VERSION {
        return Err(format!("unknown address version: {:#04x}", bytes[34]));
    }
    let expected = checksum(&pk);
    if bytes[32..34] != expected {
        return Err("checksum mismatch".to_string());
    }
    Ok(pk)
}

/// The full 64-byte secret of the candidate at `offset`, in tor's layout.
///
/// Only the left half moves: it is the scalar being searched. The right half is
/// the nonce prefix used for signing and plays no part in the public key, so it
/// is carried over unchanged.
pub fn expanded_secret_at_offset(seed: &[u8; 32], offset: u64) -> Option<[u8; 64]> {
    let mut expanded = expanded_secret_key(seed);
    let mut scalar = [0u8; 32];
    scalar.copy_from_slice(&expanded[..32]);
    add_to_scalar(&mut scalar, offset);
    if !clamping_intact(&scalar) {
        return None;
    }
    expanded[..32].copy_from_slice(&scalar);
    Some(expanded)
}

/// Adds `delta` to a 256-bit little-endian scalar in place.
///
/// This is how a candidate's secret key is recovered: the candidate at offset
/// `i` of a batch corresponds to the seed's scalar plus `8i`, and the addition
/// is done directly on the clamped scalar. Overflow past the top byte wraps,
/// which [`clamping_intact`] then rejects.
pub fn add_to_scalar(sk: &mut [u8; 32], delta: u64) {
    let mut carry = u128::from(delta);
    for byte in sk.iter_mut() {
        if carry == 0 {
            break;
        }
        carry += u128::from(*byte);
        *byte = (carry & 0xff) as u8;
        carry >>= 8;
    }
}

/// Whether a scalar still satisfies the ed25519 clamping constraints.
///
/// Advancing the counter can carry into the high bits and break them. The
/// reference implementation discards the whole seed in that case
/// (`worker_batch.inc.h:93-94`), losing any hit found in that batch; the check
/// is cheap and must not be skipped, because a key failing it is not a valid
/// ed25519 key.
pub fn clamping_intact(sk: &[u8; 32]) -> bool {
    sk[0] & 248 == sk[0] && ((sk[31] & 63) | 64) == sk[31]
}

/// The secret scalar of the candidate sitting at `offset` past a seed's own
/// scalar, or `None` if the offset broke clamping.
pub fn scalar_at_offset(seed: &[u8; 32], offset: u64) -> Option<[u8; 32]> {
    let mut sk = secret_scalar(seed);
    add_to_scalar(&mut sk, offset);
    clamping_intact(&sk).then_some(sk)
}

/// Applies the sign bit of `x` to an already packed `y`. Used when the batch
/// packed only `y` and the sign is needed for a candidate that matched a
/// filter.
pub fn apply_sign_bit(packed: &mut [u8; 32], x: &Fe, zinv: &Fe) {
    packed[31] ^= x.mul(zinv).is_negative() << 7;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed_from(n: u64) -> [u8; 32] {
        let mut b = [0u8; 32];
        let mut x = n
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(0x1234_5678);
        for chunk in b.chunks_mut(8) {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            chunk.copy_from_slice(&x.to_le_bytes());
        }
        b
    }

    /// The check that guards every hit has to reject a pair that does not
    /// belong together, or it guards nothing.
    #[test]
    fn the_hit_check_accepts_only_a_real_pair() {
        for n in 0..200u64 {
            let seed = seed_from(n);
            let secret = expanded_secret_key(&seed);
            let public = public_key(&seed);
            assert!(
                secret_produces(&secret, &public),
                "seed #{n} is a real pair"
            );

            // Somebody else's key.
            let other = public_key(&seed_from(n + 1));
            assert!(
                !secret_produces(&secret, &other),
                "seed #{n} against another"
            );

            // One bit of the secret scalar moved. Clamping means not every
            // change of a byte changes the key, so this moves a bit the
            // clamping keeps.
            let mut bent = secret;
            bent[8] ^= 1;
            assert!(
                !secret_produces(&bent, &public),
                "seed #{n} with a bent secret"
            );

            // One bit of the address moved.
            let mut elsewhere = public;
            elsewhere[3] ^= 1;
            assert!(!secret_produces(&secret, &elsewhere), "seed #{n} elsewhere");
        }
    }

    /// Differential test against curve25519-dalek: our `seed -> public key`
    /// derivation must match an independent implementation byte for byte.
    #[test]
    fn public_key_matches_curve25519_dalek() {
        use curve25519_dalek::edwards::EdwardsPoint;
        use curve25519_dalek::scalar::Scalar;

        for n in 0..1000u64 {
            let seed = seed_from(n);
            let ours = public_key(&seed);

            let sk = secret_scalar(&seed);
            let theirs = EdwardsPoint::mul_base(&Scalar::from_bytes_mod_order(sk))
                .compress()
                .to_bytes();

            assert_eq!(ours, theirs, "seed #{n}");
        }
    }

    #[test]
    fn clamping_follows_ed25519() {
        for n in 0..100u64 {
            let sk = secret_scalar(&seed_from(n));
            assert_eq!(sk[0] & 7, 0, "the low three bits must be cleared");
            assert_eq!(sk[31] & 128, 0, "the top bit must be cleared");
            assert_eq!(sk[31] & 64, 64, "bit 254 must be set");
        }
    }

    #[test]
    fn address_has_protocol_shape() {
        for n in 0..100u64 {
            let pk = public_key(&seed_from(n));
            let addr = address(&pk);
            assert_eq!(addr.len(), ADDRESS_LEN, "address #{n}");
            assert!(
                addr.bytes().all(|c| base32::symbol_value(c).is_some()),
                "address #{n} contains a symbol outside the alphabet"
            );
            // The version byte 0x03 occupies the low 5 bits of the last symbol.
            assert!(addr.ends_with('d'), "address #{n} must end with d");
            // The penultimate symbol is 2 checksum bits plus three zero version bits.
            let penultimate = addr.chars().nth(ADDRESS_LEN - 2).unwrap();
            assert!(
                "aiqy".contains(penultimate),
                "penultimate symbol of address #{n}: {penultimate}"
            );
        }
    }

    #[test]
    fn address_roundtrips_through_checksum() {
        for n in 0..100u64 {
            let pk = public_key(&seed_from(n));
            let addr = hostname(&pk);
            assert_eq!(public_key_from_address(&addr).unwrap(), pk, "address #{n}");
        }
    }

    #[test]
    fn corrupted_address_is_rejected() {
        let pk = public_key(&seed_from(1));
        let addr = address(&pk);
        // Corrupt one symbol inside the key body; the checksum stops matching.
        let mut corrupted: Vec<char> = addr.chars().collect();
        corrupted[0] = if corrupted[0] == 'a' { 'b' } else { 'a' };
        let corrupted: String = corrupted.into_iter().collect();
        assert!(public_key_from_address(&corrupted).is_err());
    }

    #[test]
    fn scalar_addition_is_little_endian() {
        let mut sk = [0u8; 32];
        add_to_scalar(&mut sk, 1);
        assert_eq!(sk[0], 1);

        let mut sk = [0xffu8; 32];
        sk[1] = 0;
        add_to_scalar(&mut sk, 1);
        assert_eq!(sk[0], 0, "the low byte must wrap");
        assert_eq!(sk[1], 1, "the carry must reach the next byte");
    }

    #[test]
    fn clamping_check_rejects_broken_scalars() {
        let sk = secret_scalar(&seed_from(3));
        assert!(clamping_intact(&sk));

        let mut low = sk;
        low[0] |= 1;
        assert!(!clamping_intact(&low), "a set low bit must be rejected");

        let mut high = sk;
        high[31] |= 128;
        assert!(!clamping_intact(&high), "a set top bit must be rejected");

        let mut missing = sk;
        missing[31] &= !64;
        assert!(
            !clamping_intact(&missing),
            "a cleared bit 254 must be rejected"
        );
    }

    /// Recovering a candidate's scalar must reproduce the public key the batch
    /// engine derived for that same offset.
    #[test]
    fn offset_scalar_yields_the_candidate_key() {
        use crate::curve::{self, Point};

        let seed = seed_from(21);
        for i in [0u64, 1, 7, 2047] {
            let offset = 8 * i;
            let sk = scalar_at_offset(&seed, offset).expect("clamping must hold here");
            let from_scalar = pack(&curve::scalar_base_mult(
                &sk,
                &Point::basepoint(),
                &curve::two_d(),
            ));

            let mut acc = curve::scalar_base_mult(
                &secret_scalar(&seed),
                &Point::basepoint(),
                &curve::two_d(),
            );
            let step = curve::eight_basepoint_cached();
            for _ in 0..i {
                acc = curve::to_p3(&curve::add(&acc, &step));
            }
            assert_eq!(from_scalar, pack(&acc), "offset {offset}");
        }
    }

    #[test]
    fn expanded_secret_key_matches_scalar() {
        for n in 0..50u64 {
            let seed = seed_from(n);
            assert_eq!(&expanded_secret_key(&seed)[..32], &secret_scalar(&seed)[..]);
        }
    }
}
