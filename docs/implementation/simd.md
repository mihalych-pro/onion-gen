# Vector field arithmetic

**English** | [Русский](simd.ru.md)

Vectorised field arithmetic, and the three optimisations attempted afterwards —
two of which worked.

## 1. Throughput

Both architectures, every row measured in one sitting against the reference's
fastest build on that machine.

**Apple M1 Pro, NEON**, reference `donna/intfilter-binsearch-besort`:

| Threads | `mkp224o` | our scalar | our NEON | NEON / reference | NEON / scalar |
|---:|---:|---:|---:|---:|---:|
| 1 | 5 887 160 | 5 587 763 | 8 345 600 | **1.42** | 1.49 |
| 2 | 11 293 700 | 11 852 595 | 16 502 374 | **1.46** | 1.39 |
| 4 | 22 266 900 | 22 760 243 | 31 941 018 | **1.43** | 1.40 |
| 6 | 30 469 300 | 31 121 408 | 42 943 693 | **1.41** | 1.38 |
| 8 | 30 528 000 | 32 586 547 | 45 754 778 | **1.50** | 1.40 |

**Intel Xeon E5-2683 v4, AVX2**, reference `amd64-64-24k/intfilter`:

| Threads | `mkp224o` | our scalar | our AVX2 | AVX2 / reference | AVX2 / scalar |
|---:|---:|---:|---:|---:|---:|
| 1 | 3 749 040 | 2 604 237 | 4 176 282 | **1.11** | 1.60 |
| 2 | 7 302 740 | 5 131 469 | 8 031 027 | **1.10** | 1.56 |
| 4 | 14 093 800 | 9 537 536 | 15 468 954 | **1.10** | 1.62 |
| 6 | 20 329 800 | 13 647 462 | 22 963 814 | **1.13** | 1.68 |
| 8 | 26 793 600 | 18 706 432 | 29 355 622 | **1.10** | 1.57 |

The acceptance threshold was 1.3x of our own scalar path; the result is 1.38–1.68x.
The project floor — no slower than the best reference build on the same hardware
— holds on **both** architectures; on the scalar path it held only on
ARM and was missed by a fifth on x86.

Note the asymmetry between the two tables. On ARM our scalar path is already
level with the reference, because the reference's fastest ARM build is portable
C. On x86 the reference's fastest build is hand-written assembly, so our scalar
path trails it by 30% and the vector path is what closes the gap and overtakes.

## 2. Three optimisations, two of which worked

The vector path did not reach these numbers on arrival. Its first working
version ran at 0.95 of the reference on x86, and the arithmetic was not the
problem: per candidate, our vector multiplication already cost less than the
reference's assembly. The overhead around it did not.

**Unrolled byte packing — worked, the largest single win.** The canonical
32-byte encoding was produced by a general bit-writer looping over limbs, about
fifty operations per candidate, and it runs once per candidate. Replacing it
with ref10's straight-line byte assignments took x86 from 25.6M to 27.9M, which
is the step from 0.95 to 1.03 of the reference.

**Skipping the canonical reduction in the hot loop — worked.** A field element
encodes differently from its canonical form only when its value lands in
`[p, 2^255)`: 19 values out of `2^255`, which no run will ever produce. Matching
now reads the unreduced bytes, and the reduction runs only for a candidate that
matched, where the address has to be right. Together with writing the field
addition and subtraction as fixed-size array operations this took x86 from 27.9M
to 29.7M.

**Two interleaved chain sets — did not work, reverted.** The chain and the batch
inversion are both dependency chains, so eight candidates in flight instead of
four should have filled the stalls. Measured no difference on either
architecture.

**Splitting the Montgomery chain in two — did not work, reverted.** Same
reasoning, applied to the inversion. It cost 8%: the extra inversion is about
265 multiplications, and the chain turns out to be limited by throughput rather
than by latency.

Both failures were plausible beforehand. The code carries comments saying what
was tried, so the next person does not spend the same day on it.

