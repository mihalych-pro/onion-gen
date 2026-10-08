# The CUDA path

**English** | [Русский](cuda-path.ru.md)

This document describes the device as a second source of candidates, through
CUDA on cards of NVIDIA.

The [README](../../README.md) gives the measured speeds. This document gives the
method and the limits.

## 1. The criterion

We set the bar before we had a machine to try it on. A kernel that runs the
`+8G` chain with batch inversion had to reach 140 million candidates a second on
one device.

The kernel passed that bar by a wide margin on an NVIDIA RTX 4060. The device is
also much faster than the processor of the same machine.

## 2. The build needs no vendor toolkit

This was the property most at risk, and it survived.

The kernel is written in Rust and compiles to PTX through the
`nvptx64-nvidia-cuda` target. The host resolves the driver at run time and does
not link it. Neither step needs CUDA on the machine. We build the whole program
on `darwin/arm64`, which has no vendor driver, and ship a binary.

The kernel needs a nightly toolchain. Both the `ptx-kernel` ABI and inline
assembly for this architecture are still unstable on Rust 1.99, which we checked
and did not assume. The kernel is therefore a crate of its own, and the program
stays on stable.

The build script of the main crate does one of two things. It compiles the
kernel and takes the PTX through `OUT_DIR`. Or it takes a PTX that already
exists, through the `ONION_GEN_KERNEL_PTX` variable. The second form lets the
pipelines compile the kernel once and give the same file to every job below. The
compiled PTX is not in the repository, because a copy there could only be the
stale one.

## 3. What the arithmetic cost to write well

The backend emits a 32x32 into 64 multiply when a value feeds one product. It
falls back to an emulated 64-bit multiply when the value feeds ten products.
That is the exact shape of a ten-limb schoolbook product. A five-line probe
confirmed both halves of this.

The accumulation is therefore written as inline PTX. That gives 100 single
instructions in place of 92 emulated ones, and it is much faster.

Portability loses nothing here. The kernel compiles to PTX, so it belongs to one
vendor whatever language it is written in. Other vendors go through the portable
path, which is written separately. The portable version stays as the reference
that we check the fast one against. The processor path uses the same
arrangement.

## 4. The split between device and host

The device rules candidates out. It never rules them in.

| Structure | On the device | Reason |
|---|---|---|
| prefix bitmap | yes | a flat array of words |
| suffix and pattern bitmaps | possible | the same structure |
| substring automaton | no | a pointer structure |
| the filters themselves | no | strings and vectors |

A filter set that builds no prefix bitmap therefore gets no device. A dictionary
of substrings is one such set. The program says so at the start. It does not
promise a device and then go quiet.

We measured the cost of the link by changing how many candidates cross it. The
difference between all of them and none of them is small, and a real search
passes one candidate in millions. The boundary will not become the limit.

## 5. Correctness

An error in device arithmetic gives wrong keys, not a crash. The checks are
therefore the load-bearing part.

- Candidates from the device match the processor byte for byte from the same
  starting point.
- The host derives each survivor again from its offset alone, by scalar
  multiplication, and compares the two. This proves the bookkeeping that turns a
  thread and a step into a position in the chain.
- Keys that the device finds pass the independent verifier and tor with
  `DisableNetwork 1`.

The byte-for-byte check is the one that finds a buffer layout changed in the
kernel and not in the host.

## 6. What is not done

- **Only the prefix bitmap travels to the device.** The suffix and pattern
  bitmaps could travel too. The substring automaton would need a flat form
  first.
- **The device does not write keys.** It reports positions, and the host derives
  the key. This is correct while hits are rare, and it would be wrong if they
  were not.

Other vendors are no longer on this list. Cards of AMD and Intel go through the
portable path and through OpenCL, which both exist. See
[portable-gpu.md](portable-gpu.md) and [opencl.md](opencl.md).

## 7. Acceptance

The acceptance run uses one ten-symbol filter on the machine that holds the
card. It takes the best of three for each path, and it runs both paths in one
sitting.

Both rows come from the same binary, with a flag between them. The comparison is
therefore between two paths and not between two builds.

The device path is in the same binary as the processor path, and it costs the
processor path almost nothing. The cost is a few more instructions for each
candidate, and the instructions-per-cycle figure does not move.
