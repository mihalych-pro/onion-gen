// ---- the rest of the field: addition, subtraction, inversion, packing ----

// `a + b`, folding a carry out of the top back in with 2^256 = 38. The second
// pass cannot carry again: a wrap leaves the low limbs near zero.
fn fe_add(f: Fe, g: Fe) -> Fe {
    var r0: u32;
    var r1: u32;
    var r2: u32;
    var r3: u32;
    var r4: u32;
    var r5: u32;
    var r6: u32;
    var r7: u32;
    var t: u64;
    var c: u32 = 0u;
    t = u64(f[0]) + u64(g[0]) + u64(c); r0 = u32(t); c = u32(t >> 32u);
    t = u64(f[1]) + u64(g[1]) + u64(c); r1 = u32(t); c = u32(t >> 32u);
    t = u64(f[2]) + u64(g[2]) + u64(c); r2 = u32(t); c = u32(t >> 32u);
    t = u64(f[3]) + u64(g[3]) + u64(c); r3 = u32(t); c = u32(t >> 32u);
    t = u64(f[4]) + u64(g[4]) + u64(c); r4 = u32(t); c = u32(t >> 32u);
    t = u64(f[5]) + u64(g[5]) + u64(c); r5 = u32(t); c = u32(t >> 32u);
    t = u64(f[6]) + u64(g[6]) + u64(c); r6 = u32(t); c = u32(t >> 32u);
    t = u64(f[7]) + u64(g[7]) + u64(c); r7 = u32(t); c = u32(t >> 32u);
    var v: u32 = c * 38u;
    for (var round = 0u; round < 2u; round = round + 1u) {
        t = u64(r0) + u64(v); r0 = u32(t); v = u32(t >> 32u);
        t = u64(r1) + u64(v); r1 = u32(t); v = u32(t >> 32u);
        t = u64(r2) + u64(v); r2 = u32(t); v = u32(t >> 32u);
        t = u64(r3) + u64(v); r3 = u32(t); v = u32(t >> 32u);
        t = u64(r4) + u64(v); r4 = u32(t); v = u32(t >> 32u);
        t = u64(r5) + u64(v); r5 = u32(t); v = u32(t >> 32u);
        t = u64(r6) + u64(v); r6 = u32(t); v = u32(t >> 32u);
        t = u64(r7) + u64(v); r7 = u32(t); v = u32(t >> 32u);
        v = v * 38u;
    }
    return Fe(r0, r1, r2, r3, r4, r5, r6, r7);
}

// `a - b`. A borrow out of the top means the result ran below zero, and
// 2^256 - 38 represents zero, so taking 38 away corrects it.
fn fe_sub(f: Fe, g: Fe) -> Fe {
    var r0: u32;
    var r1: u32;
    var r2: u32;
    var r3: u32;
    var r4: u32;
    var r5: u32;
    var r6: u32;
    var r7: u32;
    var t: u64;
    var b: u32 = 0u;
    t = u64(f[0]) - u64(g[0]) - u64(b); r0 = u32(t); b = u32((t >> 32u) & 1lu);
    t = u64(f[1]) - u64(g[1]) - u64(b); r1 = u32(t); b = u32((t >> 32u) & 1lu);
    t = u64(f[2]) - u64(g[2]) - u64(b); r2 = u32(t); b = u32((t >> 32u) & 1lu);
    t = u64(f[3]) - u64(g[3]) - u64(b); r3 = u32(t); b = u32((t >> 32u) & 1lu);
    t = u64(f[4]) - u64(g[4]) - u64(b); r4 = u32(t); b = u32((t >> 32u) & 1lu);
    t = u64(f[5]) - u64(g[5]) - u64(b); r5 = u32(t); b = u32((t >> 32u) & 1lu);
    t = u64(f[6]) - u64(g[6]) - u64(b); r6 = u32(t); b = u32((t >> 32u) & 1lu);
    t = u64(f[7]) - u64(g[7]) - u64(b); r7 = u32(t); b = u32((t >> 32u) & 1lu);
    var v: u32 = b * 38u;
    for (var round = 0u; round < 2u; round = round + 1u) {
        t = u64(r0) - u64(v); r0 = u32(t); v = u32((t >> 32u) & 1lu);
        t = u64(r1) - u64(v); r1 = u32(t); v = u32((t >> 32u) & 1lu);
        t = u64(r2) - u64(v); r2 = u32(t); v = u32((t >> 32u) & 1lu);
        t = u64(r3) - u64(v); r3 = u32(t); v = u32((t >> 32u) & 1lu);
        t = u64(r4) - u64(v); r4 = u32(t); v = u32((t >> 32u) & 1lu);
        t = u64(r5) - u64(v); r5 = u32(t); v = u32((t >> 32u) & 1lu);
        t = u64(r6) - u64(v); r6 = u32(t); v = u32((t >> 32u) & 1lu);
        t = u64(r7) - u64(v); r7 = u32(t); v = u32((t >> 32u) & 1lu);
        v = v * 38u;
    }
    return Fe(r0, r1, r2, r3, r4, r5, r6, r7);
}

