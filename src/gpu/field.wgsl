// Field arithmetic for the portable path: radix 2^32 in eight 32-bit limbs,
// least significant first, held below 2^256 and folded with 2^256 = 38.
//
// Eight saturated limbs need sixty-four partial products where ten redundant
// ones at radix 2^25.5 need a hundred, and an element is 32 bytes instead of
// 40. Both matter: this kernel is bound by what it moves and by how many
// multiplies it issues, and the saturated form is cheaper on each count.
//
// Every index into a limb array is a literal. That is not a matter of taste:
// an array a shader indexes with a loop variable cannot live in registers, and
// the whole working set lands in off-chip private memory. The rows are
// therefore written out.

alias Fe = array<u32, 8>;

fn m(a: u32, b: u32) -> u64 {
    // A widening multiply, which the backend emits as one instruction.
    return u64(a) * u64(b);
}

fn fe_mul(f: Fe, g: Fe) -> Fe {
    var p0: u32 = 0u;
    var p1: u32 = 0u;
    var p2: u32 = 0u;
    var p3: u32 = 0u;
    var p4: u32 = 0u;
    var p5: u32 = 0u;
    var p6: u32 = 0u;
    var p7: u32 = 0u;
    var p8: u32 = 0u;
    var p9: u32 = 0u;
    var p10: u32 = 0u;
    var p11: u32 = 0u;
    var p12: u32 = 0u;
    var p13: u32 = 0u;
    var p14: u32 = 0u;
    var p15: u32 = 0u;
    var t: u64;
    var cy: u32;

    cy = 0u;
    t = m(f[0], g[0]) + u64(p0) + u64(cy); p0 = u32(t); cy = u32(t >> 32u);
    t = m(f[0], g[1]) + u64(p1) + u64(cy); p1 = u32(t); cy = u32(t >> 32u);
    t = m(f[0], g[2]) + u64(p2) + u64(cy); p2 = u32(t); cy = u32(t >> 32u);
    t = m(f[0], g[3]) + u64(p3) + u64(cy); p3 = u32(t); cy = u32(t >> 32u);
    t = m(f[0], g[4]) + u64(p4) + u64(cy); p4 = u32(t); cy = u32(t >> 32u);
    t = m(f[0], g[5]) + u64(p5) + u64(cy); p5 = u32(t); cy = u32(t >> 32u);
    t = m(f[0], g[6]) + u64(p6) + u64(cy); p6 = u32(t); cy = u32(t >> 32u);
    t = m(f[0], g[7]) + u64(p7) + u64(cy); p7 = u32(t); cy = u32(t >> 32u);
    p8 = cy;

    cy = 0u;
    t = m(f[1], g[0]) + u64(p1) + u64(cy); p1 = u32(t); cy = u32(t >> 32u);
    t = m(f[1], g[1]) + u64(p2) + u64(cy); p2 = u32(t); cy = u32(t >> 32u);
    t = m(f[1], g[2]) + u64(p3) + u64(cy); p3 = u32(t); cy = u32(t >> 32u);
    t = m(f[1], g[3]) + u64(p4) + u64(cy); p4 = u32(t); cy = u32(t >> 32u);
    t = m(f[1], g[4]) + u64(p5) + u64(cy); p5 = u32(t); cy = u32(t >> 32u);
    t = m(f[1], g[5]) + u64(p6) + u64(cy); p6 = u32(t); cy = u32(t >> 32u);
    t = m(f[1], g[6]) + u64(p7) + u64(cy); p7 = u32(t); cy = u32(t >> 32u);
    t = m(f[1], g[7]) + u64(p8) + u64(cy); p8 = u32(t); cy = u32(t >> 32u);
    p9 = cy;

    cy = 0u;
    t = m(f[2], g[0]) + u64(p2) + u64(cy); p2 = u32(t); cy = u32(t >> 32u);
    t = m(f[2], g[1]) + u64(p3) + u64(cy); p3 = u32(t); cy = u32(t >> 32u);
    t = m(f[2], g[2]) + u64(p4) + u64(cy); p4 = u32(t); cy = u32(t >> 32u);
    t = m(f[2], g[3]) + u64(p5) + u64(cy); p5 = u32(t); cy = u32(t >> 32u);
    t = m(f[2], g[4]) + u64(p6) + u64(cy); p6 = u32(t); cy = u32(t >> 32u);
    t = m(f[2], g[5]) + u64(p7) + u64(cy); p7 = u32(t); cy = u32(t >> 32u);
    t = m(f[2], g[6]) + u64(p8) + u64(cy); p8 = u32(t); cy = u32(t >> 32u);
    t = m(f[2], g[7]) + u64(p9) + u64(cy); p9 = u32(t); cy = u32(t >> 32u);
    p10 = cy;

    cy = 0u;
    t = m(f[3], g[0]) + u64(p3) + u64(cy); p3 = u32(t); cy = u32(t >> 32u);
    t = m(f[3], g[1]) + u64(p4) + u64(cy); p4 = u32(t); cy = u32(t >> 32u);
    t = m(f[3], g[2]) + u64(p5) + u64(cy); p5 = u32(t); cy = u32(t >> 32u);
    t = m(f[3], g[3]) + u64(p6) + u64(cy); p6 = u32(t); cy = u32(t >> 32u);
    t = m(f[3], g[4]) + u64(p7) + u64(cy); p7 = u32(t); cy = u32(t >> 32u);
    t = m(f[3], g[5]) + u64(p8) + u64(cy); p8 = u32(t); cy = u32(t >> 32u);
    t = m(f[3], g[6]) + u64(p9) + u64(cy); p9 = u32(t); cy = u32(t >> 32u);
    t = m(f[3], g[7]) + u64(p10) + u64(cy); p10 = u32(t); cy = u32(t >> 32u);
    p11 = cy;

    cy = 0u;
    t = m(f[4], g[0]) + u64(p4) + u64(cy); p4 = u32(t); cy = u32(t >> 32u);
    t = m(f[4], g[1]) + u64(p5) + u64(cy); p5 = u32(t); cy = u32(t >> 32u);
    t = m(f[4], g[2]) + u64(p6) + u64(cy); p6 = u32(t); cy = u32(t >> 32u);
    t = m(f[4], g[3]) + u64(p7) + u64(cy); p7 = u32(t); cy = u32(t >> 32u);
    t = m(f[4], g[4]) + u64(p8) + u64(cy); p8 = u32(t); cy = u32(t >> 32u);
    t = m(f[4], g[5]) + u64(p9) + u64(cy); p9 = u32(t); cy = u32(t >> 32u);
    t = m(f[4], g[6]) + u64(p10) + u64(cy); p10 = u32(t); cy = u32(t >> 32u);
    t = m(f[4], g[7]) + u64(p11) + u64(cy); p11 = u32(t); cy = u32(t >> 32u);
    p12 = cy;

    cy = 0u;
    t = m(f[5], g[0]) + u64(p5) + u64(cy); p5 = u32(t); cy = u32(t >> 32u);
    t = m(f[5], g[1]) + u64(p6) + u64(cy); p6 = u32(t); cy = u32(t >> 32u);
    t = m(f[5], g[2]) + u64(p7) + u64(cy); p7 = u32(t); cy = u32(t >> 32u);
    t = m(f[5], g[3]) + u64(p8) + u64(cy); p8 = u32(t); cy = u32(t >> 32u);
    t = m(f[5], g[4]) + u64(p9) + u64(cy); p9 = u32(t); cy = u32(t >> 32u);
    t = m(f[5], g[5]) + u64(p10) + u64(cy); p10 = u32(t); cy = u32(t >> 32u);
    t = m(f[5], g[6]) + u64(p11) + u64(cy); p11 = u32(t); cy = u32(t >> 32u);
    t = m(f[5], g[7]) + u64(p12) + u64(cy); p12 = u32(t); cy = u32(t >> 32u);
    p13 = cy;

    cy = 0u;
    t = m(f[6], g[0]) + u64(p6) + u64(cy); p6 = u32(t); cy = u32(t >> 32u);
    t = m(f[6], g[1]) + u64(p7) + u64(cy); p7 = u32(t); cy = u32(t >> 32u);
    t = m(f[6], g[2]) + u64(p8) + u64(cy); p8 = u32(t); cy = u32(t >> 32u);
    t = m(f[6], g[3]) + u64(p9) + u64(cy); p9 = u32(t); cy = u32(t >> 32u);
    t = m(f[6], g[4]) + u64(p10) + u64(cy); p10 = u32(t); cy = u32(t >> 32u);
    t = m(f[6], g[5]) + u64(p11) + u64(cy); p11 = u32(t); cy = u32(t >> 32u);
    t = m(f[6], g[6]) + u64(p12) + u64(cy); p12 = u32(t); cy = u32(t >> 32u);
    t = m(f[6], g[7]) + u64(p13) + u64(cy); p13 = u32(t); cy = u32(t >> 32u);
    p14 = cy;

    cy = 0u;
    t = m(f[7], g[0]) + u64(p7) + u64(cy); p7 = u32(t); cy = u32(t >> 32u);
    t = m(f[7], g[1]) + u64(p8) + u64(cy); p8 = u32(t); cy = u32(t >> 32u);
    t = m(f[7], g[2]) + u64(p9) + u64(cy); p9 = u32(t); cy = u32(t >> 32u);
    t = m(f[7], g[3]) + u64(p10) + u64(cy); p10 = u32(t); cy = u32(t >> 32u);
    t = m(f[7], g[4]) + u64(p11) + u64(cy); p11 = u32(t); cy = u32(t >> 32u);
    t = m(f[7], g[5]) + u64(p12) + u64(cy); p12 = u32(t); cy = u32(t >> 32u);
    t = m(f[7], g[6]) + u64(p13) + u64(cy); p13 = u32(t); cy = u32(t >> 32u);
    t = m(f[7], g[7]) + u64(p14) + u64(cy); p14 = u32(t); cy = u32(t >> 32u);
    p15 = cy;

    // 2^256 = 38 (mod p): the top half folds back into the bottom.
    var c: u64 = 0lu;
    t = u64(p0) + 38lu * u64(p8) + c; p0 = u32(t); c = t >> 32u;
    t = u64(p1) + 38lu * u64(p9) + c; p1 = u32(t); c = t >> 32u;
    t = u64(p2) + 38lu * u64(p10) + c; p2 = u32(t); c = t >> 32u;
    t = u64(p3) + 38lu * u64(p11) + c; p3 = u32(t); c = t >> 32u;
    t = u64(p4) + 38lu * u64(p12) + c; p4 = u32(t); c = t >> 32u;
    t = u64(p5) + 38lu * u64(p13) + c; p5 = u32(t); c = t >> 32u;
    t = u64(p6) + 38lu * u64(p14) + c; p6 = u32(t); c = t >> 32u;
    t = u64(p7) + 38lu * u64(p15) + c; p7 = u32(t); c = t >> 32u;
    // Two passes: the first folds the carry out of the product, the second the
    // carry the first can leave, which cannot carry again.
    var v: u32 = u32(c) * 38u;
    for (var round = 0u; round < 2u; round = round + 1u) {
        t = u64(p0) + u64(v); p0 = u32(t); v = u32(t >> 32u);
        t = u64(p1) + u64(v); p1 = u32(t); v = u32(t >> 32u);
        t = u64(p2) + u64(v); p2 = u32(t); v = u32(t >> 32u);
        t = u64(p3) + u64(v); p3 = u32(t); v = u32(t >> 32u);
        t = u64(p4) + u64(v); p4 = u32(t); v = u32(t >> 32u);
        t = u64(p5) + u64(v); p5 = u32(t); v = u32(t >> 32u);
        t = u64(p6) + u64(v); p6 = u32(t); v = u32(t >> 32u);
        t = u64(p7) + u64(v); p7 = u32(t); v = u32(t >> 32u);
        v = v * 38u;
    }
    return Fe(p0, p1, p2, p3, p4, p5, p6, p7);
}

fn fe_sq(f: Fe) -> Fe {
    return fe_mul(f, f);
}
