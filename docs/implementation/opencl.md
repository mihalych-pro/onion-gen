# The OpenCL path

**English** | [Русский](opencl.ru.md)

OpenCL is a third route to a device, for machines where the first two do not
answer.

The [README](../../README.md) gives the measured speeds.

## 1. Three questions asked first

We asked these questions first, because a negative answer to any of them would
have made the measurements pointless.

- **Does it build without a vendor SDK?** Yes. The crate `opencl3` with the
  `dynamic` feature builds on a machine that has no OpenCL headers. This is not
  a detail. The crate `hip-sys` was once the only crate that needed an SDK on
  the build machine, and that cost AMD its vendor path.
- **Does the binary link it or open it at run time?** It opens it. The built
  binary holds no reference to OpenCL, and it finds the library through
  `dlopen`.
- **What does the binary do where OpenCL is absent?** It does not fail. It
  reports that the library did not load, and it continues without it. We checked
  this on a machine with an empty `/etc/OpenCL/vendors`.

One detail decides whether the Mac works at all. The program must create the
`CommandQueue` with the 1.2 call and not the 2.0 call. The OpenCL of Apple is
version 1.2 and does not export `clCreateCommandQueueWithProperties`. A request
for the `CL_VERSION_2_0` feature would therefore have lost the Mac completely.

## 2. The criterion

We wrote the criterion before the work, and it says nothing about the
neighbouring paths. **Take OpenCL only if it is at least twice as fast as the
processor of the same machine.** Where Vulkan or CUDA work, OpenCL is not
needed. The only question is whether OpenCL is better than no device.

Both test machines passed the criterion, first in a prototype that ran the chain
with no plumbing, and then in the product. We therefore built the path.

## 3. Where it sits among the others

OpenCL is behind both other paths, which is correct for a last resort.

On the RTX 4060 it stays behind Vulkan. On the M1 Pro the gap almost disappears,
because the OpenCL of Apple runs on top of Metal. What we measure on the Mac is
therefore the cost of that layer, and not the cost of OpenCL.

The program asks for the paths in this order: CUDA, Metal, Vulkan, DirectX 12,
OpenCL, OpenGL. It counts a device once even when several paths reach it, and it
names the other paths in the diagnostics:

```text
devices: 0: Apple M1 Pro via metal, 10922 MiB (also reachable by opencl; --compute opencl forces it)
```

## 4. The arithmetic

The arithmetic is the same as everywhere else: radix 2^25.5, ten 32-bit limbs,
accumulation in 64 bits, a seven-multiply step, and a separate squaring.

The 64-bit integer is part of OpenCL C and not an optional extension, unlike
WGSL. There is therefore nothing to test for before the kernel uses it. This is
the one place where OpenCL was easier than the portable path.

Limb indices are literals, for the same reason as in WGSL. An array that a
kernel indexes with a variable does not stay in registers.

## 5. Correctness

- **The field against `fiat-crypto`.** Multiply and inversion over a million
  random inputs and the edge inputs, on Apple OpenCL 1.2 and on NVIDIA OpenCL
  3.0. There was no disagreement.
- **The chain.** 6144 candidates across three consecutive launches, compared
  with the point that the host computes. Three launches and not one, because
  each launch hands state to the next.
- **The engine in the product.** 124 survivors out of 4 194 304 candidates. The
  processor computes the key again from the offset alone for each survivor, and
  every key matches. Metal, Vulkan and CUDA give the same count on the same
  inputs: four APIs and one answer.
- **Keys.** Keys that this path found on both machines passed the independent
  verifier and tor with `DisableNetwork 1`.

## 6. What it cost the other paths

The processor path is unchanged. We measured six alternating pairs and found no
difference.

## 7. What is not here

- **A measurement on a machine that needs OpenCL.** Both test machines have a
  card, and both have a faster path to it. We therefore measured the cost of the
  path and not the need for it. The need comes from a description of the
  environments: a stripped image, an old driver, or a machine built for compute
  and not for graphics. That is an assumption, and we name it as one.
- **A kernel in two parts.** That shape helps the vendor path and hurts the
  portable one. This path is the fallback, and its place in the queue does not
  earn that work.
