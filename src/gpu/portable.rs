//! The portable path: one kernel in WGSL, reaching Vulkan, Metal and DX12
//! through `wgpu`.
//!
//! The only path on machines the vendor path does not serve — every AMD and
//! Intel card, and every Mac — and the faster of the two on NVIDIA as well.
//!
//! The kernel needs 64-bit integers: a field multiply accumulates products of
//! 32-bit limbs, and without a 64-bit accumulator the same arithmetic costs two
//! to four times as much. WGSL has no 64-bit integer of its own, so a device
//! that does not report `SHADER_INT64` is listed as found and unusable rather
//! than served slowly. That excludes DX12 and OpenGL, both of which sit behind
//! Vulkan on the same hardware anyway.

use super::{Api, Availability, Device, Survivor};
use crate::curve::{self, Point};
use crate::field::Fe;
use std::sync::Arc;
use wgpu::util::DeviceExt;

/// How many candidates each invocation produces per launch.
///
/// The single inversion is amortised over these, so too few makes it dominate;
/// the scratch buffers are proportional to it, so too many will not fit.
/// Offsets in the table, and so pairs per invocation per launch.
const HALF: u32 = 96;
/// Candidates per invocation per launch: both members of every pair, plus the
/// base point itself.
const SLOTS: u32 = 2 * HALF + 1;

/// The kernel. Three files because the field, the rest of the field and the
/// chain are read separately; WGSL has no include of its own.
const KERNEL: &str = concat!(
    include_str!("field.wgsl"),
    include_str!("body.wgsl"),
    include_str!("chain.wgsl")
);

fn instance() -> wgpu::Instance {
    wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle())
}

fn api_of(backend: wgpu::Backend) -> Option<Api> {
    match backend {
        wgpu::Backend::Vulkan => Some(Api::Vulkan),
        wgpu::Backend::Metal => Some(Api::Metal),
        wgpu::Backend::Dx12 => Some(Api::Dx12),
        wgpu::Backend::Gl => Some(Api::Gl),
        _ => None,
    }
}

/// Everything this path can run on, optionally narrowed to one API.
pub fn look(only: Option<Api>) -> Availability {
    let adapters = pollster::block_on(instance().enumerate_adapters(wgpu::Backends::all()));
    let mut devices = Vec::new();
    let mut rejected = Vec::new();
    for a in adapters {
        let info = a.get_info();
        // A software rasteriser would run the chain correctly and far slower
        // than the processor it is running on.
        if info.device_type == wgpu::DeviceType::Cpu {
            continue;
        }
        let Some(api) = api_of(info.backend) else {
            continue;
        };
        if only.is_some_and(|w| w != api) {
            continue;
        }
        if !a.features().contains(wgpu::Features::SHADER_INT64) {
            rejected.push(format!(
                "{} via {} has no 64-bit integers",
                info.name,
                api.name()
            ));
            continue;
        }
        devices.push(Device {
            // Filled in by the caller once both paths have been asked.
            index: 0,
            api,
            ordinal: devices.len(),
            discrete: info.device_type == wgpu::DeviceType::DiscreteGpu,
            name: info.name,
            // `wgpu` reports no device memory, and guessing it would be worse
            // than saying nothing.
            memory: 0,
            also_via: None,
        });
    }
    if !devices.is_empty() {
        return Availability::Devices(devices);
    }
    if rejected.is_empty() {
        // Nothing was enumerated at all, which on a machine with a card means
        // the driver is missing rather than the card.
        Availability::NoDriver(
            "no Vulkan or Metal driver answered; on Linux that is the vendor's \
             Vulkan package, not the compiler toolkit"
                .to_string(),
        )
    } else {
        Availability::NoDriver(rejected.join("; "))
    }
}

/// One field element as the device holds it: radix 2^25.5 in ten 32-bit limbs.
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

/// One device, its state, and the buffers it works in.
pub struct Engine {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    bind: wgpu::BindGroup,
    hits: wgpu::Buffer,
    counter: wgpu::Buffer,
    staging: wgpu::Buffer,
    threads: u32,
    hit_limit: u32,
    launches_done: u64,
    errors: Arc<std::sync::Mutex<Option<String>>>,
}

