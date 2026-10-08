# The portable device path

**English** | [Русский](portable-gpu.ru.md)

A second way of computing candidates on a device: one WGSL kernel reaching
Vulkan, Metal and DX12 through `wgpu`. It turned out not to be a fallback for
people without an NVIDIA card but the fastest path there is — including on
NVIDIA.

## 1. Throughput

Both machines, one sitting each, the same binary, one ten-symbol filter:

| Machine | processor | device | ratio |
|---|---:|---:|---:|
| Apple M1 Pro, 8 threads | 43,455,693 | **149,690,778** (Metal) | **3.44** |
| Intel i9-9900K, 16 threads + RTX 4060 | 66,985,984 | 836,213,555 (CUDA) | 12.48 |
| same hardware | 66,985,984 | **960,655,360** (Vulkan) | **14.34** |

The processor row on the laptop moves between 43.5M and 51.6M depending on what
else is open — it is a machine in use, not a bench. The device row barely moves
between the same sittings, which is itself the answer: the device is not
competing for the cores.

The criterion, written before the work, was at least 2× the processor of the
same machine. Met on both: 2.98× and 13.01×.

The figures include the processor threads, which keep working alongside the
device; the "device" row is the throughput of the whole run, not of the card
alone.

## 2. The portable path is faster than the vendor path, on the vendor's own card

This is not what we expected. On an RTX 4060 the portable path outruns the
vendor path, measured on the same card on the same day:

| | prototype kernel, 2^19 chains | the whole product |
|---|---:|---:|
| CUDA, as it was | 537.5 M/s | 535.4 M/s |
| CUDA, after the post-mortem (sections 11-13) | 771.7 M/s | 836.2 M/s |
| Vulkan | 875.8 M/s | 960.7 M/s |

The gap was taken apart down to its causes — every one of them in our kernel and
not in CUDA, and all of them fixed (sections 11-13). What is left is 1.14×.

The selection rule follows the measurement rather than the order the paths are
asked in: a card reached more than one way keeps the fastest way for it — CUDA
on NVIDIA, Metal on Apple, Vulkan elsewhere. The same card visible to two paths
is counted once, otherwise two engines would run against it.

## 3. WGSL does have 64-bit integers

A field multiply accumulates products of 32-bit limbs, and base WGSL has no
64-bit integer. The `SHADER_INT64` feature is optional, so where it exists
decides where this path runs.

| Backend | device | `SHADER_INT64` |
|---|---|---|
| Vulkan | RTX 4060 | **yes** |
| Metal | Apple M1 Pro | **yes** |
| DX12 | RTX 4060 | no |
| OpenGL | RTX 4060 | no |

Vulkan and Metal cover everything this path exists for: AMD, Intel, Apple
Silicon and the same NVIDIA. DX12 and OpenGL sit behind Vulkan on the same
card, so a device without 64-bit integers is reported as found and unusable
rather than served three times slower.

## 4. Writing out the limb loops: 5.9×

The first working version of the kernel ran at 17.1 M/s on an M1 Pro — that is
**0.33 of the processor of the same machine**, and the criterion would have
failed.

The arithmetic was not the problem. A field element is an `array<u32, 10>`, and
the loops over limbs indexed it with a loop variable. An array a shader indexes
with anything but a literal cannot live in registers: the whole working set was
going to off-chip private memory. Once every index was a literal, the same
kernel on the same device ran at 100.4 M/s.

| | M1 Pro, Metal |
|---|---:|
| indexed by a loop variable | 17.1 M/s |
| indexed by a literal | **100.4 M/s** |

This is exactly the mistake a comment in the vendor kernel warns about
(in the prototype kernel), where it was caught by a count of local
memory accesses. Here nothing showed it but the measurement.

## 5. The widening multiply is one instruction, not an emulation

Tested by substitution: the same chain, with `u64(a) * u64(b)` assembled by
hand from four 16-bit products.

| Backend | native | assembled | ratio |
|---|---:|---:|---:|
| Metal, M1 Pro | 97.3 M/s | 50.8 M/s | 1.92× |
| Vulkan, RTX 4060 | 832.8 M/s | 218.7 M/s | 3.81× |