// Saturated limbs are always normalised, so there is nothing to carry.
fn fe_reduce(f: Fe) -> Fe {
    return f;
}

fn fe_one() -> Fe {
    return Fe(1u, 0u, 0u, 0u, 0u, 0u, 0u, 0u);
}

// f^(p-2), which is f^-1 for a non-zero f. Eleven multiplications and 254
// squarings, one per round of candidates rather than one per candidate.
fn fe_invert(f: Fe) -> Fe {
    let z1 = f;
    let z2 = fe_sq(z1);
    let z8 = fe_sq(fe_sq(z2));
    let z9 = fe_mul(z1, z8);
    let z11 = fe_mul(z2, z9);
    let z22 = fe_sq(z11);
    let z5 = fe_mul(z9, z22);

    var t = fe_sq(z5);
    for (var i = 1u; i < 5u; i = i + 1u) { t = fe_sq(t); }
    let z10 = fe_mul(t, z5);

    t = fe_sq(z10);
    for (var i = 1u; i < 10u; i = i + 1u) { t = fe_sq(t); }
    let z20 = fe_mul(t, z10);

    t = fe_sq(z20);
    for (var i = 1u; i < 20u; i = i + 1u) { t = fe_sq(t); }
    let z40 = fe_mul(t, z20);

    t = fe_sq(z40);
    for (var i = 1u; i < 10u; i = i + 1u) { t = fe_sq(t); }
    let z50 = fe_mul(t, z10);

    t = fe_sq(z50);
    for (var i = 1u; i < 50u; i = i + 1u) { t = fe_sq(t); }
    let z100 = fe_mul(t, z50);

    t = fe_sq(z100);
    for (var i = 1u; i < 100u; i = i + 1u) { t = fe_sq(t); }
    let z200 = fe_mul(t, z100);

    t = fe_sq(z200);
    for (var i = 1u; i < 50u; i = i + 1u) { t = fe_sq(t); }
    let z250 = fe_mul(t, z50);

    t = fe_sq(z250);
    for (var i = 1u; i < 5u; i = i + 1u) { t = fe_sq(t); }
    return fe_mul(t, z11);
}

// The canonical 32-byte little-endian encoding, as eight words. A value below
// 2^256 is below 2p + 38, so two conditional subtractions of the modulus are
// always enough; a third costs four words and removes the need to argue.
fn fe_pack(f: Fe) -> array<u32, 8> {
    var r0: u32 = f[0];
    var r1: u32 = f[1];
    var r2: u32 = f[2];
    var r3: u32 = f[3];
    var r4: u32 = f[4];
    var r5: u32 = f[5];
    var r6: u32 = f[6];
    var r7: u32 = f[7];
    var t: u64;
    var b: u32;
    var s0: u32;
    var s1: u32;
    var s2: u32;
    var s3: u32;
    var s4: u32;
    var s5: u32;
    var s6: u32;
    var s7: u32;
    for (var round = 0u; round < 3u; round = round + 1u) {
        b = 0u;
        t = u64(r0) - u64(0xffffffedu) - u64(b); s0 = u32(t); b = u32((t >> 32u) & 1lu);
        t = u64(r1) - u64(0xffffffffu) - u64(b); s1 = u32(t); b = u32((t >> 32u) & 1lu);
        t = u64(r2) - u64(0xffffffffu) - u64(b); s2 = u32(t); b = u32((t >> 32u) & 1lu);
        t = u64(r3) - u64(0xffffffffu) - u64(b); s3 = u32(t); b = u32((t >> 32u) & 1lu);
        t = u64(r4) - u64(0xffffffffu) - u64(b); s4 = u32(t); b = u32((t >> 32u) & 1lu);
        t = u64(r5) - u64(0xffffffffu) - u64(b); s5 = u32(t); b = u32((t >> 32u) & 1lu);
        t = u64(r6) - u64(0xffffffffu) - u64(b); s6 = u32(t); b = u32((t >> 32u) & 1lu);
        t = u64(r7) - u64(0x7fffffffu) - u64(b); s7 = u32(t); b = u32((t >> 32u) & 1lu);
        if (b == 0u) {
            r0 = s0;
            r1 = s1;
            r2 = s2;
            r3 = s3;
            r4 = s4;
            r5 = s5;
            r6 = s6;
            r7 = s7;
        }
    }
    return array<u32, 8>(r0, r1, r2, r3, r4, r5, r6, r7);
}
