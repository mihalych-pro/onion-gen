# The OpenCL path

**English** | [Русский](opencl.ru.md)

A third route to a device, for machines where the first two do not answer.

## 1. Three questions asked first

They were asked first, because any negative answer would have made the
measurements pointless.

- **Does it build without a vendor SDK?** Yes. `opencl3` with the `dynamic`
  feature builds on a machine that has no OpenCL headers at all. That is not a
  detail: `hip-sys` once turned out to be the only crate needing an SDK on the
  build machine, and it cost AMD its vendor path.
- **Is it linked or opened at run time?** Opened. The built binary holds no
  reference to OpenCL, and the library is found through `dlopen`.
- **What does the binary do where OpenCL is absent?** It does not fail: it says
  the library did not load and carries on without it. Checked on a machine whose
  `/etc/OpenCL/vendors` is empty.

One detail cost an evening of its own: the `CommandQueue` has to be created with
the 1.2 call rather than the 2.0 one. Apple's OpenCL is 1.2 and does not export
`clCreateCommandQueueWithProperties` at all, so asking for the `CL_VERSION_2_0`
feature would have lost the Mac entirely.

## 2. The criterion and the result

The criterion was written before the work and is not about the neighbouring
paths: **OpenCL is taken only if it beats the processor of the same machine**,
threshold 2x. Where Vulkan or CUDA work, OpenCL is not needed; the only question
is whether it is better than nothing.

The prototype, the chain with no plumbing:

| Machine | processor | OpenCL | ratio |
|---|---:|---:|---:|
| Apple M1 Pro | 51.6 M/s | **119.1 M/s** | **2.3** |
| Intel i9-9900K + RTX 4060 | 67.0 M/s | **589.5 M/s** | **8.8** |

Met on both. **Decision: build it.**

In the product, where the processor threads work alongside the device:

| Machine | processor | OpenCL | for comparison |
|---|---:|---:|---|
| Apple M1 Pro | 40.1 M/s | **142.4 M/s** | Metal 145.9 |
| i9-9900K + RTX 4060 | 67.0 M/s | **673.8 M/s** | Vulkan 957.2 |

## 3. Where it landed among the others

Behind both — exactly what a last resort should be. On the 4060 it reaches 0.70
of Vulkan. On the M1 Pro the gap nearly vanishes, 0.98 of Metal, because Apple's
OpenCL runs on top of Metal: what is measured there is the cost of the layer,
not of OpenCL.

Hence the order in which the paths are asked: portable, vendor, OpenCL. A device
several of them reach is counted once, and the others are named in the
diagnostics:

```
devices: 0: Apple M1 Pro via metal (also reachable by opencl, ...)
```

## 4. The arithmetic

The same as everywhere: radix 2^25.5, ten 32-bit limbs, accumulation in 64, a
seven-multiply step and a dedicated squaring. Unlike WGSL, the 64-bit integer is
part of OpenCL C rather than an optional extension, so there is nothing to check
for before using it — the one place where OpenCL turned out easier than the
portable path.

Limb indices are literals, for the reason they are in WGSL: an array a kernel
indexes with a variable does not live in registers. That mistake cost 5.9x
there, and there is no sense in repeating it.

## 5. Correctness

- **The field against `fiat-crypto`** — multiply and inversion over a million
  random inputs and the edge ones, on Apple OpenCL 1.2 and NVIDIA OpenCL 3.0,
  with no disagreement.
- **The chain** — 6144 candidates across three consecutive launches, compared
  with the point computed on the host. Three launches rather than one because
  state is handed from each to the next.
- **The engine in the product** — 124 survivors out of 4,194,304 candidates,
  every one matching the key the processor recomputes from the offset alone. The
  count is the same one Metal, Vulkan and CUDA give on the same inputs: four
  APIs, one answer.
- **Keys** — those found by this path on both machines passed the independent
  verifier and tor with `DisableNetwork 1`.

## 6. What it cost the others

The processor path is unchanged: 6 alternating pairs, median 1.003.

## 7. What is not here

- **A measurement where OpenCL is actually needed.** It exists on both machines
  with a card, and both have a faster path. So what was measured is the cost of
  the path, not the need for it; the need is taken from a description of the
  environments — a stripped image, an old driver, a machine built for compute
  rather than graphics. That is an assumption, and it is named rather than
  passed off as measured.
- **Splitting the kernel in two.** On the vendor path that was worth 27% and on
  the portable one it hurt. It was not tried here: this is the fallback, and its
  place in the queue does not earn that kind of polish.
