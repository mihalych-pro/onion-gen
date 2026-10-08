#![no_std]
#![feature(abi_ptx, stdarch_nvptx, asm_experimental_arch)]

mod curve;
mod field;
mod field_asm;

use core::arch::nvptx;
use core::panic::PanicInfo;

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    loop {}
}

/// The smallest thing that proves the whole path: Rust to PTX, PTX to the
/// driver, the driver to the device, and the answer back.
#[no_mangle]
pub unsafe extern "ptx-kernel" fn add(out: *mut u64, a: *const u64, b: *const u64, n: u32) {
    let i = nvptx::_block_idx_x() as u32 * nvptx::_block_dim_x() as u32
        + nvptx::_thread_idx_x() as u32;
    if i < n {
        let i = i as usize;
        *out.add(i) = *a.add(i) + *b.add(i);
    }
}

/// One atomic increment, which is how a thread claims a slot in the output.
#[inline(always)]
unsafe fn claim(counter: *mut u32) -> u32 {
    let old: u32;
    core::arch::asm!(
        "atom.global.add.u32 {0}, [{1}], 1;",
        out(reg32) old,
        in(reg64) counter,
        options(nostack)
    );
    old
}

/// The chain, split in two so that each half gets its own register budget.
///
/// One kernel is allocated for the worst of its parts. The first half advances
/// the point and needs every register it can hold; the second reads three
/// planes out of memory per candidate and needs warps, not registers. Sharing
/// one allocation makes the second half run at the first half's occupancy,
/// which on an Ada part is 12 resident warps out of 48.
///
/// The halves talk through the same buffers they already used, so the split
/// costs one extra launch per batch and no extra traffic.
#[no_mangle]
pub unsafe extern "ptx-kernel" fn chain_steps(
    state: *mut u32,
    step_q: *const u32,
    denbuf: *mut u32,
    numbuf: *mut u32,
    half: u32,
    n: u32,
    table: *const u32,
    prod: *mut u32,
) {
    let tid = nvptx::_block_idx_x() as u32 * nvptx::_block_dim_x() as u32
        + nvptx::_thread_idx_x() as u32;
    if tid >= n {
        return;
    }
    let at = |r: u32, k: usize| -> usize {
        r as usize * 8 * n as usize + k * n as usize + tid as usize
    };
    let coord = |which: usize, k: usize| -> usize {
        which * 8 * n as usize + k * n as usize + tid as usize
    };
    let store_soa = |p: *mut u32, r: u32, v: &field::Fe| {
        let mut k = 0;
        while k < 8 {
            *p.add(at(r, k)) = v[k];
            k += 1;
        }
    };
    let load_coord = |which: usize| -> field::Fe {
        let mut v: field::Fe = [0; 8];
        let mut k = 0;
        while k < 8 {
            v[k] = *state.add(coord(which, k));
            k += 1;
        }
        v
    };
    let store_coord = |which: usize, v: &field::Fe| {
        let mut k = 0;
        while k < 8 {
            *state.add(coord(which, k)) = v[k];
            k += 1;
        }
    };

    let table_fe = |m: u32, which: usize| -> field::Fe {
        let b = (m as usize * 3 + which) * 8;
        let mut v: field::Fe = [0; 8];
        let mut k = 0;
        while k < 8 {
            v[k] = *table.add(b + k);
            k += 1;
        }
        v
    };

    let point = curve::Point {
        x: load_coord(0),
        y: load_coord(1),
        z: load_coord(2),
        t: load_coord(3),
    };

    // Two candidates per table entry:
    //   y(P+Q) = (T - x2y2*Z) / (X*y2 - Y*x2)
    //   y(P-Q) = (T + x2y2*Z) / (X*y2 + Y*x2)
    // Three multiplies for the pair, and Z never has to be inverted away.
    //
    // The running product of the denominators is folded into the numerators
    // here, so the second half needs no forward pass and no third plane: it
    // reads one numerator and one denominator per candidate and nothing else.
    let mut running: field::Fe = [1, 0, 0, 0, 0, 0, 0, 0];
    let mut m = 0;
    while m < half {
        let a = field_asm::mul(&point.x, &table_fe(m, 1));
        let b = field_asm::mul(&point.y, &table_fe(m, 0));
        let c = field_asm::mul(&table_fe(m, 2), &point.z);

        let plus = 2 * m;
        let d0 = field_asm::sub(&a, &b);
        store_soa(numbuf, plus, &field_asm::mul(&field_asm::sub(&point.t, &c), &running));
        store_soa(denbuf, plus, &d0);
        running = field_asm::mul(&running, &d0);

        let d1 = field_asm::add(&a, &b);
        store_soa(numbuf, plus + 1, &field_asm::mul(&field_asm::add(&point.t, &c), &running));
        store_soa(denbuf, plus + 1, &d1);
        running = field_asm::mul(&running, &d1);
        m += 1;
    }
    // The base point itself is the last candidate: y = Y / Z.
    let centre = 2 * half;
    store_soa(numbuf, centre, &field_asm::mul(&point.y, &running));
    store_soa(denbuf, centre, &point.z);
    running = field_asm::mul(&running, &point.z);

    // The product the second half inverts, one field element per thread.
    {
        let mut k = 0;
        while k < 8 {
            *prod.add(k * n as usize + tid as usize) = running[k];
            k += 1;
        }
    }

    // One addition for the whole launch, where the chain did one per candidate.
    let moved = curve::step(&point, step_q);
    store_coord(0, &moved.x);
    store_coord(1, &moved.y);
    store_coord(2, &moved.z);
    store_coord(3, &moved.t);
}

