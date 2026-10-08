//! The graphics device as a second source of candidates.
//!
//! Two ways to reach one: the vendor path, NVIDIA only, and the portable path,
//! anywhere there is a Vulkan or Metal driver. On an NVIDIA card both work, and
//! the choice between them is made by rule here and can be overridden.
//!
//! What the device cannot do — deciding *which* filter matched, writing keys —
//! stays on the host either way.

pub mod cuda;
pub mod opencl;
pub mod portable;

use crate::curve::Point;
use std::fmt;

/// Which API reaches a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Api {
    Cuda,
    Vulkan,
    Metal,
    Dx12,
    Gl,
    OpenCl,
}

impl Api {
    pub fn name(self) -> &'static str {
        match self {
            Api::Cuda => "cuda",
            Api::Vulkan => "vulkan",
            Api::Metal => "metal",
            Api::Dx12 => "dx12",
            Api::Gl => "gl",
            Api::OpenCl => "opencl",
        }
    }

    /// Whether this API is reached through the portable path, which is `wgpu`
    /// and nothing else: OpenCL is portable in the ordinary sense of the word
    /// and is not that path.
    pub fn portable(self) -> bool {
        !matches!(self, Api::Cuda | Api::OpenCl)
    }
}

#[derive(Debug, Clone)]
pub struct Device {
    /// Position in the list this run addresses devices by, which is what
    /// `--devices` names. Both paths share one numbering so that a device is
    /// asked for the same way whichever API reaches it.
    pub index: usize,
    pub api: Api,
    /// The index the API's own enumeration uses, which is not the same thing.
    pub ordinal: usize,
    pub name: String,
    /// Bytes, or zero where the API does not report it — `wgpu` does not.
    pub memory: usize,
    /// Another API that reaches this same card and was not chosen. Present so
    /// that the diagnostics can say a choice was made rather than leave the
    /// user to guess why their vendor path is idle.
    pub also_via: Option<Api>,
    /// A card of its own rather than a share of the machine's memory. It
    /// decides how large a batch is reasonable to ask for.
    pub discrete: bool,
}

impl fmt::Display for Device {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {} via {}", self.index, self.name, self.api.name())?;
        if self.memory > 0 {
            write!(f, ", {} MiB", self.memory / (1024 * 1024))?;
        }
        if let Some(other) = self.also_via {
            write!(
                f,
                " (also reachable by {}; --compute {} forces it)",
                other.name(),
                other.name()
            )?;
        }
        Ok(())
    }
}

/// What was found, or why nothing was.
#[derive(Debug)]
pub enum Availability {
    /// Devices, in the order they will be addressed.
    Devices(Vec<Device>),
    /// No driver for this path on this machine. Names what was looked for,
    /// because "no GPU" and "no driver" call for different answers.
    NoDriver(String),
    /// A driver is there and reports nothing to run on.
    NoDevice,
}

/// Which paths the user will accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Want {
    /// Whichever is expected to be faster on each device.
    Any,
    /// The portable path only, optionally through one named API.
    Portable(Option<Api>),
    /// The vendor path only.
    Vendor,
    /// OpenCL only.
    OpenCl,
}

/// A candidate the device could not rule out.
pub struct Survivor {
    /// Steps from the seed's starting point, in the same units the processor
    /// path counts them.
    pub offset: u64,
    /// The public key as the device packed it, without the sign bit.
    pub packed: [u8; 32],
}

/// Which way into one card is preferred, lower first.
///
/// Measured on the cards available here, on the kernels that ship:
///
/// | card | CUDA | Vulkan | OpenCL | Metal |
/// |---|---:|---:|---:|---:|
/// | RTX 4060 | 1862 | 1791 | 1759 | — |
/// | Apple M1 Pro | — | no driver | 250 | 256 |
///
/// So the vendor path wins on NVIDIA and Metal wins on Apple, which is also
/// what each vendor's own tooling assumes. Where neither applies — an AMD or
/// Intel card — Vulkan leads, because it is the only one of the three with a
/// driver on all of them.
fn rank(api: Api) -> u8 {
    match api {
        Api::Cuda => 0,
        Api::Metal => 1,
        Api::Vulkan => 2,
        Api::Dx12 => 3,
        Api::OpenCl => 4,
        Api::Gl => 5,
    }
}

