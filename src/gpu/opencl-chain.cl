
// The chain of +8G with a batch inversion, the same shape every other path
// runs, so a candidate can be compared with any of them.

typedef struct {
    Fe x, y, z, t;
} Point;

// The step is the same for every work item, so it stays in memory and is read
// when needed: neighbouring items ask for the same address, which the cache
// answers once. It is held with its Z scaled to one on the host, which is what
// makes the addition below cost seven multiplies instead of eight.
// Entry `m` of the offset table, coordinate `which` of x, y, xy.
//
// Which address space the table lives in is decided per device, because the
// answer is opposite on the two kinds. Measured on the same kernel: an Apple
// M1 Pro goes from 230 to 248 M/s when the work group holds its own copy,
// while an RTX 4060 drops from 1604 to 1204. The copy is 9 KiB of local
// memory per group, which on the discrete card costs more occupancy than it
// saves in reads — its cache was already answering them.
#if OG_SHARED_TABLE
inline Fe table_fe(__local const uint *table, uint m, uint which) {
#else
inline Fe table_fe(__global const uint *table, uint m, uint which) {
#endif
    uint b = (m * 3 + which) * 8;
    Fe v;
    #pragma unroll
    for (int k = 0; k < 8; k++) v.v[k] = table[b + k];
    return v;
}

inline Fe cached(__global const uint *step_q, uint which) {
    uint b = which * 8;
    Fe v;
    #pragma unroll
    for (int k = 0; k < 8; k++) v.v[k] = step_q[b + k];
    return v;
}

inline Point step_once(Point p, __global const uint *step_q) {
    Fe a = fe_add(p.y, p.x);
    Fe b = fe_sub(p.y, p.x);
    Fe zz = fe_mul(a, cached(step_q, 0));
    Fe yy = fe_mul(b, cached(step_q, 1));
    Fe tt = fe_mul(cached(step_q, 2), p.t);
    // With the step's Z at one this is 2 * Z1 and not 2 * Z1 * Z2. The carry
    // stays: doubling leaves the value loose, and both of the values built from
    // it below would otherwise exceed what the scaling by 19 inside the
    // multiply can hold.
    Fe t0 = fe_reduce(fe_add(p.z, p.z));

    Fe x = fe_sub(zz, yy);
    Fe y = fe_add(zz, yy);
    Fe z = fe_add(t0, tt);
    Fe t = fe_sub(t0, tt);

    Point o;
    o.x = fe_mul(x, t);
    o.y = fe_mul(y, z);
    o.z = fe_mul(z, t);
    o.t = fe_mul(x, y);
    return o;
}

// One field element out of a stride-n plane: item tid reads word tid of each
// limb, so neighbouring items touch neighbouring words.
inline Fe load_soa(__global const uint *p, uint base, uint n, uint tid) {
    Fe v;
    #pragma unroll
    for (int k = 0; k < 8; k++) v.v[k] = p[base + k * n + tid];
    return v;
}

inline void store_soa(__global uint *p, uint base, uint n, uint tid, Fe v) {
    #pragma unroll
    for (int k = 0; k < 8; k++) p[base + k * n + tid] = v.v[k];
}

__kernel void chain(
    __global uint *hits,
    __global uint *counter,
    const uint limit,
    __global uint *state,
    __global const uint *step_q,
    __global uint *denbuf,
    __global uint *numbuf,
    __global const uint *bitmap,
    const uint bitmap_bits,
    const uint half_,
    const uint n,
    __global const uint *table
) {
#if OG_SHARED_TABLE
    // HALF is fixed at compile time, so the size is known here. The copy
    // happens before any thread leaves: `barrier` has to be reached by all of
    // them, and a thread that returned early would never arrive.
    __local uint offsets[OG_HALF * 3 * 8];
    {
        uint lid = get_local_id(0);
        uint lsz = get_local_size(0);
        for (uint i = lid; i < OG_HALF * 3 * 8; i += lsz) {
            offsets[i] = table[i];
        }
    }
    barrier(CLK_LOCAL_MEM_FENCE);
#else
    __global const uint *offsets = table;
#endif

    uint tid = get_global_id(0);
    if (tid >= n) {
        return;
    }
    uint slots = 2 * half_ + 1;

    Point p;
    p.x = load_soa(state, 0 * 8 * n, n, tid);
    p.y = load_soa(state, 1 * 8 * n, n, tid);
    p.z = load_soa(state, 2 * 8 * n, n, tid);
    p.t = load_soa(state, 3 * 8 * n, n, tid);

    // Two candidates per table entry:
    //   y(P+Q) = (T - x2y2*Z) / (X*y2 - Y*x2)
    //   y(P-Q) = (T + x2y2*Z) / (X*y2 + Y*x2)
    // Three multiplies for the pair, and Z never has to be inverted away.
    // The running product of the denominators is folded into the numerators as
    // they are produced, so Montgomery's trick needs no forward pass and no
    // third plane. One numerator and one denominator per candidate is all the
    // memory this kernel touches.
    Fe running = fe_one();
    for (uint m = 0; m < half_; m++) {
        Fe a = fe_mul(p.x, table_fe(offsets, m, 1));
        Fe b = fe_mul(p.y, table_fe(offsets, m, 0));
        Fe c = fe_mul(table_fe(offsets, m, 2), p.z);

        uint plus = 2 * m;
        Fe d0 = fe_sub(a, b);
        store_soa(numbuf, plus * 8 * n, n, tid, fe_mul(fe_sub(p.t, c), running));
        store_soa(denbuf, plus * 8 * n, n, tid, d0);
        running = fe_mul(running, d0);

        uint minus = plus + 1;
        Fe d1 = fe_add(a, b);
        store_soa(numbuf, minus * 8 * n, n, tid, fe_mul(fe_add(p.t, c), running));
        store_soa(denbuf, minus * 8 * n, n, tid, d1);
        running = fe_mul(running, d1);
    }
    // The base point itself is the last candidate: y = Y / Z.
    uint centre = (slots - 1) * 8 * n;
    store_soa(numbuf, centre, n, tid, fe_mul(p.y, running));
    store_soa(denbuf, centre, n, tid, p.z);
    running = fe_mul(running, p.z);

    // One addition for the whole launch, where the chain did one per candidate.
    p = step_once(p, step_q);
    store_soa(state, 0 * 8 * n, n, tid, p.x);
    store_soa(state, 1 * 8 * n, n, tid, p.y);
    store_soa(state, 2 * 8 * n, n, tid, p.z);
    store_soa(state, 3 * 8 * n, n, tid, p.t);

    Fe acc = fe_invert(running);

    for (uint back = 0; back < slots; back++) {
        uint r = slots - 1 - back;
        uint base = r * 8 * n;
        Fe d = load_soa(denbuf, base, n, tid);
        Fe nu = load_soa(numbuf, base, n, tid);
        uint packed[8];
        fe_pack(fe_mul(nu, acc), packed);
        acc = fe_mul(acc, d);

        // The first four bytes, big-endian, are what the index is cut from.
        uint w0 = packed[0];
        uint head = ((w0 & 0xffu) << 24) | (((w0 >> 8) & 0xffu) << 16)
                  | (((w0 >> 16) & 0xffu) << 8) | ((w0 >> 24) & 0xffu);
        uint slot = head >> (32 - bitmap_bits);
        if (((bitmap[slot >> 5] >> (slot & 31)) & 1u) == 1u) {
            uint idx = atomic_add(counter, 1u);
            if (idx < limit) {
                uint at = idx * 10;
                hits[at] = tid;
                hits[at + 1] = r;
                for (uint k = 0; k < 8; k++) {
                    hits[at + 2 + k] = packed[k];
                }
            }
        }
    }
}
