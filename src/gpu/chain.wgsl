
// Candidates in pairs, with a batch inversion. From one base point and a table
// of affine offsets Q(m) = 8m*G, three multiplies give two candidates:
//
//   y(P+Q) = (T - x2y2*Z) / (X*y2 - Y*x2)
//   y(P-Q) = (T + x2y2*Z) / (X*y2 + Y*x2)
//
// In extended coordinates Z cancels out of both, so the base point never has to
// be made affine. The base moves once per launch instead of once per candidate,
// which is where the chain spent most of its arithmetic.

struct Params {
    limit: u32,
    bitmap_bits: u32,
    half: u32,
    n: u32,
}

@group(0) @binding(0) var<storage, read_write> hits: array<u32>;
@group(0) @binding(1) var<storage, read_write> counter: array<atomic<u32>>;
@group(0) @binding(2) var<storage, read_write> state: array<u32>;
@group(0) @binding(3) var<storage, read> step_q: array<u32>;
@group(0) @binding(4) var<storage, read_write> denbuf: array<u32>;
@group(0) @binding(5) var<storage, read_write> numbuf: array<u32>;
@group(0) @binding(6) var<storage, read> bitmap: array<u32>;
@group(0) @binding(7) var<uniform> params: Params;
// Q(m) = 8(m+1)*G, affine, as x, y and the product x*y. Shared by every
// invocation and read-only, so the cache answers the whole workgroup at once.
@group(0) @binding(8) var<storage, read> table: array<u32>;

struct Point {
    x: Fe,
    y: Fe,
    z: Fe,
    t: Fe,
}

// The step is the same for every invocation, so it stays in memory and is read
// when needed: neighbouring invocations ask for the same address, which the
// cache answers once.
// The constant step is held with its Z scaled to one, which is what makes the
// addition below cost seven multiplies instead of eight: the product Z1 * Z2
// becomes Z1. Scaling the cached point costs one inversion on the host, once
// per run, and leaves the same point projectively.
fn cached(which: u32) -> Fe {
    let b = which * 8u;
    return Fe(step_q[b], step_q[b + 1u], step_q[b + 2u], step_q[b + 3u],
              step_q[b + 4u], step_q[b + 5u], step_q[b + 6u], step_q[b + 7u]);
}

// One field element out of a stride-n plane: invocation tid reads word tid of
// each limb, so neighbouring invocations touch neighbouring words. WGSL will
// not take a storage pointer as a parameter, so there is one of these per
// buffer rather than one shared.

fn load_state(base: u32, n: u32, tid: u32) -> Fe {
    return Fe(state[base + 0u * n + tid], state[base + 1u * n + tid], state[base + 2u * n + tid],
              state[base + 3u * n + tid], state[base + 4u * n + tid], state[base + 5u * n + tid],
              state[base + 6u * n + tid], state[base + 7u * n + tid]);
}

fn store_state(base: u32, n: u32, tid: u32, v: Fe) {
    state[base + 0u * n + tid] = v[0]; state[base + 1u * n + tid] = v[1];
    state[base + 2u * n + tid] = v[2]; state[base + 3u * n + tid] = v[3];
    state[base + 4u * n + tid] = v[4]; state[base + 5u * n + tid] = v[5];
    state[base + 6u * n + tid] = v[6]; state[base + 7u * n + tid] = v[7];
}

fn load_den(base: u32, n: u32, tid: u32) -> Fe {
    return Fe(denbuf[base + 0u * n + tid], denbuf[base + 1u * n + tid], denbuf[base + 2u * n + tid],
              denbuf[base + 3u * n + tid], denbuf[base + 4u * n + tid], denbuf[base + 5u * n + tid],
              denbuf[base + 6u * n + tid], denbuf[base + 7u * n + tid]);
}

fn store_den(base: u32, n: u32, tid: u32, v: Fe) {
    denbuf[base + 0u * n + tid] = v[0]; denbuf[base + 1u * n + tid] = v[1];
    denbuf[base + 2u * n + tid] = v[2]; denbuf[base + 3u * n + tid] = v[3];
    denbuf[base + 4u * n + tid] = v[4]; denbuf[base + 5u * n + tid] = v[5];
    denbuf[base + 6u * n + tid] = v[6]; denbuf[base + 7u * n + tid] = v[7];
}

fn load_num(base: u32, n: u32, tid: u32) -> Fe {
    return Fe(numbuf[base + 0u * n + tid], numbuf[base + 1u * n + tid], numbuf[base + 2u * n + tid],
              numbuf[base + 3u * n + tid], numbuf[base + 4u * n + tid], numbuf[base + 5u * n + tid],
              numbuf[base + 6u * n + tid], numbuf[base + 7u * n + tid]);
}

fn store_num(base: u32, n: u32, tid: u32, v: Fe) {
    numbuf[base + 0u * n + tid] = v[0]; numbuf[base + 1u * n + tid] = v[1];
    numbuf[base + 2u * n + tid] = v[2]; numbuf[base + 3u * n + tid] = v[3];
    numbuf[base + 4u * n + tid] = v[4]; numbuf[base + 5u * n + tid] = v[5];
    numbuf[base + 6u * n + tid] = v[6]; numbuf[base + 7u * n + tid] = v[7];
}