So `u64(a) * u64(b)` reaches a single device instruction on both backends, and
the representation is the right one: radix 2^25.5, ten 32-bit limbs,
accumulation in 64 — the same form the processor's vector path and the vendor
kernel use, which is what lets a candidate be compared with either limb for
limb.

It is also the price of a path without 64-bit integers: that one would be
slower still than the assembled variant, because it has to emulate the
accumulation as well. There is nothing to build it for while Vulkan is on the
same hardware.

## 6. What a launch costs, and how long a batch should be

| | empty launch | reading 160 KiB back |
|---|---:|---:|
| Metal, M1 Pro | 255 µs | 283 µs |
| Vulkan, RTX 4060 | 93 µs | 117 µs |

A batch lasting milliseconds covers this, but it puts a floor under the batch:
at 2^14 chains a launch takes 2 ms and the overhead shows.

| chains | Vulkan, RTX 4060 | Metal, M1 Pro |
|---:|---:|---:|
| 2^14 | 521.8 M/s | 94.8 M/s |
| 2^16 | 729.7 M/s | 94.2 M/s |
| 2^17 | 814.6 M/s | — |
| 2^19 | 837.9 M/s | 100.9 M/s |

The curve flattens while the memory grows linearly: the three working buffers
are `chains × rounds × 120` bytes, which is 1.0 GiB at 2^17. The automatic
choice starts at 2^17 and walks down until the device accepts, and says in the
diagnostics what it settled on; `--device-threads` sets it by hand. Walking
down rather than asking how much memory is free is forced by `wgpu`, which
reports no device memory at all.

## 7. Correctness

The defect most easily introduced here is the same one the vendor
path could: the device computes the wrong thing, nothing fails, addresses are
printed, and the keys do not open them. One kernel goes through three different
compilers, so this was checked on each backend separately.

- **Field multiply and inversion** against `fiat-crypto` on a million random
  inputs and on edge ones, on Metal and on Vulkan. The comparison is of the
  canonical 32-byte encoding rather than of limbs: that encoding is what the
  address is built from.
- **The whole chain** — 6144 candidates across three consecutive launches,
  compared with the point computed on the host. Three launches rather than one
  because state is handed from each to the next, and that is where a resume
  breaks.
- **The engine in the product** — 124 survivors out of 4,194,304 candidates,
  every one of them matching the key the processor recomputes from the offset
  alone (`cargo bench --bench gpu-chain`). The counts on Metal and on
  Vulkan agree with each other, which is what one kernel on two compilers has
  to produce.
- **Keys** — eight found on Metal and six on Vulkan passed the independent
  verifier, and two from each path passed tor with `DisableNetwork 1`, which
  derived the same address. The network stays off: what is checked is that the
  key is well formed, not that a service can be published.

## 8. A defect in the vendor path, found on the way

On an overflowing hit list the vendor path took `min(found, limit)` and said
nothing about it — that is, it **silently found fewer**. It showed up with a
narrow index and a wide batch: on a six-symbol filter with `--index-bits 8` the
index lets one candidate in 256 through, and a batch of 8.4 M produces 33,000
survivors where there was room for 4096.

The list is now sized from the rate the index lets through — four times the
expectation, with a floor and a ceiling — and an overflow is an error with an
explanation rather than a loss. Both paths now run that case to the end.

## 9. What is not here

- **AMD is not tested on hardware.** The route to it is the same — Vulkan, and
  Mesa RADV advertises `shaderInt64` — but we have no AMD card, and the README
  says "not tested" rather than "works".
- **DX12 and OpenGL are not supported.** The reason is in section 3: no 64-bit
  integers, and Vulkan is on the same hardware.
- **ROCm/HIP and OpenCL were not built.** They are the next links of the chain
  from D11, and `wgpu` may have removed the need for the first.
- **There is no dedicated squaring.** It would drop about a third of the
  products, but the `+8G` chain contains no squarings: they are only in the
  inversion, one per batch.

## 10. Which wall each path is against

The same question — why so little on Metal, and why less on CUDA than on Vulkan
— has different answers on the two devices, and they are arithmetic rather than
guesswork.

The traffic per candidate is the same on every path: thirty words written (`z`,
`y` and the running product the batch inversion needs) and thirty read, so 240
bytes. That gives the share of each device's peak bandwidth:

