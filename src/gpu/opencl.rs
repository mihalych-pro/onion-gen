//! The last resort among device paths: OpenCL.
//!
//! Asked after the portable and vendor paths, and behind both where all three
//! work: 590 M candidates/s against Vulkan's 876 on an RTX 4060. It is reached
//! only when the other two have said no, and there it beats the processor of
//! the same machine by 2.3x on an M1 Pro and 8.8x on a 4060.
//!
//! Like the other two it opens its library at run time rather than linking it,
//! so the binary starts on a machine with no OpenCL at all.

use super::{Api, Availability, Device, Survivor};
use crate::curve::{self, Point};
use crate::field::Fe;
use opencl3::command_queue::CommandQueue;
use opencl3::context::Context;
use opencl3::device::{Device as ClDevice, CL_DEVICE_TYPE_ALL};
use opencl3::kernel::{ExecuteKernel, Kernel};
use opencl3::memory::{Buffer, CL_MEM_READ_ONLY, CL_MEM_READ_WRITE};
use opencl3::platform::get_platforms;
use opencl3::program::Program;
use opencl3::types::{cl_uint, CL_BLOCKING};

/// The kernel, in the order the pieces depend on each other. OpenCL C has no
/// include of its own, so the host concatenates them — and compiles them on the
/// machine that runs them, which is what lets one binary serve drivers that
/// share no bytecode.
const KERNEL: &str = concat!(
    include_str!("opencl-field.cl"),
    include_str!("opencl-body.cl"),
    include_str!("opencl-chain.cl")
);

/// How many candidates each work item produces per launch.
/// Offsets in the table, and so pairs per work item per launch.
/// The launch shapes the automatic choice walks, widest batch first.
///
/// `half` is offsets per work item, so candidates per item are `2*half + 1`;
/// `threads` is how many items run at once. The product is what the device
/// holds in buffers, which is why the two move against each other.
///
/// An integrated part keeps its table in local memory and so cannot go past
/// what a work group is allowed — 96 offsets is 9 KiB, and 384 would be 36.
fn shapes(discrete: bool) -> Vec<(u32, u32)> {
    if discrete {
        vec![
            (384, 65_536),
            (384, 32_768),
            (256, 131_072),
            (256, 65_536),
            (96, 262_144),
            (96, 131_072),
            (96, 32_768),
        ]
    } else {
        vec![(96, 131_072), (96, 65_536), (96, 32_768), (96, 8_192)]
    }
}

/// Offsets per work item, chosen per device.
///
/// More of them means one inversion and one base-point advance spread over
/// more candidates, which is what the rate follows. Measured on an RTX 4060:
/// 1634 M/s at 96, 1725 at 256, 1759 at 384, 1737 at 512 — so the useful
/// ceiling is where occupancy starts losing more than the amortisation gains.
///
/// An integrated part keeps the small one. Its copy of the table lives in
/// local memory, and 384 offsets would be 36 KiB of it — past what Apple
/// allows a work group, where the kernel simply produces nothing.
fn half_for(discrete: bool) -> u32 {
    if discrete {
        384
    } else {
        96
    }
}

/// Everything this path can run on.
pub fn look() -> Availability {
    let platforms = match get_platforms() {
        Ok(p) => p,
        // Not finding the library is the ordinary case on a machine with no
        // OpenCL, and must cost nothing but a sentence.
        Err(e) => return Availability::NoDriver(format!("no OpenCL here ({e})")),
    };
    let mut devices = Vec::new();
    for platform in platforms {
        let Ok(ids) = platform.get_devices(CL_DEVICE_TYPE_ALL) else {
            continue;
        };
        for id in ids {
            let d = ClDevice::new(id);
            // A device sharing the host's memory is an integrated part, and the
            // batch it should be asked for is smaller. Where the driver will
            // not say, a discrete card is the safer guess: the walk down finds
            // a size that fits, while too small a batch is simply slow.
            let discrete = !d.host_unified_memory().unwrap_or(false);
            devices.push(Device {
                index: 0,
                api: Api::OpenCl,
                ordinal: devices.len(),
                name: d.name().unwrap_or_else(|_| "unnamed".to_string()),
                memory: d.global_mem_size().unwrap_or(0) as usize,
                discrete,
                also_via: None,
            });
        }
    }
    if devices.is_empty() {
        Availability::NoDevice
    } else {
        Availability::Devices(devices)
    }
}