// Entry `m` of the table, coordinate `which` of x, y, xy.
fn table_fe(m: u32, which: u32) -> Fe {
    let b = (m * 3u + which) * 8u;
    return Fe(table[b], table[b + 1u], table[b + 2u], table[b + 3u],
              table[b + 4u], table[b + 5u], table[b + 6u], table[b + 7u]);
}

fn step(p: Point) -> Point {
    let a = fe_add(p.y, p.x);
    let b = fe_sub(p.y, p.x);
    let zz = fe_mul(a, cached(0u));
    let yy = fe_mul(b, cached(1u));
    let tt = fe_mul(cached(2u), p.t);
    // With the step's Z at one this is 2 * Z1 and not 2 * Z1 * Z2, which is the
    // multiply that goes away. The carry stays: doubling leaves the value
    // loose, and both of the values built from it below would otherwise exceed
    // what the scaling by 19 inside the multiply can hold.
    let t0 = fe_reduce(fe_add(p.z, p.z));

    let x = fe_sub(zz, yy);
    let y = fe_add(zz, yy);
    let z = fe_add(t0, tt);
    let t = fe_sub(t0, tt);

    return Point(fe_mul(x, t), fe_mul(y, z), fe_mul(z, t), fe_mul(x, y));
}

@compute @workgroup_size(64)
fn chain(@builtin(global_invocation_id) gid: vec3<u32>) {
    let tid = gid.x;
    let n = params.n;
    if (tid >= n) { return; }
    let half = params.half;
    let slots = 2u * half + 1u;

    var p: Point;
    p.x = load_state(0u * 8u * n, n, tid);
    p.y = load_state(1u * 8u * n, n, tid);
    p.z = load_state(2u * 8u * n, n, tid);
    p.t = load_state(3u * 8u * n, n, tid);

    // Two candidates per table entry: the sum and the difference share both
    // products, so the pair costs three multiplies rather than a point
    // addition each.
    //
    // The running product of the denominators is folded into the numerators as
    // they are produced, so Montgomery's trick needs no separate forward pass
    // and no third plane to remember it in. That is the whole of the memory
    // this kernel touches per candidate: one numerator and one denominator
    // written here, both read back below.
    var running = fe_one();
    for (var m = 0u; m < half; m = m + 1u) {
        let a = fe_mul(p.x, table_fe(m, 1u));
        let b = fe_mul(p.y, table_fe(m, 0u));
        let c = fe_mul(table_fe(m, 2u), p.z);

        let plus = 2u * m;
        let d0 = fe_sub(a, b);
        store_num(plus * 8u * n, n, tid, fe_mul(fe_sub(p.t, c), running));
        store_den(plus * 8u * n, n, tid, d0);
        running = fe_mul(running, d0);

        let minus = plus + 1u;
        let d1 = fe_add(a, b);
        store_num(minus * 8u * n, n, tid, fe_mul(fe_add(p.t, c), running));
        store_den(minus * 8u * n, n, tid, d1);
        running = fe_mul(running, d1);
    }
    // The base point itself is the last candidate, and it is already a
    // fraction: y = Y / Z.
    let centre = (slots - 1u) * 8u * n;
    store_num(centre, n, tid, fe_mul(p.y, running));
    store_den(centre, n, tid, p.z);
    running = fe_mul(running, p.z);

    // One addition for the whole launch, where the chain did one per candidate.
    p = step(p);
    store_state(0u * 8u * n, n, tid, p.x);
    store_state(1u * 8u * n, n, tid, p.y);
    store_state(2u * 8u * n, n, tid, p.z);
    store_state(3u * 8u * n, n, tid, p.t);

    var acc = fe_invert(running);

    for (var back = 0u; back < slots; back = back + 1u) {
        let r = slots - 1u - back;
        let base = r * 8u * n;
        let d = load_den(base, n, tid);
        let nu = load_num(base, n, tid);
        let packed = fe_pack(fe_mul(nu, acc));
        acc = fe_mul(acc, d);

        // The first four bytes, big-endian, are what the index is cut from.
        let w0 = packed[0];
        let head = ((w0 & 0xffu) << 24u) | (((w0 >> 8u) & 0xffu) << 16u)
                 | (((w0 >> 16u) & 0xffu) << 8u) | ((w0 >> 24u) & 0xffu);
        let slot = head >> (32u - params.bitmap_bits);
        if (((bitmap[slot >> 5u] >> (slot & 31u)) & 1u) == 1u) {
            let idx = atomicAdd(&counter[0], 1u);
            if (idx < params.limit) {
                let at = idx * 10u;
                hits[at] = tid;
                hits[at + 1u] = r;
                for (var k = 0u; k < 8u; k = k + 1u) { hits[at + 2u + k] = packed[k]; }
            }
        }
    }
}
