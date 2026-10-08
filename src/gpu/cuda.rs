//! The vendor path: NVIDIA through its own driver.
//!
//! The driver is opened at run time, not linked at build time: the same binary
//! runs where there is a card and where there is not, and nothing about
//! building it needs a vendor toolkit. What the device cannot do — deciding
//! *which* filter matched, writing keys — stays on the host.

use super::{Api, Availability, Device};

use crate::curve::{self, Point};
use crate::field::Fe;
use cudarc::driver::{CudaContext, CudaSlice, CudaStream, LaunchConfig, PushKernelArg};
use std::sync::Arc;

/// The kernel, compiled from `cuda-kernel/` by this crate's build script.
///
/// Built rather than committed: a kernel edited without its compiled form
/// being refreshed produces wrong addresses and no error at all.
const KERNEL: &str = include_str!(concat!(env!("OUT_DIR"), "/cuda-kernel.ptx"));

/// How many candidates each thread produces per launch.
///
/// The single inversion is amortised over these, so too few makes it dominate;
/// the scratch buffers are proportional to it, so too many will not fit.
/// Offsets in the table, and so pairs per thread per launch.
const HALF: u32 = 96;
/// Candidates per thread per launch: both members of every pair, plus the base
/// point itself.
const SLOTS: u32 = 2 * HALF + 1;

/// Looks for devices without requiring any of it to be present.
///
/// Every failure here is an ordinary outcome, not an error: a machine with no
/// card is the common case and must cost nothing but a message.
pub fn look() -> Availability {
    // This has to come first. The driver is opened lazily on the first call
    // into it, and if it is absent that call aborts the process rather than
    // returning an error — which is precisely what a machine without a card
    // must not do.
    if !unsafe { cudarc::driver::sys::is_culib_present() } {
        return Availability::NoDriver(missing_library());
    }
    match cudarc::driver::result::init() {
        Ok(()) => {}
        Err(e) => {
            return Availability::NoDriver(format!("{} refused to start ({e})", library_name()))
        }
    }
    let count = match cudarc::driver::result::device::get_count() {
        Ok(n) if n > 0 => n as usize,
        Ok(_) => return Availability::NoDevice,
        Err(e) => return Availability::NoDriver(format!("the driver reported no devices ({e})")),
    };

    let mut devices = Vec::with_capacity(count);
    for ordinal in 0..count {
        let Ok(ctx) = CudaContext::new(ordinal) else {
            continue;
        };
        devices.push(Device {
            // Filled in by the caller once both paths have been asked.
            index: 0,
            api: Api::Cuda,
            ordinal,
            name: ctx.name().unwrap_or_else(|_| "unnamed".to_string()),
            memory: ctx.total_mem().unwrap_or(0),
            // This vendor ships no integrated part this path can reach.
            discrete: true,
            also_via: None,
        });
    }
    if devices.is_empty() {
        Availability::NoDevice
    } else {
        Availability::Devices(devices)
    }
}

/// The library the driver lives in, so the message says what is missing rather
/// than that something went wrong.
fn library_name() -> &'static str {
    if cfg!(windows) {
        "nvcuda.dll"
    } else {
        "libcuda.so.1"
    }
}

fn missing_library() -> String {
    if cfg!(target_os = "macos") {
        // Not a missing install: the vendor has shipped no driver for this
        // platform since 2019, and saying "install it" would send the reader
        // after something that does not exist.
        return "this vendor has no driver for macOS".to_string();
    }
    format!(
        "{} was not found; the vendor driver is what provides it, not the toolkit",
        library_name()
    )
}

/// One device, its state, and the buffers it works in.
pub struct Engine {
    stream: Arc<CudaStream>,
    steps: cudarc::driver::CudaFunction,
    reduce: cudarc::driver::CudaFunction,
    state: CudaSlice<u32>,
    step_q: CudaSlice<u32>,
    denbuf: CudaSlice<u32>,
    numbuf: CudaSlice<u32>,
    table: CudaSlice<u32>,
    prod: CudaSlice<u32>,
    bitmap: CudaSlice<u32>,
    hits: CudaSlice<u32>,
    counter: CudaSlice<u32>,
    bitmap_bits: u32,
    threads: u32,
    block: u32,
    limit: u32,
    steps_done: u64,
    _ctx: Arc<CudaContext>,
}

/// One field element as the device holds it.
/// A field element as the kernel holds it: eight 32-bit limbs, least
/// significant first, which is the canonical encoding read as words.
fn limbs(fe: &Fe) -> [u32; 8] {
    let b = fe.to_bytes();
    std::array::from_fn(|k| u32::from_le_bytes(b[k * 4..k * 4 + 4].try_into().unwrap()))
}