/// Keeps whichever way into this card is preferred, recording the other.
fn prefer(seen: &mut Device, other: Api) {
    if rank(other) < rank(seen.api) {
        seen.also_via = Some(seen.api);
        seen.api = other;
    } else if seen.also_via.is_none() {
        seen.also_via = Some(other);
    }
}

/// Everything reachable, numbered once across both paths.
///
/// All three paths are asked, and a card reached by more than one keeps the
/// way [`rank`] prefers — which is the faster of them where that was measured.
/// The order the paths are asked in therefore decides nothing; it only decides
/// which enumeration numbers a card first.
///
/// A device one path cannot see — no Vulkan driver installed, no vendor
/// runtime — still comes back through another. That is the fallback, and it
/// needs no flag.
pub fn look(want: Want) -> Availability {
    let mut devices = Vec::new();
    let mut why = Vec::new();

    if matches!(want, Want::Any | Want::Portable(_)) {
        let filter = match want {
            Want::Portable(api) => api,
            _ => None,
        };
        match portable::look(filter) {
            Availability::Devices(found) => devices.extend(found),
            Availability::NoDriver(e) => why.push(e),
            Availability::NoDevice => {
                why.push("the portable path found no device it can use".to_string())
            }
        }
    }

    if matches!(want, Want::Any | Want::Vendor) {
        match cuda::look() {
            Availability::Devices(found) => {
                for d in found {
                    // The same physical card reached two ways would otherwise
                    // be counted twice and run two engines against itself.
                    if let Some(seen) = devices.iter_mut().find(|seen| same_card(seen, &d)) {
                        // The ordinal belongs to whichever API is kept: the
                        // two enumerations number cards independently, and
                        // running CUDA against `wgpu`'s index would open a
                        // different card on a machine with two.
                        let ordinal = d.ordinal;
                        if rank(d.api) < rank(seen.api) {
                            seen.ordinal = ordinal;
                        }
                        prefer(seen, d.api);
                        if seen.memory == 0 {
                            // The vendor driver reports the memory `wgpu` does
                            // not, and it is the same card.
                            seen.memory = d.memory;
                        }
                        continue;
                    }
                    devices.push(d);
                }
            }
            Availability::NoDriver(e) => why.push(e),
            Availability::NoDevice => why.push("the vendor driver reports no device".to_string()),
        }
    }

    // Last, and only for what the first two did not reach. Every machine we
    // can measure has it behind both of them, so asking it earlier would put a
    // slower path in front of a faster one on the same card.
    if matches!(want, Want::Any | Want::OpenCl) {
        match opencl::look() {
            Availability::Devices(found) => {
                for d in found {
                    if let Some(seen) = devices.iter_mut().find(|seen| same_card(seen, &d)) {
                        prefer(seen, d.api);
                        continue;
                    }
                    devices.push(d);
                }
            }
            Availability::NoDriver(e) => why.push(e),
            Availability::NoDevice => why.push("OpenCL reports no device".to_string()),
        }
    }

    if devices.is_empty() {
        return if why.is_empty() {
            Availability::NoDevice
        } else {
            Availability::NoDriver(why.join("; "))
        };
    }
    for (i, d) in devices.iter_mut().enumerate() {
        d.index = i;
    }
    Availability::Devices(devices)
}

/// Whether two entries are the same piece of hardware seen through two APIs.
///
/// The name is what both report and the only thing they agree on: `wgpu` gives
/// no bus address and the vendor driver gives no PCI ids through the interface
/// used here. So the comparison is of names, loosely — the two drivers space
/// and capitalise them differently often enough that an exact match would let
/// one card through twice and run two engines against it.
///
/// Two identical cards in one machine collapse into one entry here. That is why
/// the vendor entry is the one dropped rather than the portable one: the
/// portable path enumerates both cards, so nothing is lost.
fn same_card(a: &Device, b: &Device) -> bool {
    loose(&a.name) == loose(&b.name)
}