impl Engine {
    /// Prepares a device to continue the chain that starts at `start`.
    ///
    /// `bitmap` and `bitmap_bits` are the prefix index; without one there is
    /// nothing for the device to filter on and every candidate would have to
    /// cross back, which the link cannot carry.
    pub fn new(
        want: &Device,
        start: &Point,
        threads: u32,
        bitmap: &[u64],
        bitmap_bits: u32,
    ) -> Result<Self, String> {
        let instance = instance();
        let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
        let mut at = 0usize;
        let mut chosen = None;
        for a in adapters {
            let info = a.get_info();
            if info.device_type == wgpu::DeviceType::Cpu {
                continue;
            }
            if api_of(info.backend) != Some(want.api) {
                continue;
            }
            if !a.features().contains(wgpu::Features::SHADER_INT64) {
                continue;
            }
            if at == want.ordinal {
                chosen = Some(a);
                break;
            }
            at += 1;
        }
        let adapter = chosen.ok_or_else(|| format!("{} is no longer there", want.name))?;

        let limits = adapter.limits();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("onion-gen"),
            required_features: wgpu::Features::SHADER_INT64,
            required_limits: limits.clone(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .map_err(|e| format!("{e}"))?;

        // The product aborts on panic, and `wgpu` reports a failed allocation
        // by panicking from whichever thread happens to be inside it. Catching
        // it here turns a dead process into a device that stops and says why.
        let errors = Arc::new(std::sync::Mutex::new(None));
        let sink = Arc::clone(&errors);
        device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| {
            let mut slot = sink.lock().unwrap_or_else(|p| p.into_inner());
            if slot.is_none() {
                *slot = Some(e.to_string());
            }
        }));

        let n = threads as usize;
        let plane = n * SLOTS as usize * 8 * 4;
        if plane as u64 > limits.max_storage_buffer_binding_size {
            return Err(format!(
                "{threads} threads x {SLOTS} candidates needs {} MiB in one buffer and this device \
                 allows {} MiB; ask for fewer threads",
                plane / (1024 * 1024),
                limits.max_storage_buffer_binding_size / (1024 * 1024)
            ));
        }

        // Invocation t owns the window of SLOTS consecutive candidates that
        // starts at t * SLOTS, and its base point sits at that window's centre.
        // Every launch moves each of them on by the whole width, so no two
        // invocations ever meet.
        let two_d_init = curve::two_d();
        let window = curve::to_cached(
            &curve::scalar_base_mult(
                &scalar(u64::from(SLOTS) * 8),
                &Point::basepoint(),
                &two_d_init,
            ),
            &two_d_init,
        );
        let to_centre = curve::to_cached(
            &curve::scalar_base_mult(
                &scalar(u64::from(HALF + 1) * 8),
                &Point::basepoint(),
                &two_d_init,
            ),
            &two_d_init,
        );
        let mut p = curve::to_p3(&curve::add(start, &to_centre));
        // Coordinate-major, limb-major, thread-minor: the layout the kernel
        // reads, and the one where neighbouring invocations touch neighbouring
        // words.
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

        // Q(m) = 8(m+1)*G, affine and with the product of its coordinates, which
        // is what the pair formula reads. Built once and shared by every
        // invocation.
        let mut table = Vec::with_capacity(HALF as usize * 24);
        let mut q = curve::scalar_base_mult(&scalar(8), &Point::basepoint(), &two_d);
        let eight = curve::eight_basepoint_cached();
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

        // The device tests one bit at a time, so the map travels as 32-bit
        // words.
        let mut halves = Vec::with_capacity(bitmap.len() * 2);
        for w in bitmap {
            halves.push(*w as u32);
            halves.push((*w >> 32) as u32);
        }