/// The second half: one inversion for every candidate the first half
/// produced, then the address of each against the bitmap.
#[no_mangle]
pub unsafe extern "ptx-kernel" fn chain_reduce(
    hits: *mut u32,
    counter: *mut u32,
    limit: u32,
    denbuf: *const u32,
    numbuf: *const u32,
    prod: *const u32,
    bitmap: *const u32,
    bitmap_bits: u32,
    slots: u32,
    n: u32,
) {
    let tid = nvptx::_block_idx_x() as u32 * nvptx::_block_dim_x() as u32
        + nvptx::_thread_idx_x() as u32;
    if tid >= n {
        return;
    }
    let at = |r: u32, k: usize| -> usize {
        r as usize * 8 * n as usize + k * n as usize + tid as usize
    };
    let load_soa = |p: *const u32, r: u32| -> field::Fe {
        let mut v: field::Fe = [0; 8];
        let mut k = 0;
        while k < 8 {
            v[k] = *p.add(at(r, k));
            k += 1;
        }
        v
    };
    let mut running: field::Fe = [0; 8];
    {
        let mut k = 0;
        while k < 8 {
            running[k] = *prod.add(k * n as usize + tid as usize);
            k += 1;
        }
    }
    let mut acc = field_asm::invert(&running);

    let mut r = slots;
    while r > 0 {
        r -= 1;
        let d = load_soa(denbuf, r);
        let y = field_asm::mul(&load_soa(numbuf, r), &acc);
        acc = field_asm::mul(&acc, &d);
        let packed = field_asm::pack(&y);

        let w0 = packed[0];
        let head = ((w0 & 0xff) << 24)
            | (((w0 >> 8) & 0xff) << 16)
            | (((w0 >> 16) & 0xff) << 8)
            | ((w0 >> 24) & 0xff);
        let slot = head >> (32 - bitmap_bits);
        let word = *bitmap.add((slot >> 5) as usize);
        if (word >> (slot & 31)) & 1 == 1 {
            let idx = claim(counter);
            if idx < limit {
                let base = idx as usize * 10;
                *hits.add(base) = tid;
                *hits.add(base + 1) = r;
                let mut k = 0;
                while k < 8 {
                    *hits.add(base + 2 + k) = packed[k];
                    k += 1;
                }
            }
        }
    }
}