/// The scalar `value`, little-endian, as the multiplication wants it.
fn scalar(value: u64) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[..8].copy_from_slice(&value.to_le_bytes());
    out
}

impl Engine {
    /// Prepares a device to continue the chain that starts at `start`.
    ///
    /// `bitmap` and `bitmap_bits` are the prefix index; without one there is
    /// nothing for the device to filter on and every candidate would have to
    /// cross back, which the link cannot carry.
    pub fn new(
        ordinal: usize,
        start: &Point,
        threads: u32,
        bitmap: &[u64],
        bitmap_bits: u32,
    ) -> Result<Self, cudarc::driver::DriverError> {
        let ctx = CudaContext::new(ordinal)?;
        let stream = ctx.default_stream();
        let module = ctx.load_module(cudarc::nvrtc::Ptx::from_src(KERNEL))?;
        // Two launches, not one. A kernel is allocated registers for the worst
        // of its parts: the half that advances the point wants every register
        // it can hold, and the half that inverts and matches reads three planes
        // per candidate and wants resident warps instead. Fused, the second
        // half runs at the first half's occupancy; split, each half gets its
        // own budget — 566 to 682 M candidates/s.
        let steps = module.load_function("chain_steps")?;
        let reduce = module.load_function("chain_reduce")?;

        let two_d = curve::two_d();
        let eight = curve::eight_basepoint_cached();

        // Thread t owns the window of SLOTS consecutive candidates starting at
        // t * SLOTS, with its base point at that window's centre; every launch
        // moves each of them on by the whole width, so no two threads ever
        // meet.
        let window = curve::to_cached(
            &curve::scalar_base_mult(&scalar(u64::from(SLOTS) * 8), &Point::basepoint(), &two_d),
            &two_d,
        );
        let to_centre = curve::to_cached(
            &curve::scalar_base_mult(
                &scalar(u64::from(HALF + 1) * 8),
                &Point::basepoint(),
                &two_d,
            ),
            &two_d,
        );
        let mut points = Vec::with_capacity(threads as usize);
        let mut p = curve::to_p3(&curve::add(start, &to_centre));
        for _ in 0..threads {
            points.push(p);
            p = curve::to_p3(&curve::add(&p, &window));
        }

        // Q(m) = 8(m+1)*G, affine and with the product of its coordinates.
        let mut table = Vec::with_capacity(HALF as usize * 24);
        let mut q = curve::scalar_base_mult(&scalar(8), &Point::basepoint(), &two_d);
        for m in 0..HALF {
            let zinv = q.z.invert();
            let x = q.x.mul(&zinv);
            let y = q.y.mul(&zinv);
            for fe in [x, y, x.mul(&y)] {
                table.extend_from_slice(&limbs(&fe));
            }
            if m + 1 < HALF {
                q = curve::to_p3(&curve::add(&q, &eight));
            }
        }
        // Coordinate-major, limb-major, thread-minor: the layout the kernel
        // reads, and the one where neighbouring threads touch neighbouring
        // words.
        let mut state = vec![0u32; threads as usize * 32];
        for (t, q) in points.iter().enumerate() {
            for (which, fe) in [&q.x, &q.y, &q.z, &q.t].iter().enumerate() {
                let l = limbs(fe);
                for (k, v) in l.iter().enumerate() {
                    state[which * 8 * threads as usize + k * threads as usize + t] = *v;
                }
            }
        }

        let width = curve::scalar_base_mult(
            &scalar(u64::from(threads) * u64::from(SLOTS) * 8),
            &Point::basepoint(),
            &two_d,
        );
        let cached = curve::to_cached(&width, &two_d);
        // Scaled so that Z is one. Every cached value divides by Z, which
        // leaves the same point projectively and turns the product Z1 * Z2 in
        // the addition into Z1 — one multiply out of eight, bought with a
        // single inversion here, once per run.
        let zinv = cached.z.carry().invert();
        let mut step_q = Vec::with_capacity(24);
        for fe in [
            cached.y_plus_x.carry().mul(&zinv),
            cached.y_minus_x.carry().mul(&zinv),
            cached.t2d.carry().mul(&zinv),
        ] {
            step_q.extend_from_slice(&limbs(&fe));
        }

        // The device tests one bit at a time, so the map travels as 32-bit
        // words.
        let mut halves = Vec::with_capacity(bitmap.len() * 2);
        for w in bitmap {
            halves.push(*w as u32);
            halves.push((*w >> 32) as u32);
        }

        let words = threads as usize * SLOTS as usize * 8;
        // Room for the survivors of one launch, from the rate the index lets
        // through. A fixed number here lost hits without saying so whenever a
        // narrow index met a wide batch.
        let limit = super::hit_room(u64::from(threads) * u64::from(SLOTS), bitmap_bits);
        Ok(Engine {
            state: stream.clone_htod(&state)?,
            step_q: stream.clone_htod(&step_q)?,
            bitmap: stream.clone_htod(&halves)?,
            denbuf: stream.alloc_zeros::<u32>(words)?,
            numbuf: stream.alloc_zeros::<u32>(words)?,
            table: stream.clone_htod(&table)?,
            prod: stream.alloc_zeros::<u32>(threads as usize * 8)?,
            hits: stream.alloc_zeros::<u32>(limit as usize * 10)?,
            counter: stream.alloc_zeros::<u32>(1)?,
            bitmap_bits,
            threads,
            block: 128,
            limit,
            steps_done: 0,
            steps,
            reduce,
            stream,
            _ctx: ctx,
        })
    }