| Path | steps only | whole chain | ratio | GB/s | share of peak |
|---|---:|---:|---:|---:|---:|
| Vulkan, 4060 | 1755 M/s | 838 M/s | 0.48 | 201 | **74%** |
| CUDA, 4060, as it was | 1458 M/s | 537 M/s | 0.37 | 129 | 47% |
| Metal, M1 Pro | 217 M/s | 100 M/s | 0.46 | 24 | **12%** |

**On the 4060 the portable path is against the memory wall.** 74% of peak is
around 88% of what is achievable in practice; speeding up the arithmetic there
buys almost nothing, and cutting traffic is the only lever left — and the
current algorithm already writes the minimum a batch inversion needs.

**On the M1 Pro nothing but the arithmetic is binding.** 12% of the bandwidth
says memory is not it, and counting multiplies closes the gap almost exactly:
the step is 8 field multiplies, the whole chain is 15.1 (8 in the step, 3 in the
backward pass, 265/64 for the inversion). 217 × 8 / 15.1 = 115 M/s against 100
measured. "Metal is slow" is not a mystery but a modest 32-bit multiply
throughput.

Hence the practical consequence: **the same change moves these two devices in
opposite directions**, which is not an anomaly but what different walls mean.
Section 12.

## 11. Three reasons the vendor path lagged

The gap was in our kernel, not in CUDA. All three were found by reading the PTX
rather than by guessing, and each was measured on its own, on one card, on one
day.

**The group step was not inlined.** `curve::step` carried `#[inline]`, which for
this backend is a hint, and the hint was declined. At the PTX level a call is a
real call with its arguments passed through local memory: `chain_resume` carried
a 360-byte stack frame and 268 off-chip accesses, for a point that belongs in
registers. With `#[inline(always)]` the chain without the inversion went from
1458 to 1742 M/s — **level with Vulkan's 1755**. That is the proof that the
group arithmetic was never what was wrong.

**The key was packed one bit at a time.** `to_bytes` assembled 32 bytes into a
`[u8; 32]` indexed by a running counter: 255 iterations per candidate, each
writing a single bit, and the array lived in local memory for the same reason.
Eight word assignments instead of the loop: 552 → 608 M/s. This is word for word
the mistake that cost 5.9× in WGSL (section 4), noticed months later in another
file.

**The inline assembly turned into a cost.** `mad.wide.u32` was adopted in
`gpu-backend` as an optimisation and did measure as a win then — but it was
measured in a kernel that was spilling anyway. Once the spilling was gone the
assembly got in the register allocator's way: without it, 608 → 689 M/s, even
though a product now costs two instructions instead of one. Occupancy turned out
to be worth more than instructions.

These three together: 535 → 688 M/s in the product, **plus 28%**; sections 12
and 13 added another 28%. Local accesses in `chain_resume` are down from 268 to 40, and CI now
watches that number, because putting it back costs nothing.

## 12. Two optimisations that read differently depending on which kernel you measure

Both cut arithmetic, both are correct, both were checked differentially.

**A seven-multiply step.** The constant step point is normalised on the host so
that its `Z` is one; the product `Z1 · Z2` in the addition formula then becomes
`Z1`. The point stays the same projectively, and the price is one inversion per
run.

**A dedicated squaring.** The symmetric products are paired: 55 instead of 100.
The inversion is 254 squarings and 11 multiplies, so almost all of its cost is
here.

On the portable path both win, and these are alternating paired measurements:
Metal 6 pairs, median 1.088, five wins out of six; Vulkan 4 pairs, median 1.030.
The difference between them is exactly what section 10 predicts: Metal is
against the arithmetic and gets nearly all of it, Vulkan is against the memory
and gets little.

**On the vendor path the first conclusion was wrong, and here is why.** Both
were first measured with the prototype's `chain` kernel, which said the
seven-multiply step gave 689 → 465 and the squaring 689 → 471. But `chain`
writes all 32 bytes of every candidate to memory, while the product launches
`chain_resume`, which sends across the bus only what passed the bitmap. That is
a different memory profile altogether, and a conclusion does not carry from one
kernel to the other.

On `chain_resume`, which is the kernel that actually runs (2^19 chains, 64
rounds, no register cap):