/// A name with its spacing, case, punctuation and trademark marks taken out,
/// so that "NVIDIA GeForce RTX 4060" and "NVIDIA  GeForce(R) RTX 4060" are one
/// card. The marks come and go between drivers, and Intel's names carry them.
fn loose(name: &str) -> String {
    let mut lowered = name.to_ascii_lowercase();
    for mark in ["(r)", "(tm)", "(c)", "®", "™"] {
        lowered = lowered.replace(mark, "");
    }
    lowered
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect()
}

/// One line for the diagnostics, whatever the outcome.
pub fn describe(want: Want) -> String {
    match look(want) {
        Availability::Devices(devices) => {
            let names: Vec<String> = devices.iter().map(Device::to_string).collect();
            format!("devices: {}", names.join("; "))
        }
        Availability::NoDevice => "devices: none".to_string(),
        Availability::NoDriver(why) => format!("devices: none, {why}"),
    }
}

/// One device under whichever path reaches it.
///
/// Boxed because the two are far apart in size and one engine is held per
/// worker thread, not per candidate: the indirection costs a pointer hop once
/// per launch, which is a few milliseconds long.
pub enum Engine {
    Vendor(Box<cuda::Engine>),
    Portable(Box<portable::Engine>),
    OpenCl(Box<opencl::Engine>),
}

/// How many survivors one launch must have room for.
///
/// The bitmap lets roughly one candidate in `2^bits` through, so the expected
/// count is the batch divided by that. The room is a multiple of it because
/// the arrivals are random and a launch that runs over would have to lose
/// some, which is the one outcome a search must not have. The floor covers a
/// wide index where the expectation rounds to nothing; the ceiling keeps the
/// buffer that comes back every launch from growing without bound.
pub fn hit_room(batch: u64, bitmap_bits: u32) -> u32 {
    let expected = batch >> bitmap_bits.min(63);
    expected
        .saturating_mul(4)
        .saturating_add(1024)
        .clamp(4096, 1 << 18) as u32
}

/// Batch sizes the automatic choice walks down, largest first.
///
/// Each chain in flight costs 120 bytes of scratch per round, so the three
/// working buffers come to `threads * rounds * 120`: 1.0 GiB at 2^17, 4.0 GiB
/// at 2^19.
///
/// On a discrete card a large batch keeps paying — an RTX 4060 runs at 818 M
/// candidates/s at 2^17 and 836 at 2^19 through the vendor path, 919 and 960
/// through the portable one. On an integrated part the memory is the machine's
/// and a larger batch stops helping sooner: an M1 Pro measured 107 M/s at 2^17
/// and 95 at 2^18, so that list starts lower.
///
/// The walk-down replaces asking how much memory is free: `wgpu` reports no
/// device memory at all.
fn batch_choices(discrete: bool) -> &'static [u32] {
    if discrete {
        &[
            1 << 19,
            1 << 18,
            1 << 17,
            1 << 16,
            1 << 15,
            1 << 14,
            1 << 12,
        ]
    } else {
        &[1 << 17, 1 << 16, 1 << 15, 1 << 14, 1 << 12]
    }
}