    pub fn threads(&self) -> u32 {
        self.threads
    }

    /// How many candidates one call to [`Engine::advance`] examines.
    pub fn batch(&self) -> u64 {
        u64::from(self.threads) * u64::from(SLOTS)
    }

    /// Advances every chain and returns what the bitmap did not rule out.
    pub fn advance(&mut self) -> Result<Vec<super::Survivor>, String> {
        self.stream
            .memset_zeros(&mut self.counter)
            .map_err(|e| e.to_string())?;
        let cfg = LaunchConfig {
            grid_dim: (self.threads.div_ceil(self.block), 1, 1),
            block_dim: (self.block, 1, 1),
            shared_mem_bytes: 0,
        };
        let half = HALF;
        let slots = SLOTS;
        let n = self.threads;
        let limit = self.limit;
        let bits = self.bitmap_bits;
        let mut args = self.stream.launch_builder(&self.steps);
        args.arg(&mut self.state)
            .arg(&self.step_q)
            .arg(&mut self.denbuf)
            .arg(&mut self.numbuf)
            .arg(&half)
            .arg(&n)
            .arg(&self.table)
            .arg(&mut self.prod);
        unsafe { args.launch(cfg) }.map_err(|e| e.to_string())?;
        let mut args = self.stream.launch_builder(&self.reduce);
        args.arg(&mut self.hits)
            .arg(&mut self.counter)
            .arg(&limit)
            .arg(&self.denbuf)
            .arg(&self.numbuf)
            .arg(&self.prod)
            .arg(&self.bitmap)
            .arg(&bits)
            .arg(&slots)
            .arg(&n);
        unsafe { args.launch(cfg) }.map_err(|e| e.to_string())?;
        self.stream.synchronize().map_err(|e| e.to_string())?;

        let found = self
            .stream
            .clone_dtoh(&self.counter)
            .map_err(|e| e.to_string())?[0];
        let kept = found.min(self.limit) as usize;
        let raw = self
            .stream
            .clone_dtoh(&self.hits)
            .map_err(|e| e.to_string())?;
        let launch = self.steps_done;
        self.steps_done += 1;

        let mut out = Vec::with_capacity(kept);
        for i in 0..kept {
            let base = i * 10;
            let tid = u64::from(raw[base]);
            let slot = u64::from(raw[base + 1]);
            // The window this thread owns, and where in it the slot sits.
            let window = tid + launch * u64::from(self.threads);
            let centre = (window * u64::from(SLOTS) + u64::from(HALF) + 1) * 8;
            let offset = if slot == u64::from(SLOTS) - 1 {
                centre
            } else {
                let stepped = (slot / 2 + 1) * 8;
                if slot % 2 == 0 {
                    centre + stepped
                } else {
                    centre - stepped
                }
            };
            let mut packed = [0u8; 32];
            for k in 0..8 {
                packed[k * 4..k * 4 + 4].copy_from_slice(&raw[base + 2 + k].to_le_bytes());
            }
            out.push(super::Survivor { offset, packed });
        }
        if found > self.limit {
            // Silently keeping fewer is the one outcome this must not have.
            return Err(format!(
                "one launch produced {found} survivors and only {} fit; the index is too \
                 narrow for what it is filtering — a wider --index-bits is the answer",
                self.limit
            ));
        }
        Ok(out)
    }
}