        let storage = |data: &[u32]| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(data),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            })
        };
        let zeros = |words: usize| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: (words * 4) as u64,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };

        let words = n * SLOTS as usize * 8;
        let hit_limit = super::hit_room(u64::from(threads) * u64::from(SLOTS), bitmap_bits);
        let hits = zeros(hit_limit as usize * 10);
        let counter = zeros(1);
        let state = storage(&state);
        let step_q = storage(&step_q);
        let denbuf = zeros(words);
        let numbuf = zeros(words);
        let bitmap = storage(&halves);
        let table = storage(&table);
        let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&[hit_limit, bitmap_bits, HALF, threads]),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("chain"),
            source: wgpu::ShaderSource::Wgsl(KERNEL.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("chain"),
            layout: None,
            module: &module,
            entry_point: Some("chain"),
            compilation_options: Default::default(),
            cache: None,
        });

        // Built once: rebuilding it per launch showed up as a measurable share
        // of a launch that is only a few milliseconds long.
        let binding = [
            &hits, &counter, &state, &step_q, &denbuf, &numbuf, &bitmap, &params, &table,
        ];
        let entries: Vec<wgpu::BindGroupEntry> = binding
            .iter()
            .enumerate()
            .map(|(i, b)| wgpu::BindGroupEntry {
                binding: i as u32,
                resource: b.as_entire_binding(),
            })
            .collect();
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &entries,
        });

        // The counter and the hit list come back in one mapping: two would be
        // two round trips, and the round trip is what costs.
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(hit_limit) * 40 + 4,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        if let Some(e) = errors.lock().unwrap_or_else(|p| p.into_inner()).take() {
            return Err(e);
        }
        Ok(Engine {
            device,
            queue,
            pipeline,
            bind,
            hits,
            counter,
            staging,
            threads,
            hit_limit,
            launches_done: 0,
            errors,
        })
    }

    pub fn threads(&self) -> u32 {
        self.threads
    }

    pub fn batch(&self) -> u64 {
        u64::from(self.threads) * u64::from(SLOTS)
    }

    pub fn advance(&mut self) -> Result<Vec<Survivor>, String> {
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        enc.clear_buffer(&self.counter, 0, None);
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: None,
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind, &[]);
            // The kernel's workgroup is 64 wide.
            pass.dispatch_workgroups(self.threads.div_ceil(64), 1, 1);
        }
        enc.copy_buffer_to_buffer(&self.counter, 0, &self.staging, 0, 4);
        enc.copy_buffer_to_buffer(
            &self.hits,
            0,
            &self.staging,
            4,
            u64::from(self.hit_limit) * 40,
        );
        self.queue.submit(Some(enc.finish()));

        let slice = self.staging.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .map_err(|e| format!("{e}"))?;
        if let Some(e) = self.errors.lock().unwrap_or_else(|p| p.into_inner()).take() {
            self.staging.unmap();
            return Err(e);
        }
        let view = slice.get_mapped_range().map_err(|e| format!("{e}"))?;
        let raw: &[u32] = bytemuck::cast_slice(&view);

        let found = raw[0];
        let kept = found.min(self.hit_limit) as usize;
        let launch = self.launches_done;
        self.launches_done += 1;
        let slots = u64::from(SLOTS);
        let half = u64::from(HALF);

        let mut out = Vec::with_capacity(kept);
        for i in 0..kept {
            let base = 1 + i * 10;
            let tid = u64::from(raw[base]);
            let slot = u64::from(raw[base + 1]);
            // Invocation `tid` of this launch owns the window of SLOTS
            // candidates starting at `window * SLOTS`, and its base point is
            // that window's centre. Slot `2m` is the centre plus `8(m+1)`,
            // slot `2m+1` the centre minus it, and the last slot is the centre.
            let window = tid + launch * u64::from(self.threads);
            let centre = (window * slots + half + 1) * 8;
            let offset = if slot == slots - 1 {
                centre
            } else {
                let step = (slot / 2 + 1) * 8;
                if slot % 2 == 0 {
                    centre + step
                } else {
                    centre - step
                }
            };
            let mut packed = [0u8; 32];
            for k in 0..8 {
                packed[k * 4..k * 4 + 4].copy_from_slice(&raw[base + 2 + k].to_le_bytes());
            }
            out.push(Survivor { offset, packed });
        }
        drop(view);
        self.staging.unmap();
        if found > self.hit_limit {
            // Silently keeping fewer is the one outcome this must not have.
            return Err(format!(
                "one launch produced {found} survivors and only {} fit; the index is too \
                 narrow for what it is filtering — a wider --index-bits is the answer",
                self.hit_limit
            ));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::KERNEL;

    /// The shader is compiled by the driver on the machine that runs it, so
    /// nothing here would catch a typo in it — the first sign would be a user
    /// with a device and an error message. Parsing and validating it in the
    /// test run closes that, and needs no device.
    #[test]
    fn the_shader_parses_and_validates() {
        let module = naga::front::wgsl::parse_str(KERNEL)
            .unwrap_or_else(|e| panic!("the kernel does not parse: {}", e.emit_to_string(KERNEL)));
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            // The one capability beyond the base language that the field
            // arithmetic needs, and the one the device is checked for.
            naga::valid::Capabilities::SHADER_INT64,
        );
        validator
            .validate(&module)
            .unwrap_or_else(|e| panic!("the kernel does not validate: {e:?}"));
    }

    /// The host builds the workgroup count by dividing by this, so the two
    /// have to agree; they are in different files and different languages.
    #[test]
    fn the_workgroup_width_matches_what_the_host_divides_by() {
        assert!(
            KERNEL.contains("@workgroup_size(64)"),
            "the kernel's workgroup is not 64 wide, but `advance` divides by 64"
        );
    }
}
