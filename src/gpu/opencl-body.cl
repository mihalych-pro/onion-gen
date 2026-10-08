// ---- the rest of the field: addition, subtraction, inversion, packing ----

// `a + b`, folding a carry out of the top back in with 2^256 = 38.
inline Fe fe_add(Fe f, Fe g) {
    Fe r; uint c = 0u;
    #pragma unroll
    for (int k = 0; k < 8; k++) {
        ulong t = (ulong)f.v[k] + (ulong)g.v[k] + (ulong)c;
        r.v[k] = (uint)t; c = (uint)(t >> 32);
    }
    uint v = c * 38u;
    #pragma unroll
    for (int round = 0; round < 2; round++) {
        #pragma unroll
        for (int k = 0; k < 8; k++) {
            ulong t = (ulong)r.v[k] + (ulong)v; r.v[k] = (uint)t; v = (uint)(t >> 32);
        }
        v *= 38u;
    }
    return r;
}

// `a - b`. A borrow out of the top means the result ran below zero, and
// 2^256 - 38 represents zero, so taking 38 away corrects it.
inline Fe fe_sub(Fe f, Fe g) {
    Fe r; uint b = 0u;
    #pragma unroll
    for (int k = 0; k < 8; k++) {
        ulong t = (ulong)f.v[k] - (ulong)g.v[k] - (ulong)b;
        r.v[k] = (uint)t; b = (uint)((t >> 32) & 1UL);
    }
    uint v = b * 38u;
    #pragma unroll
    for (int round = 0; round < 2; round++) {
        #pragma unroll
        for (int k = 0; k < 8; k++) {
            ulong t = (ulong)r.v[k] - (ulong)v; r.v[k] = (uint)t; v = (uint)((t >> 32) & 1UL);
        }
        v *= 38u;
    }
    return r;
}

// Saturated limbs are always normalised, so there is nothing to carry.
inline Fe fe_reduce(Fe f) { return f; }

inline Fe fe_one(void) {
    Fe r; r.v[0] = 1u;
    #pragma unroll
    for (int k = 1; k < 8; k++) r.v[k] = 0u;
    return r;
}

// f^(p-2), which is f^-1 for a non-zero f.
inline Fe fe_invert(Fe f) {
    Fe z1 = f;
    Fe z2 = fe_sq(z1);
    Fe z8 = fe_sq(fe_sq(z2));
    Fe z9 = fe_mul(z1, z8);
    Fe z11 = fe_mul(z2, z9);
    Fe z22 = fe_sq(z11);
    Fe z5 = fe_mul(z9, z22);

    Fe t = fe_sq(z5);
    for (int i = 1; i < 5; i++) t = fe_sq(t);
    Fe z10 = fe_mul(t, z5);

    t = fe_sq(z10);
    for (int i = 1; i < 10; i++) t = fe_sq(t);
    Fe z20 = fe_mul(t, z10);

    t = fe_sq(z20);
    for (int i = 1; i < 20; i++) t = fe_sq(t);
    Fe z40 = fe_mul(t, z20);

    t = fe_sq(z40);
    for (int i = 1; i < 10; i++) t = fe_sq(t);
    Fe z50 = fe_mul(t, z10);

    t = fe_sq(z50);
    for (int i = 1; i < 50; i++) t = fe_sq(t);
    Fe z100 = fe_mul(t, z50);

    t = fe_sq(z100);
    for (int i = 1; i < 100; i++) t = fe_sq(t);
    Fe z200 = fe_mul(t, z100);

    t = fe_sq(z200);
    for (int i = 1; i < 50; i++) t = fe_sq(t);
    Fe z250 = fe_mul(t, z50);

    t = fe_sq(z250);
    for (int i = 1; i < 5; i++) t = fe_sq(t);
    return fe_mul(t, z11);
}

// The canonical 32-byte little-endian encoding, as eight words. A value below
// 2^256 is below 2p + 38, so two conditional subtractions always suffice; a
// third costs eight words and removes the need to argue.
inline void fe_pack(Fe f, uint *out) {
    uint r[8];
    #pragma unroll
    for (int k = 0; k < 8; k++) r[k] = f.v[k];
    const uint P[8] = {0xffffffedu, 0xffffffffu, 0xffffffffu, 0xffffffffu,
                       0xffffffffu, 0xffffffffu, 0xffffffffu, 0x7fffffffu};
    for (int round = 0; round < 3; round++) {
        uint s[8]; uint b = 0u;
        #pragma unroll
        for (int k = 0; k < 8; k++) {
            ulong t = (ulong)r[k] - (ulong)P[k] - (ulong)b;
            s[k] = (uint)t; b = (uint)((t >> 32) & 1UL);
        }
        if (b == 0u) {
            #pragma unroll
            for (int k = 0; k < 8; k++) r[k] = s[k];
        }
    }
    #pragma unroll
    for (int k = 0; k < 8; k++) out[k] = r[k];
}