/// A field element as the kernel holds it: eight 32-bit limbs, least
/// significant first, which is the canonical encoding read as words.
fn limbs(fe: &Fe) -> [u32; 8] {
    let b = fe.to_bytes();
    std::array::from_fn(|k| u32::from_le_bytes(b[k * 4..k * 4 + 4].try_into().unwrap()))
}

fn scalar(value: u64) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[..8].copy_from_slice(&value.to_le_bytes());
    out
}

/// One blocking write. Blocking on purpose: these happen at set-up and once per
/// launch for a single word, and an asynchronous one would only add a handle to
/// wait on.
fn write(queue: &CommandQueue, buffer: &mut Buffer<cl_uint>, data: &[u32]) -> Result<(), String> {
    let event = unsafe { queue.enqueue_write_buffer(buffer, CL_BLOCKING, 0, data, &[]) }
        .map_err(|e| format!("{e}"))?;
    event.wait().map_err(|e| format!("{e}"))
}

/// One device, its state, and the buffers it works in.
pub struct Engine {
    queue: CommandQueue,
    kernel: Kernel,
    state: Buffer<cl_uint>,
    step_q: Buffer<cl_uint>,
    denbuf: Buffer<cl_uint>,
    numbuf: Buffer<cl_uint>,
    table: Buffer<cl_uint>,
    bitmap: Buffer<cl_uint>,
    hits: Buffer<cl_uint>,
    counter: Buffer<cl_uint>,
    bitmap_bits: u32,
    threads: u32,
    /// Offsets per work item on this device, and candidates per work item.
    half: u32,
    slots: u32,
    limit: u32,
    steps_done: u64,
    // Dropped last: the buffers and the kernel are its children.
    _context: Context,
}

/// Opens a command queue through the newest call the device supports.
///
/// `clCreateCommandQueue` was deprecated in OpenCL 2.0 and replaced by
/// `clCreateCommandQueueWithProperties`. Both are reached at run time rather
/// than linked, so this can ask for the newer one where the driver has it and
/// fall back where it does not — which is every Apple device, whose OpenCL
/// stopped at 1.2 and exports no 2.0 entry point at all.
///
/// Measured on an RTX 4060: the two queues run the same kernel at the same
/// rate. The newer call is used because the older one is deprecated, not
/// because it is faster.
fn open_queue(context: &Context, device: &opencl3::device::Device) -> Result<CommandQueue, String> {
    if offers_two_zero(device) {
        if let Ok(queue) = CommandQueue::create_default_with_properties(context, 0, 0) {
            return Ok(queue);
        }
    }
    CommandQueue::create_default(context, 0).map_err(|e| format!("{e}"))
}

/// Whether the device reports OpenCL 2.0 or later.
///
/// The string is `OpenCL <major>.<minor> <vendor text>`, which the
/// specification fixes, so the two numbers are where they are expected.
fn offers_two_zero(device: &opencl3::device::Device) -> bool {
    let text = device.version().unwrap_or_default();
    parse_cl_version(&text).is_some_and(|(major, _)| major >= 2)
}