One more thing was measured and reverted along the way: the first version of the
deferred reduction kept `y·z⁻¹` in a separate array so a hit could be re-packed.
That array cost a fifth of the throughput in cache pressure. Recomputing `y`
from the stored `z⁻¹` is cheaper than keeping it.

## 3. Field multiplication

Measured on our own code, not on a separate benchmark
(`cargo bench --bench field-bench`):

| Platform | scalar (`fiat-crypto`, radix 2^51) | vector (radix 2^25.5) | gain |
|---|---:|---:|---:|
| M1 Pro, NEON | 11.82 ns | 6.28 ns | **1.88x** |
| Xeon E5-2683 v4, AVX2 | 25.03 ns | 14.29 ns | **1.75x** |

The NEON figure matches the 1.83–1.92x the C microbenchmark in
`scripts/bench/neon/` measured before any of this was written, which is the
point of having both.

### The first measurement was wrong, and how

An early version of `field-bench` reported 1.31x for NEON rather than 1.88x. The
code was fine; the measurement was not. The vector path was timed as a chain of
dependent multiplications — each result feeding the next, so what was measured
was latency — while the scalar path ran four independent chains and was measured
for throughput. Once both ran four independent chains the numbers agreed with
the C benchmark. The benchmark now runs four chains on every path.

## 4. Correctness

The defect most easily introduced here is a vector path that
computes the wrong thing: nothing crashes, addresses are printed, and the keys
behind them do not work.

- **Field operations against the scalar field**: multiplication, squaring,
  addition, subtraction, inversion and canonical packing, each checked lane by
  lane against `fiat-crypto` on random and edge inputs — zero, one, `p-1`, all
  limbs maximal, and unreduced sums.
- **Limb bounds**, which is where a vector implementation goes wrong quietly.
  Multiplication scales one operand by 19 or 38 depending on limb parity and
  feeds it to an instruction reading 32 bits, so the ceilings differ by parity
  and by which operand. A test built from maximal limbs — not random ones, whose
  margins hide a wrong bound — confirms that three stacked additions still fit
  and four do not.
- **The whole engine**: 122 880 candidates generated by both engines from the
  same seed and compared byte for byte, plus the same comparison at the address
  level (`tests/engine_equivalence.rs`). This is what catches a lane mix-up,
  where every candidate is a valid key but attached to the wrong counter offset.
- **Both architectures natively.** 84 tests pass on `aarch64` and on `x86_64`,
  with a test asserting which implementation actually dispatched — a
  differential test that quietly ran on the fallback would prove nothing.

## 5. What did not change

Filter-count scaling, measured with the vector engine (8 threads, 6-symbol
filters): 45.3M at one filter, 42.9M at a thousand — **1.05x**, well inside the
1.5x tolerance the matching work established.

## 6. Where the next gain is

Per candidate on a single thread, with the vector engine:

| | x86 (AVX2) | ARM (NEON) |
|---|---:|---:|
| Candidate | 239 ns | 120 ns |
| Arithmetic | 171 ns (72%) | 75 ns (63%) |
| Everything else | 68 ns (28%) | 45 ns (37%) |

**Matching is no longer the place to look**, at least not at ordinary
dictionary sizes: it costs about 2.8% of the candidate budget at a thousand
filters. At a million filters it rises to roughly 15%, so the case for working
on it is a case about large dictionaries, not about throughput in general.

**AVX-512 IFMA remains the largest untested lever.** Its 52-bit
multiply-accumulate would allow radix 2^51 in vector form — half the limbs, half
the operations — projecting to roughly 1.66x of the reference on x86 rather than
1.10x. No available machine has it: the x86 machine is Broadwell, and Apple
Silicon has no equivalent. It stays a hypothesis.

**A radix 2^64 scalar path with `mulx`/`adc`, the way the reference's fastest
build works, is not worth building.** It would speed up a path we no longer use:
our vector multiplication at 14.29 ns already beats the reference's assembly at
roughly 15 ns, and the vector path is 1.57x faster than our scalar one.
