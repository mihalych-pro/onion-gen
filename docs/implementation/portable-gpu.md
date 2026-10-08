# The portable device path

**English** | [Русский](portable-gpu.ru.md)

This document describes the device path that works without a vendor toolkit. It
runs through `wgpu`, which reaches Vulkan, Metal, DirectX 12 and OpenGL from
one kernel in WGSL.

The [README](../../README.md) gives the measured speeds. The vendor path has a
document of its own, [cuda-path.md](cuda-path.md).

## 1. The path is faster than the vendor path on some cards

The portable path is not a fallback for the fast one. On the cards tested here
it is close to the vendor path, and on some of them ahead of it.

The program therefore chooses by measurement and not by the order in which it
asks. A card that several paths reach keeps the fastest path for that card:
CUDA on NVIDIA, Metal on Apple, Vulkan elsewhere. The program counts such a
card once, because two engines against one device would fight each other.

## 2. WGSL needs 64-bit integers, and not every backend has them

A field multiply accumulates products of 32-bit limbs in 64 bits, and base WGSL
has no 64-bit integer. The `SHADER_INT64` feature is optional, so where it
exists decides where this path runs.

| Backend | Device | `SHADER_INT64` |
|---|---|---|
| Vulkan | RTX 4060 | yes |
| Metal | Apple M1 Pro | yes |
| DX12 | RTX 4060 | no |
| OpenGL | RTX 4060 | no |

Vulkan and Metal cover everything that this path exists for: AMD, Intel, Apple
Silicon and the same NVIDIA. DX12 and OpenGL sit behind Vulkan on the same
card. A device without 64-bit integers is therefore reported as found and
unusable, and not served several times slower.

A path built without 64-bit integers would be slower again than one that
assembles the widening multiply by hand, because it must emulate the
accumulation as well. There is nothing to build it for while Vulkan is on the
same hardware.

## 3. Every limb index is a literal

An array that a shader indexes with a loop variable cannot live in registers.
The whole working set then goes to off-chip private memory, and the kernel
becomes many times slower.

Every index into a limb array is therefore a literal, and the rows of the
schoolbook form are written out. This is a requirement of the hardware and not
a matter of taste. Nothing in the source shows the mistake if it returns: only
a measurement does.

## 4. The field: radix 2^32 in eight saturated limbs

A field element is eight 32-bit limbs, least significant first, held below
2^256 and folded with `2^256 = 38`.

Eight saturated limbs need sixty-four partial products, where ten redundant
limbs at radix 2^25.5 need a hundred. An element is also 32 bytes instead of
40. This kernel is short of both multiplies and bandwidth, so the saturated
form is cheaper on each count.

The processor path uses the same representation, so a candidate can be compared
limb for limb across every path.

## 5. The paired scheme

Every device path reads candidates in pairs instead of walking a chain. From a
base point in extended coordinates and a table of affine offsets
`Q(m) = 8m·G`, three multiplications give two candidates, because `Z` cancels
out of the numerator and the denominator:

```text
y(P+Q) = (T - x2y2*Z) / (X*y2 - Y*x2)
y(P-Q) = (T + x2y2*Z) / (X*y2 + Y*x2)
```

The base point therefore moves once for each launch, instead of once for each
candidate, and it never has to be made affine. The table is shared and
read-only, so a warp asks for one address at a time and the cache answers once.

## 6. The kernel moves as little as it can

Two planes of traffic are gone, and this matters more than the arithmetic on
the devices that are bound by memory:

- **The Montgomery scratch plane.** The kernel folds the running product of the
  denominators into the numerators as it makes them, so the backward pass has
  everything it needs from the two planes that it already reads.
- **The separate forward pass.** The folding happens during generation, so the
  kernel walks its candidates twice and not three times.

That is 160 bytes of traffic for each candidate instead of 280.

## 7. The window: 193 candidates for each invocation

One point addition and one inversion are paid for each invocation in each
launch, so a wider window spreads that cost further.