| What was built | one kernel | two kernels (section 13) |
|---|---:|---:|
| eight multiplies, squaring is a multiply | 565.5 M/s | 578.0 M/s |
| eight multiplies, dedicated squaring | 583.7 M/s | 634.1 M/s |
| seven multiplies, dedicated squaring | 535.9 M/s | **681.8 M/s** |

So the seven-multiply step does hurt a fused kernel — not because the arithmetic
is worse but because `p.z` stays live longer, registers go from 168 to 200 and
occupancy falls from 12 warps to 10. Once the kernel is split, the same step is
the best of the three.

The lesson is worth more than the numbers: **a performance conclusion holds only
for the kernel it was measured on**, and a prototype will happily measure the
one next to it.

## 13. Splitting the kernel in two

A kernel is allocated registers for the worst of its parts. The first half — the
`+8G` chain — wants every register it can hold. The second — the batch inversion
and the address check — reads three planes per candidate and wants resident
warps rather than registers, because what limits it is memory latency.

Fused, the second half runs at the first half's occupancy. On this card that is
12 resident warps out of 48, or 25%:

| Kernel | registers | warps of 48 |
|---|---:|---:|
| `chain_resume`, fused | 200 | 10 |
| `chain_steps` | 208 | 9 |
| `chain_reduce` | 96 | 21 |

The halves talk through the buffers they already used, so the split costs one
extra launch per batch and not one byte of extra traffic. It returns **1.27×** on
the prototype.

The split also overturned one more earlier conclusion. `invert` had been left
un-inlined because, in the fused kernel, `inline(always)` measured as a loss. In
the split kernel it lives only in the second half, and inlining it removes the
last ABI boundary: `chain_reduce` goes from 96 registers to 80, from 21 resident
warps to 25, and the chain from 681.8 to **771.7 M/s**.

One more step came from the batch. On a discrete card a larger batch keeps
paying (in the product, 818 M/s at 2^17 chains against 836 at 2^19 on the vendor
path, 919 and 960 on the portable one), and on an integrated part it stops long
before (M1 Pro: 107 M/s at 2^17 and 95 at 2^18). So the automatic choice now
opens at 2^19 chains where the device has memory of its own and at 2^17 where it
shares the machine's. All told, 688 → 836 M/s in the product.

**The portable path does not need the split and is hurt by it:** Vulkan 0.89,
Metal 0.92, alternating pairs. That is the answer to the original question — the
split does by hand what the SPIR-V compiler was already doing, which is why the
portable path was faster in the first place.

## 14. What else was tried and rejected

**A register cap** (`.maxnreg` from 64 to 168). Every value is worse than none:
occupancy rises, but the kernel starts spilling to local memory and the spill
costs more. At a cap of 64 that is 32 warps and 261 M/s against 713 uncapped.
Curiously, even a cap equal to the register count already in use loses a quarter
of the speed: the directive itself changes what the scheduler does.

**Building for the actual architecture.** The PTX is generated for `sm_70` while
the card is `sm_89`. Building with `-C target-cpu=sm_89` (which needs
`-Z build-std=core`) gave 683 against 692 — nothing, because the driver
recompiles the PTX for the card anyway. A CI dependency on `rust-src` and a
nightly rebuild of `core` is not worth that.

## 15. The vendor path stays

Since the portable path is faster on the same card, whether to keep the vendor
one was a question worth asking. **Decided on 2026-09-23: keep it, as a
fallback.**

It costs `cudarc`, 1.4 MB of PTX in the repository and a CI job on nightly. In
return it is the only path where Vulkan does not answer: a stripped image with
no loader, an old or broken driver, an environment that has the vendor stack and
not the portable one. Being 1.15 times behind is the price paid by someone who
would otherwise not be counting at all.

## 16. Where this stopped

| Path | before | after | ratio |
|---|---:|---:|---:|
| vendor, CUDA | 535.4 M/s | **836.2 M/s** | 1.56 |
| portable, Vulkan | 858.8 M/s | **960.7 M/s** | 1.12 |

The gap between the two paths on one card is down from 1.60 to **1.15**. The
processor of the same machine gives 67.0 M/s, so the device is 12.5 and 14.3
times faster than it.

What is left unexamined is the traffic. Both chains write 30 words per candidate
and read 30 — the minimum for a batch inversion in its present shape, though it
is not proven that no other shape exists. On the 4060 that is the only lever
left, because 74% of the bandwidth is already in use; on the M1 Pro there is no
point in it at all.