impl Engine {
    /// `threads` of zero means: take the largest batch this device accepts.
    pub fn new(
        device: &Device,
        start: &Point,
        threads: u32,
        bitmap: &[u64],
        bitmap_bits: u32,
        retune: bool,
    ) -> Result<Self, String> {
        if threads != 0 {
            return Self::sized(device, start, threads, bitmap, bitmap_bits);
        }
        // OpenCL settles its own shape, because it also varies how many
        // candidates one work item produces. Every other path varies only the
        // batch, and is timed here.
        if matches!(device.api, Api::OpenCl) {
            return opencl::Engine::new(device, start, 0, bitmap, bitmap_bits, retune)
                .map(|e| Engine::OpenCl(Box::new(e)));
        }
        // What this device settled on last time, where it was recorded. The
        // shape depends on the card and the driver, neither of which changes
        // between runs, so measuring every start is a second spent on an
        // answer already known.
        let key = crate::tuning::device_key(&device.name, device.api.name(), 0);
        if !retune {
            if let Some(shape) = crate::tuning::get(&key) {
                if let Ok(engine) = Self::sized(device, start, shape.first, bitmap, bitmap_bits) {
                    return Ok(engine);
                }
                // The remembered shape no longer builds — a smaller card, less
                // free memory. Fall through and measure.
            }
        }

        // Timed rather than guessed. Taking the largest batch that fits was
        // measured slower than the best one on both kinds of device: an M1 Pro
        // through Metal reaches 266 M/s at 65536 chains against 253 at 131072,
        // and the gap is wider on a discrete card.
        let mut best: Option<(f64, Self)> = None;
        let mut last = String::new();
        for want in batch_choices(device.discrete).iter().copied() {
            let mut engine = match Self::sized(device, start, want, bitmap, bitmap_bits) {
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
                    first: engine.threads(),
                    second: 0,
                    rate: *rate,
                    at: crate::tuning::now(),
                },
            );
        }
        best.map(|(_, e)| e)
            .ok_or(format!("no batch size this device would take: {last}"))
    }

    /// Times one launch, in candidates a second.
    ///
    /// One launch is enough to rank batch sizes: the kernel does the same work
    /// every time, and the differences are tens of percent rather than the few
    /// a single sample is uncertain by.
    fn time_one_launch(&mut self) -> Option<f64> {
        // The first launch pays for the tables reaching the device, so it is
        // run and thrown away.
        self.advance().ok()?;
        let at = std::time::Instant::now();
        self.advance().ok()?;
        let seconds = at.elapsed().as_secs_f64();
        (seconds > 0.0).then(|| self.batch() as f64 / seconds)
    }

    fn sized(
        device: &Device,
        start: &Point,
        threads: u32,
        bitmap: &[u64],
        bitmap_bits: u32,
    ) -> Result<Self, String> {
        match device.api {
            Api::Cuda => cuda::Engine::new(device.ordinal, start, threads, bitmap, bitmap_bits)
                .map(|e| Engine::Vendor(Box::new(e)))
                .map_err(|e| e.to_string()),
            // `false`: this is the sized path, where the caller named the
            // shape, so there is nothing to look up or measure.
            Api::OpenCl => opencl::Engine::new(device, start, threads, bitmap, bitmap_bits, false)
                .map(|e| Engine::OpenCl(Box::new(e))),
            _ => portable::Engine::new(device, start, threads, bitmap, bitmap_bits)
                .map(|e| Engine::Portable(Box::new(e))),
        }
    }

    /// How many chains this engine settled on, which the automatic choice may
    /// have lowered.
    pub fn threads(&self) -> u32 {
        match self {
            Engine::Vendor(e) => e.threads(),
            Engine::Portable(e) => e.threads(),
            Engine::OpenCl(e) => e.threads(),
        }
    }

    /// How many candidates one call to [`Engine::advance`] examines.
    pub fn batch(&self) -> u64 {
        match self {
            Engine::Vendor(e) => e.batch(),
            Engine::Portable(e) => e.batch(),
            Engine::OpenCl(e) => e.batch(),
        }
    }

    /// Advances every chain and returns what the bitmap did not rule out.
    pub fn advance(&mut self) -> Result<Vec<Survivor>, String> {
        match self {
            Engine::Vendor(e) => e.advance(),
            Engine::Portable(e) => e.advance(),
            Engine::OpenCl(e) => e.advance(),
        }
    }
}

/// What `--compute` means for the device paths.
///
/// `cpu` is absent from this list on purpose: it is the answer "no device",
/// which the caller handles before asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Choice {
    pub want: Want,
    /// Whether a run without a device is a failure to explain rather than an
    /// ordinary outcome.
    pub required: bool,
}

impl Choice {
    /// `None` when the value is not a device path.
    pub fn parse(value: &str) -> Option<Choice> {
        let want = match value {
            "auto" | "gpu" => Want::Any,
            "portable" => Want::Portable(None),
            "vulkan" => Want::Portable(Some(Api::Vulkan)),
            "metal" => Want::Portable(Some(Api::Metal)),
            "dx12" => Want::Portable(Some(Api::Dx12)),
            "vendor" | "cuda" => Want::Vendor,
            "opencl" => Want::OpenCl,
            _ => return None,
        };
        Some(Choice {
            want,
            required: value != "auto",
        })
    }
}

