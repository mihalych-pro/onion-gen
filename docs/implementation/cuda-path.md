# The CUDA path

**English** | [Русский](cuda-path.ru.md)

The graphics device as a second source of candidates, and the answer to the
question the project has carried unanswered since it began.

## 1. The criterion, and what it turned out to be

, set the bar before there
was a machine to try it on: a kernel running the `+8G` chain with batch
inversion had to reach **140 million candidates a second** on one device.

Measured on an NVIDIA RTX 4060:

| | candidates |
|---|---|
| chain with batch inversion | 550 M/s |
| the same without the inversion, for comparison | 1470 M/s |
| the criterion | 140 M/s |

In the product, on the machine that holds the card: **553 M/s with the device
against 77 M/s with its processor alone**, a factor of 7.2. Against the
reference implementation's best figure of about 30 M/s, eighteen.

## 2. Nothing about building this needs a vendor toolkit

This was the property most at risk and it survived intact.

The kernel is written in Rust and compiled to PTX through the
`nvptx64-nvidia-cuda` target. The host resolves the driver at run time rather
than linking it. Neither needs CUDA installed: the whole thing is built on
`darwin/arm64`, which has no vendor driver at all, and shipped as a binary.

The kernel does need a nightly toolchain — both the `ptx-kernel` ABI and inline
assembly for this architecture are still unstable on 1.99, checked rather than
assumed. That is why the kernel is a crate of its own: the program itself stays
on stable. The main crate's build script compiles it on every build and takes
the PTX through `OUT_DIR`, so a kernel and the PTX in a binary cannot drift
apart. The compiled form is not in the repository, because a copy there could
only ever be the stale one. None of this needs a CUDA toolkit; the kernel is
compiled by `rustc` and the driver is opened at run time.

## 3. What the arithmetic cost to write well

The backend emits a 32x32 into 64 multiply when a value feeds one product, and
falls back to an emulated 64-bit multiply when it feeds ten — which is exactly
the shape of a ten-limb schoolbook product. A five-line probe confirmed both
halves of that. Writing the accumulation as inline PTX gives 100 single
instructions instead of 92 emulated ones, and measures **1.67x**:

| | field multiplies |
|---|---|
| portable Rust | 7.60 G/s |
| inline PTX | 12.69 G/s |

Portability loses nothing by this. The kernel compiles to PTX, so it is bound
to one vendor whatever it is written in; other vendors are reached through a
portable path that is written separately. The portable version stays as the
reference the fast one is checked against, the same arrangement the processor
path uses.

## 4. The split between device and host

The device rules candidates out. It never rules them in.

| structure | on the device | why |
|---|---|---|
| prefix bitmap | yes | a flat array of words |
| suffix and pattern bitmaps | could be | the same structure |
| substring automaton | no | a pointer structure |
| the filters themselves | no | strings and vectors |

So a set of filters that builds no prefix bitmap — a dictionary of substrings,
say — gets no device, and the program says so at the start rather than
promising one and going quiet.

What the link costs was measured by varying how much passes:

| candidates passed | throughput |
|---|---|
| all of them | 563.9 M/s |
| one in 256 | 573.8 M/s |
| none | 618.0 M/s |

Nine percent between everything and nothing, and a real search passes one in
millions. The boundary will not become the limit.

## 5. Correctness

An error in device arithmetic shows up as wrong keys, not as a crash, so the
checks are the load-bearing part.

- Candidates from the device match the processor **byte for byte** on the same
  starting point.
- Every survivor was re-derived on the host from its offset alone, by scalar
  multiplication, and compared: this is what proves the bookkeeping that turns
  a thread and a step into a position in the chain.
- Keys found by the device pass the independent verifier and tor with
  `DisableNetwork 1`.

The byte-for-byte check earned its place immediately: it caught a buffer layout
changed in the kernel and not in the host.

## 6. A measurement that lied, and what it cost

The first harness timed the allocation of gigabyte buffers and the copy of
results across the bus along with the kernel. It reported 65 M/s — below the
criterion — and no change to the kernel moved it. Coalescing, occupancy and
register pressure were all suspected and adjusted before the harness itself
was.

The number was the speed of the link. With allocation and transfer moved
outside the timing the same kernel measured 550 M/s.

The rule worth keeping: when a figure refuses to respond to changes that must
affect it, the harness is the first suspect, not the code.

## 7. What is not done

- **Only the vendor path exists.** AMD and Intel are reached through the
  portable path, which is written separately and is not written yet.
- **Only the prefix bitmap travels.** The suffix and pattern bitmaps could,
  and the substring automaton would need flattening first.
- **The device does not write keys.** It reports positions and the host derives
  the key, which is right while hits are rare and would not be if they were not.

## 8. What carrying the device path costs the processor path

Paired against the commit before this path existed, eight pairs, on a machine with no
device: prefix search measures 0.996 and a thousand-filter dictionary 0.995,
with the new build ahead in one pair of eight each time. Small, and consistent
in direction.

This time it is real work rather than code layout, which was checked rather
than assumed: instructions retired per candidate go from about 1620 to about
1630, half a percent, while the instructions-per-cycle figure does not move.
Carrying the device path in the same binary costs the processor path half a
percent, and that is the honest figure.

## 9. Acceptance

On the machine that holds the card, one ten-symbol filter, best of three each,
both paths in one sitting:

| path | candidates |
|---|---|
| processor only | 80.6 M/s |
| processor and device | 585.3 M/s |

A factor of 7.26, from the same binary with a flag between them, so the
comparison is of paths rather than of builds.