## The paired scheme on the device

Every device path reads candidates in pairs rather than walking a chain. From a
base point held in extended coordinates and a table of affine offsets
`Q(m) = 8m*G`, three multiplications give two candidates, because `Z` cancels
out of both the numerator and the denominator:

```text
y(P+Q) = (T - x2y2*Z) / (X*y2 - Y*x2)
y(P-Q) = (T + x2y2*Z) / (X*y2 + Y*x2)
```

The base point therefore moves once per launch instead of once per candidate,
and never has to be made affine. The table is shared and read-only, so a warp
asks for one address at a time and the cache answers once.

Measured against the chain it replaced, device only, slices in rotation on the
same card, counting the keys each one wrote:

| Path | chain | paired | |
|---|---:|---:|---:|
| Metal, M1 Pro | 115.8 M/s | **138.2 M/s** | 1.19x |
| OpenCL, M1 Pro | 115.8 M/s | **126.5 M/s** | 1.09x |
| CUDA, RTX 4060 | 688.9 M/s | **800.1 M/s** | 1.16x |
| Vulkan, RTX 4060 | 749.1 M/s | **789.4 M/s** | 1.05x |

The gain is smaller than the 2x the same change bought on the processor: the
arithmetic per candidate falls by about the same factor, but these kernels write
two planes and read three per candidate, and that traffic did not change.

### What the host checks

A device reports **where** it found something, not what. The host derives the
key from that position and now refuses it unless the result is the key the
device reported. Without that check a kernel whose position bookkeeping is wrong
writes addresses that match nothing, which is exactly what one of these three
did while it was being written, silently.

### Cutting the memory the kernel touches

The paired scheme bought nothing on the paths that were already the fastest on
their device — OpenCL on an M1 Pro, Vulkan on a 4060 — because those are not
short of arithmetic but of memory bandwidth. Two planes went away:

* **The Montgomery scratch plane.** The running product of the denominators is
  folded into the numerators as they are produced, so the backward pass has
  everything it needs from the two planes it already reads.
* **The separate forward pass.** Folding happens during generation, so the
  kernel walks its candidates twice rather than three times.

Per candidate that is 160 bytes of traffic instead of 280. Measured, device
only, with a filter long enough that no hit lands:

| Path | chain | paired | folded |
|---|---:|---:|---:|
| Metal, M1 Pro | 109.1 | 135.5 | **174.7** |
| OpenCL, M1 Pro | 120.8 | 121.0 | **188.3** |
| CUDA, RTX 4060 | 765.0 | 858.8 | **1291.6** |
| Vulkan, RTX 4060 | 892.5 | 892.9 | **1397.2** |

The two paths that gained nothing from the pairs gained the most from the
traffic, which is what being bound by memory rather than arithmetic looks like.

### A compact field, and a wider window

Two more things came out of reading `prefix32`, which is roughly twice as fast
as the chain kernel was on the same card.

**The field moved to eight saturated 32-bit limbs.** Radix 2^25.5 over ten
redundant limbs avoids carries but needs a hundred partial products; eight
saturated ones need sixty-four, and an element is 32 bytes instead of 40. The
kernel is short of both multiplies and bandwidth, so it wins twice. This is the
same representation the processor path uses, so a candidate can be compared limb
for limb across all four.

**Each invocation now owns 193 candidates rather than 65.** One point addition
and one inversion are paid per invocation per launch, so a wider window spreads
them further. Measured on Metal, device only:

| candidates per invocation | 17 | 33 | 65 | 129 | 193 | 257 |
|---|---:|---:|---:|---:|---:|---:|
| M/s | 95.6 | 147.9 | 200.2 | 233.3 | **250.4** | 249.3 |

Where this leaves the four paths, against the chain they started from:

| Path | chain | now | |
|---|---:|---:|---:|
| Metal, M1 Pro | 109.1 | **250.4** | 2.30x |
| OpenCL, M1 Pro | 120.8 | **225.1** | 1.86x |
| CUDA, RTX 4060 | 765.0 | **1861.9** | 2.43x |
| Vulkan, RTX 4060 | 892.5 | **1791.0** | 2.01x |

On the 4060 that is ahead of `prefix32`, which measures 1815 M/s there.