/// Every value `--compute` accepts, for the error message that lists them.
pub const COMPUTE_VALUES: &[&str] = &[
    "auto", "cpu", "gpu", "portable", "vendor", "cuda", "opencl", "vulkan", "metal", "dx12",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_room_covers_the_expected_rate_with_margin() {
        // The shapes a real run produces: a batch of a few million against an
        // index between 8 and 30 bits wide.
        for bits in 8..=30u32 {
            for threads in [1u64 << 12, 1 << 16, 1 << 17] {
                let batch = threads * 64;
                let expected = batch >> bits;
                let room = u64::from(hit_room(batch, bits));
                assert!(
                    room >= expected * 4,
                    "{bits} bits, batch {batch}: room {room} is not four times {expected}"
                );
            }
        }
    }

    #[test]
    fn hit_room_stays_inside_what_can_be_copied_back() {
        // The hit list comes back every launch, so its size is a cost paid at
        // launch rate and not only when there are hits.
        let widest = hit_room(u64::MAX, 0);
        assert!(
            u64::from(widest) * 40 <= 16 * 1024 * 1024,
            "a {widest}-record buffer crosses the link every launch"
        );
    }

    #[test]
    fn a_wide_index_still_leaves_room() {
        // Nothing is expected through a 30-bit index, but "nothing expected"
        // is not "nothing arrives".
        assert!(hit_room(1 << 22, 30) >= 1024);
    }

    #[test]
    fn a_shared_memory_device_asks_for_less() {
        let big = batch_choices(true)[0];
        let small = batch_choices(false)[0];
        assert!(
            small < big,
            "an integrated part must not open with a discrete card's batch"
        );
        // Both lists have to end somewhere a small device can meet.
        for discrete in [true, false] {
            let last = *batch_choices(discrete).last().expect("a non-empty list");
            assert!(last <= 1 << 12);
            let mut previous = u32::MAX;
            for &v in batch_choices(discrete) {
                assert!(v < previous, "the walk has to go down");
                previous = v;
            }
        }
    }

    #[test]
    fn compute_values_all_parse() {
        for value in COMPUTE_VALUES {
            if *value == "cpu" {
                assert!(Choice::parse(value).is_none(), "cpu names no device path");
            } else {
                assert!(Choice::parse(value).is_some(), "{value} must be accepted");
            }
        }
        // Something that names no path at all, and is not simply a path we do
        // not have: `rocm` was decided against, and `nonsense` never existed.
        assert!(Choice::parse("rocm").is_none());
        assert!(Choice::parse("nonsense").is_none());
    }

    #[test]
    fn only_auto_leaves_a_missing_device_unremarked() {
        assert!(!Choice::parse("auto").expect("auto is a path").required);
        for value in [
            "gpu", "portable", "vendor", "cuda", "opencl", "vulkan", "metal",
        ] {
            assert!(
                Choice::parse(value).expect("a path").required,
                "{value} was asked for, so its absence has to be said"
            );
        }
    }

    #[test]
    fn one_card_seen_twice_is_listed_once() {
        let via = |api: Api| Device {
            index: 0,
            api,
            ordinal: 0,
            name: "NVIDIA GeForce RTX 4060".to_string(),
            memory: 0,
            discrete: true,
            also_via: None,
        };
        assert!(same_card(&via(Api::Vulkan), &via(Api::Cuda)));
        // The two drivers do not agree on spacing or capitalisation.
        let spelt = Device {
            name: "NVIDIA  GeForce(R) rtx 4060".to_string(),
            ..via(Api::Cuda)
        };
        assert!(same_card(&via(Api::Vulkan), &spelt));
        let other = Device {
            name: "NVIDIA GeForce RTX 4070".to_string(),
            ..via(Api::Cuda)
        };
        assert!(!same_card(&via(Api::Vulkan), &other));
    }
}