/// The major and minor from an OpenCL version string.
fn parse_cl_version(text: &str) -> Option<(u32, u32)> {
    let rest = text.strip_prefix("OpenCL ")?;
    let number = rest.split_whitespace().next()?;
    let (major, minor) = number.split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

impl Engine {
    /// Opens the device, settling the launch shape by running it.
    ///
    /// `threads` of zero means "decide": each shape below is built and timed
    /// over one launch, and the fastest is kept. The alternative — picking the
    /// largest that fits in memory — was measured at 302 M/s on an RTX 4060
    /// where the best shape reached 1759, because the widest batch is not the
    /// fastest one once occupancy is counted.
    ///
    /// A named `threads` skips all of it and is taken as given.
    pub fn new(
        want: &Device,
        start: &Point,
        threads: u32,
        bitmap: &[u64],
        bitmap_bits: u32,
        retune: bool,
    ) -> Result<Self, String> {
        if threads != 0 {
            return Self::sized(
                want,
                start,
                threads,
                half_for(want.discrete),
                bitmap,
                bitmap_bits,
            );
        }

        // What this device settled on last time. Both numbers are recorded,
        // because this path varies the offsets per work item as well as how
        // many work items run at once.
        let key = crate::tuning::device_key(&want.name, "opencl", 0);
        if !retune {
            if let Some(shape) = crate::tuning::get(&key) {
                if let Ok(engine) =
                    Self::sized(want, start, shape.second, shape.first, bitmap, bitmap_bits)
                {
                    return Ok(engine);
                }
                // It no longer builds — less free memory than there was. Fall
                // through and measure again.
            }
        }

        let mut best: Option<(f64, Self)> = None;
        let mut last = String::from("no shape this device would take");
        for (half, threads) in shapes(want.discrete) {
            let mut engine = match Self::sized(want, start, threads, half, bitmap, bitmap_bits) {
                Ok(e) => e,
                Err(e) => {
                    last = e;
                    continue;
                }
            };
            let Some(rate) = engine.time_one_launch() else {
                continue;
            };
            if best.as_ref().is_none_or(|(had, _)| rate > *had) {
                best = Some((rate, engine));
            }
        }
        if let Some((rate, engine)) = &best {
            crate::tuning::put(
                &key,
                crate::tuning::Shape {
                    first: engine.half,
                    second: engine.threads,
                    rate: *rate,
                    at: crate::tuning::now(),
                },
            );
        }
        best.map(|(_, e)| e).ok_or(last)
    }

    /// Times one launch, in candidates a second.
    ///
    /// One is enough to rank shapes: the kernel does the same work every time,
    /// and the differences between shapes are tens of percent rather than the
    /// few percent a single sample is uncertain by.
    fn time_one_launch(&mut self) -> Option<f64> {
        // The first launch pays for the table reaching the device, so it is
        // run and thrown away.
        self.advance().ok()?;
        let at = std::time::Instant::now();
        self.advance().ok()?;
        let seconds = at.elapsed().as_secs_f64();
        (seconds > 0.0).then(|| self.batch() as f64 / seconds)
    }

    fn sized(
        want: &Device,
        start: &Point,
        threads: u32,
        half: u32,
        bitmap: &[u64],
        bitmap_bits: u32,
    ) -> Result<Self, String> {
        let platforms = get_platforms().map_err(|e| format!("{e}"))?;
        let mut at = 0usize;
        let mut chosen = None;
        'outer: for platform in platforms {
            let Ok(ids) = platform.get_devices(CL_DEVICE_TYPE_ALL) else {
                continue;
            };
            for id in ids {
                if at == want.ordinal {
                    chosen = Some(ClDevice::new(id));
                    break 'outer;
                }
                at += 1;
            }
        }
        let device = chosen.ok_or_else(|| format!("{} is no longer there", want.name))?;
        let context = Context::from_device(&device).map_err(|e| format!("{e}"))?;
        let queue = open_queue(&context, &device)?;
        // Where the offset table lives, decided per device: a work group's own
        // copy pays on an integrated part and costs occupancy on a discrete
        // card. Measured both ways on both kinds; the numbers are in the
        // kernel beside the switch.
        let shared = u32::from(!want.discrete);
        let slots = 2 * half + 1;
        let options = format!("-D OG_HALF={half} -D OG_SHARED_TABLE={shared}");
        let program = Program::create_and_build_from_source(&context, KERNEL, &options)
            .map_err(|e| format!("the kernel did not build: {e}"))?;
        let kernel = Kernel::create(&program, "chain").map_err(|e| format!("{e}"))?;

        let n = threads as usize;
        let words = n * slots as usize * 8;
        let limit = super::hit_room(u64::from(threads) * u64::from(slots), bitmap_bits);

        // Work item t owns the window of SLOTS consecutive candidates starting
        // at t * SLOTS, with its base point at that window's centre; every
        // launch moves each of them on by the whole width, so no two ever meet.
        // Coordinate-major, limb-major, thread-minor: the layout the kernel
        // reads, where neighbouring items touch neighbouring words.
        let two_d_init = curve::two_d();
        let window = curve::to_cached(
            &curve::scalar_base_mult(
                &scalar(u64::from(slots) * 8),
                &Point::basepoint(),
                &two_d_init,
            ),
            &two_d_init,
        );
        let to_centre = curve::to_cached(
            &curve::scalar_base_mult(
                &scalar(u64::from(half + 1) * 8),
                &Point::basepoint(),
                &two_d_init,
            ),
            &two_d_init,
        );
        let mut p = curve::to_p3(&curve::add(start, &to_centre));
        let mut state = vec![0u32; n * 32];
        for t in 0..n {
            for (which, fe) in [&p.x, &p.y, &p.z, &p.t].iter().enumerate() {
                for (k, v) in limbs(fe).iter().enumerate() {
                    state[which * 8 * n + k * n + t] = *v;
                }
            }
            p = curve::to_p3(&curve::add(&p, &window));
        }

        let two_d = curve::two_d();
        let width = curve::scalar_base_mult(
            &scalar(u64::from(threads) * u64::from(slots) * 8),
            &Point::basepoint(),
            &two_d,
        );
        let cached = curve::to_cached(&width, &two_d);
        // Scaled so that Z is one, which turns the product Z1 * Z2 in the
        // addition into Z1 — one multiply out of eight, for one inversion here.
        let zinv = cached.z.carry().invert();
        let mut step_q = Vec::with_capacity(24);
        for fe in [
            cached.y_plus_x.carry().mul(&zinv),
            cached.y_minus_x.carry().mul(&zinv),
            cached.t2d.carry().mul(&zinv),
        ] {
            step_q.extend_from_slice(&limbs(&fe));
        }

        // Q(m) = 8(m+1)*G, affine and with the product of its coordinates.
        let mut table = Vec::with_capacity(half as usize * 24);
        let mut q = curve::scalar_base_mult(&scalar(8), &Point::basepoint(), &two_d);
        let eight = curve::eight_basepoint_cached();
        for m in 0..half {
            let zinv = q.z.invert();
            let x = q.x.mul(&zinv);
            let y = q.y.mul(&zinv);
            for fe in [x, y, x.mul(&y)] {
                table.extend_from_slice(&limbs(&fe));
            }
            if m + 1 < half {
                q = curve::to_p3(&curve::add(&q, &eight));
            }
        }

        // The device tests one bit at a time, so the map travels as 32-bit
        // words.
        let mut halves = Vec::with_capacity(bitmap.len() * 2);
        for w in bitmap {
            halves.push(*w as u32);
            halves.push((*w >> 32) as u32);
        }

        let make = |len: usize, flags| -> Result<Buffer<cl_uint>, String> {
            unsafe { Buffer::create(&context, flags, len, std::ptr::null_mut()) }
                .map_err(|e| format!("no buffer of {len} words: {e}"))
        };
        let mut engine = Engine {
            half,
            slots,
            state: make(state.len(), CL_MEM_READ_WRITE)?,
            step_q: make(step_q.len(), CL_MEM_READ_ONLY)?,
            denbuf: make(words, CL_MEM_READ_WRITE)?,
            numbuf: make(words, CL_MEM_READ_WRITE)?,
            table: make(table.len(), CL_MEM_READ_ONLY)?,
            bitmap: make(halves.len(), CL_MEM_READ_ONLY)?,
            hits: make(limit as usize * 10, CL_MEM_READ_WRITE)?,
            counter: make(1, CL_MEM_READ_WRITE)?,
            bitmap_bits,
            threads,
            limit,
            steps_done: 0,
            queue,
            kernel,
            _context: context,
        };
        write(&engine.queue, &mut engine.state, &state)?;
        write(&engine.queue, &mut engine.step_q, &step_q)?;
        write(&engine.queue, &mut engine.bitmap, &halves)?;
        write(&engine.queue, &mut engine.table, &table)?;
        Ok(engine)
    }

    pub fn threads(&self) -> u32 {
        self.threads
    }

    pub fn batch(&self) -> u64 {
        u64::from(self.threads) * u64::from(self.slots)
    }

    pub fn advance(&mut self) -> Result<Vec<Survivor>, String> {
        write(&self.queue, &mut self.counter, &[0u32])?;

        let limit = self.limit;
        let half = self.half;
        let n = self.threads;
        let bits = self.bitmap_bits;
        // The kernel's work group is 64 wide, and the range has to be a whole
        // number of them.
        let local = 64usize;
        let global = (n as usize).div_ceil(local) * local;
        let event = unsafe {
            ExecuteKernel::new(&self.kernel)
                .set_arg(&self.hits)
                .set_arg(&self.counter)
                .set_arg(&limit)
                .set_arg(&self.state)
                .set_arg(&self.step_q)
                .set_arg(&self.denbuf)
                .set_arg(&self.numbuf)
                .set_arg(&self.bitmap)
                .set_arg(&bits)
                .set_arg(&half)
                .set_arg(&n)
                .set_arg(&self.table)
                .set_global_work_size(global)
                .set_local_work_size(local)
                .enqueue_nd_range(&self.queue)
        }
        .map_err(|e| format!("{e}"))?;
        event.wait().map_err(|e| format!("{e}"))?;

        let mut found = [0u32; 1];
        let read = unsafe {
            self.queue
                .enqueue_read_buffer(&self.counter, CL_BLOCKING, 0, &mut found, &[])
        }
        .map_err(|e| format!("{e}"))?;
        read.wait().map_err(|e| format!("{e}"))?;

        let launch = self.steps_done;
        self.steps_done += 1;
        let kept = found[0].min(self.limit) as usize;
        let mut out = Vec::with_capacity(kept);
        if kept > 0 {
            let mut raw = vec![0u32; kept * 10];
            let read = unsafe {
                self.queue
                    .enqueue_read_buffer(&self.hits, CL_BLOCKING, 0, &mut raw, &[])
            }
            .map_err(|e| format!("{e}"))?;
            read.wait().map_err(|e| format!("{e}"))?;
            for i in 0..kept {
                let base = i * 10;
                let tid = u64::from(raw[base]);
                let slot = u64::from(raw[base + 1]);
                // The window this item owns, and where in it the slot sits.
                let window = tid + launch * u64::from(self.threads);
                let centre = (window * u64::from(self.slots) + u64::from(self.half) + 1) * 8;
                let offset = if slot == u64::from(self.slots) - 1 {
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
                out.push(Survivor { offset, packed });
            }
        }
        if found[0] > self.limit {
            // Silently keeping fewer is the one outcome this must not have.
            return Err(format!(
                "one launch produced {} survivors and only {} fit; the index is too \
                 narrow for what it is filtering — a wider --index-bits is the answer",
                found[0], self.limit
            ));
        }
        Ok(out)
    }
}
