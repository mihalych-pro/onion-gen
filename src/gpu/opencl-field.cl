// Field arithmetic for the OpenCL path: radix 2^32 in eight 32-bit limbs,
// least significant first, held below 2^256 and folded with 2^256 = 38.
//
// Eight saturated limbs need sixty-four partial products where ten redundant
// ones at radix 2^25.5 need a hundred, and an element is 32 bytes instead of
// 40 — the same arithmetic the processor path uses, and for the same reason.

typedef struct {
    uint v[8];
} Fe;

inline Fe fe_mul(Fe f, Fe g) {
    uint p[16];
    #pragma unroll
    for (int i = 0; i < 16; i++) p[i] = 0u;
    #pragma unroll
    for (int i = 0; i < 8; i++) {
        uint cy = 0u;
        #pragma unroll
        for (int j = 0; j < 8; j++) {
            ulong t = (ulong)f.v[i] * (ulong)g.v[j] + (ulong)p[i + j] + (ulong)cy;
            p[i + j] = (uint)t;
            cy = (uint)(t >> 32);
        }
        p[i + 8] = cy;
    }
    // 2^256 = 38 (mod p): the top half folds back into the bottom.
    Fe r;
    ulong c = 0;
    #pragma unroll
    for (int k = 0; k < 8; k++) {
        ulong t = (ulong)p[k] + 38UL * (ulong)p[k + 8] + c;
        r.v[k] = (uint)t;
        c = t >> 32;
    }
    uint v = (uint)c * 38u;
    #pragma unroll
    for (int round = 0; round < 2; round++) {
        #pragma unroll
        for (int k = 0; k < 8; k++) {
            ulong t = (ulong)r.v[k] + (ulong)v;
            r.v[k] = (uint)t;
            v = (uint)(t >> 32);
        }
        v *= 38u;
    }
    return r;
}

inline Fe fe_sq(Fe f) { return fe_mul(f, f); }
