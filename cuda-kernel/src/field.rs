//! The field, radix 2^32 in eight 32-bit limbs.
//!
//! Least significant limb first, held below `2^256` and folded with
//! `2^256 = 38 (mod p)`. Eight saturated limbs need sixty-four partial products
//! where ten redundant ones at radix 2^25.5 need a hundred, and an element is
//! 32 bytes instead of 40 — the same representation the processor path and the
//! other two kernels use.

/// A field element: eight 32-bit limbs, least significant first.
pub type Fe = [u32; 8];