Each invocation owns `SLOTS` candidates, which is `2 · HALF + 1` with
`HALF = 96`, so 193. The curve is flat past that point, and a wider window only
costs memory.

## 8. The batch size comes from a timed launch

The program times one launch at each candidate size and keeps the fastest. It
does not compute the size from device properties, and it does not take the
largest that fits.

The reason is that the largest batch is often not the fastest. The three
working buffers grow linearly with the batch, and a device that is bound by
memory slows down long before it runs out. `--device-threads` sets the size by
hand, and the diagnostics say what the program settled on.

A launch also has a fixed cost of some tens or hundreds of microseconds, which
puts a floor under the batch. A batch that lasts milliseconds covers it.

## 9. The hit list is sized from what the index lets through

The kernel writes survivors into a list of fixed size. A narrow index and a
wide batch can produce far more survivors than a small list holds.

The program sizes the list from the rate that the index passes, at four times
the expectation, with a floor and a ceiling. An overflow is an error with an
explanation. It must never be a quiet `min(found, limit)`, which would silently
find fewer.

## 10. The host re-derives every find

A device reports **where** it found something, not what. The host derives the
key from that position, and it refuses the find unless the result is the key
that the device reported.

Without that check, a kernel whose position bookkeeping is wrong writes
addresses that match nothing, and it does so silently.

## 11. Correctness

One kernel goes through three different compilers, so each backend is checked
separately.

- **Field multiply and inversion** against `fiat-crypto`, on a million random
  inputs and on the edge inputs, on Metal and on Vulkan. The comparison is of
  the canonical 32-byte encoding and not of limbs, because that encoding is
  what the address is built from.
- **The whole chain**: 6144 candidates across three consecutive launches,
  compared with the point that the host computes. Three launches and not one,
  because each hands state to the next, and that is where a resume breaks.
- **The engine in the product**: 124 survivors out of 4 194 304 candidates, and
  the processor computes the key again from the offset alone for every one of
  them. Run `cargo bench --bench gpu-chain`. The counts on Metal and on Vulkan
  agree with each other, which is what one kernel on two compilers must
  produce.
- **Keys** found on both backends pass the independent verifier, and keys from
  each path pass tor with `DisableNetwork 1`, which derives the same address.
  The network stays off. What is checked is that the key is well formed, not
  that a service can be published.

## 12. Which wall each device is against

The two devices here are limited by different things, and that is arithmetic
rather than guesswork. The traffic for each candidate is the same on every
path, so the share of each device's peak bandwidth follows from the rate.

The RTX 4060 uses most of its bandwidth and is therefore bound by memory. The
M1 Pro uses a small part of its own and is bound by arithmetic.

This is why one change can help one device and do nothing for the other. The
paired scheme gives most to a device short of arithmetic. The folded traffic
gives most to a device short of bandwidth.

## 13. What does not help

**A register cap on the vendor kernel.** Every value is worse than none.
Occupancy rises, and the kernel starts to spill to local memory, and the spill
costs more than the occupancy gains. Even a cap equal to the register count
already in use loses speed, because the directive itself changes what the
scheduler does.

**Building the PTX for the exact architecture.** The driver recompiles the PTX
for the card in any case. A CI dependency on `rust-src` and a nightly rebuild
of `core` buys nothing.

## 14. What is not here

- **AMD is not tested on hardware.** The route is the same, and Mesa RADV
  advertises `shaderInt64`, but there is no AMD card here. The README says "not
  tested" and not "works".
- **DX12 and OpenGL are not supported**, for the reason in section 2.
- **ROCm and HIP are not built.** `wgpu` may have removed the need for them.
- **There is no dedicated squaring.** It would drop about a third of the
  products, but the `+8G` chain holds no squarings. They are only in the
  inversion, one for each batch.

## 15. The vendor path stays

The vendor path costs `cudarc`, a PTX build and a CI job on nightly. It stays
as a fallback, because it is the only path where Vulkan does not answer: a
stripped image with no loader, an old or broken driver, or an environment that
has the vendor stack and not the portable one.
