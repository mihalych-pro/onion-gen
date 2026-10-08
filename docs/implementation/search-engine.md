# The search engine

**English** | [Русский](search-engine.ru.md)

Acceptance results for the first working version of `onion-gen`, and the places
where reality differed from the plan.

## 1. Throughput against the reference

Both binaries measured back to back in one sitting, same filter set, same
thread counts (Apple M1 Pro, macOS 27.0.0,
mains power, one 6-symbol filter).

| Threads | `mkp224o` best build | `onion-gen` | Ratio |
|---:|---:|---:|---:|
| 1 | 5 812 260 | 5 948 621 | 1.02 |
| 2 | 11 226 900 | 11 743 232 | 1.05 |
| 4 | 21 948 000 | 22 358 426 | 1.02 |
| 6 | 30 006 800 | 30 677 811 | 1.02 |
| 8 | 31 109 300 | 32 351 437 | 1.04 |

The floor is met at every thread count, and thread scaling matches the
reference (5.44x versus 5.35x at 8 threads).

This is on a scalar implementation, with no SIMD and no GPU — both of which the
plan reserves for later changes.


### On x86 the floor is not met, and that was not known until now

The table above is `darwin/arm64`. When an x86 machine became available during
the SIMD change, the same comparison was run natively on an Intel Xeon E5-2683
v4:

| Threads | `mkp224o` best build | `onion-gen` | Ratio |
|---:|---:|---:|---:|
| 1 | 3 738 880 | 2 952 704 | **0.79** |
| 2 | 7 089 220 | 5 532 160 | **0.78** |
| 4 | 13 877 200 | 10 683 392 | **0.77** |
| 8 | 26 887 200 | 21 372 416 | **0.79** |

The floor — "no slower than the best reference build on the same hardware" — is
therefore **met on ARM and missed on x86**, by about a fifth.

The reason is visible in the reference's own build matrix: on ARM its fastest
build is `donna`, portable C, which `fiat-crypto` matches. On x86 its fastest
build is `amd64-64-24k`, hand-written assembly, which `fiat-crypto`'s scalar
code does not. Against `donna` on the same x86 machine the ratio is 0.94, so
most of the gap is the assembly rather than anything specific to our engine.

This makes the SIMD work necessary on x86 rather than merely desirable: AVX2
measures 1.75x on field multiplication, and at a 75% arithmetic share that
projects to roughly 1.19 of the best reference build. Until that lands, the
honest statement is that `onion-gen` is at parity on ARM and behind on x86.

### Comparisons hold only inside one sitting

It was originally written as an absolute threshold: "no less than 31M
candidates/s". Measurement showed that tests the machine rather than the code.
The same reference build, measured on two different days, differed by about 20%;
during one session it ranged from 25.8M to 31.8M as background load shifted.

The criterion is relative: our median must be no lower than the reference's
median **measured in the same sitting**. Measuring the two alternately is what
makes that hold — whatever else the machine is doing is then charged to both.

## 2. Throughput against filter count

The tolerance set by the analysis was "no more than 1.5x degradation from 1 to
1000 filters at any filter length". Measured (6-symbol filters,
8 threads, one sitting):

| Filters | `onion-gen` | `mkp224o` besort | `mkp224o` default |
|---:|---:|---:|---:|
| 1 | 30 000 333 | 31 815 400 | 29 071 300 |
| 10 | 31 522 406 | 29 644 800 | 25 899 500 |
| 100 | 31 340 544 | 27 042 800 | 10 163 500 |
| 1 000 | 29 902 438 | 22 697 500 | 1 548 470 |
| **1 → 1000** | **1.00x** | **1.40x** | **18.77x** |

The dependence is gone, not merely reduced. That matters beyond the number: a
dictionary is the strongest lever against name length, and it only pays off if adding
filters is free.

### Filters of differing lengths

The reference's `OMITMASK` trap costs up to 8x when filter lengths diverge. On
our side:

| Set | Longest filter | Index size | `calc/sec` |
|---|---:|---:|---:|
| 1000 × 6 symbols | 6 | 2 048 KiB | 33 248 500 |
| the same plus one 11-symbol | 11 | 2 048 KiB | 32 661 100 |
| the same plus one 20-symbol | 20 | 2 048 KiB | 32 611 900 |

Index size does not depend on the filter set at all, and throughput changes
within the measurement spread.

For contrast, the reference built with `--enable-intfilter --enable-binsearch`
and without `--enable-besort` **did not finish preparing filters within 400
seconds** on the middle set: flattening a thousand six-symbol filters to an
11-symbol mask means `1000 · 32⁵` entries. In that configuration the trap is not
a slowdown, it is an inability to start.

## 3. Two defects found by measurement

Both were invisible to the tests — every test passed throughout — and both were
found only because throughput was measured from the first working run rather
than at the end.

**Matching computed the address for every candidate.** Deriving an address means
a SHA3-256 checksum, a base32 encoding and a heap allocation. Throughput was
**8.3M** against a reference at 31M. Matching now compares against the packed
public key directly; a filter longer than 51 symbols is prefiltered that way and
confirmed against the full address afterwards, which happens so rarely that its
cost is irrelevant.

**Field addition reduced results that were immediately un-reduced.**
`fiat-crypto` multiplies unreduced operands, so carrying after every addition
and relaxing again before the multiplication was pure waste. Deferring the
reduction gave **+13%** end-to-end (29.2M → 32.9M) and is what moved the
implementation past the reference.

## 4. Open questions from the design, answered

**The bitmap width `k`.** The design proposed 24 bits (2 MB) from a density
calculation and left the final value to measurement. It stands: the index is
2 MiB regardless of the filter set, it fits L2 on the reference platform, and no
configuration measured here made a different width preferable. It remains
configurable.

**The threshold below which the prefilter is disabled.** The design guessed
"prefixes shorter than four symbols". Measurement showed that guess loses real
gains, so the rule changed: the decision is made by **measured occupancy of the
built index**, not by filter length. An index filled beyond 25% is discarded.

Measured in isolation (`cargo bench --bench match-bench`, 4M
candidates, no key writing and no curve arithmetic):

| Symbols | Filters | Indexed, keys/s | Scan, keys/s | Speedup | Decision |
|---:|---:|---:|---:|---:|---|
| 2 | 1 | 912 166 153 | 197 277 968 | 4.6x | indexed |
| 2 | 1 000 | 592 180 | 595 547 | 1.0x | discarded, map saturated |
| 3 | 1 000 | 18 093 810 | 305 087 | 59.3x | indexed |
| 4 | 1 000 | 325 589 274 | 299 923 | 1 086x | indexed |
| 6 | 1 000 | 738 694 557 | 304 763 | 2 424x | indexed |

By the original length rule, the 2/1 and 3/1000 rows would have run unindexed,
giving up 4.6x and 59x respectively.

## 4a. How large a dictionary can be

A dictionary is the strongest lever against address length, so its ceiling is a
product question, not a curiosity: a thousand names raise the hit rate a
thousandfold, which is worth two equivalent prefix symbols — more than the
entire planned core acceleration programme, which buys 1.13.

Measured (8-symbol filters, 8 threads):

| Filters | 2 MiB index | 128 MiB index | Load time |
|---:|---:|---:|---:|
| 1 000 | 34 842 624 | 32 484 966 | < 0.01 s |
| 100 000 | 34 649 702 | 34 640 282 | 0.02 s |
| 1 000 000 | 33 602 355 | 34 220 032 | 0.16 s |

**A million names cost 1.04x against a thousand.** Loading them takes 0.16 s and
the process holds 229 MiB.

Two things had to be true for that, and only one of them was.

**A wider index is not the answer.** Going from 2 MiB to 128 MiB changes nothing
measurable, and at small filter counts it is slightly worse: the map stops
fitting in cache, so every candidate pays a memory access to save false hits
that were already rare. 2 MiB stays the default.

**The exact check was.** The bitmap only says a candidate *might* match;
something then has to say which filter, and that step was a linear scan. At a
thousand filters it is invisible, at a million it is the whole cost. Measured on
the matcher alone:

| Filters | Linear check | Sorted check | |
|---:|---:|---:|---:|
| 1 000 | 659 975 396 | 291 887 327 | scan still wins below 64 filters |
| 10 000 | 46 281 520 | 415 634 853 | 9x |
| 100 000 | 525 878 | 249 152 632 | 474x |
| 1 000 000 | 5 005 | 55 295 398 | **11 000x** |

Filters are now bucketed by length and each bucket holds a sorted array of
64-bit key prefixes, so a bitmap hit costs one binary search per distinct
length — and dictionaries hold one or two lengths, not fifty. Below 64 filters
the scan is kept: the arrays fit in cache and it has no branch misprediction to
pay for.

**The index width should follow the filter count, and currently does not.**
Measured on the matcher after the sorted lookup landed (keys/s):

| Filters | 128 KiB | 2 MiB | 16 MiB | 128 MiB |
|---:|---:|---:|---:|---:|
| 1 000 | **330M** | 292M | 92M | 67M |
| 10 000 | 405M | **416M** | 114M | 60M |
| 100 000 | 161M | **249M** | 107M | 69M |
| 1 000 000 | disabled | 55M | **67M** | 61M |

The optimum grows with the filter count, and the fixed 24 bits lose at both
ends: at a thousand filters the map needlessly misses L1, at a million it
needlessly lets false hits through. A candidate rule — the smallest `k` keeping
occupancy under about 1% — would pick 17, 20, 24 and 27 for those four rows,
close to the measured optima. Left to the matching change that follows the SIMD
work, and to be settled by measurement rather than by the formula.

Sorting also rescues the case the bitmap cannot help with at all. A thousand
two-symbol filters saturate any index, so it is discarded — and before, that
left a linear scan running at 592 180 keys/s. With the sorted lookup the same
set matches at 60 359 402 keys/s, a hundredfold better, with no index involved.

What is left to gain is small: at a million filters matching costs 3.4%
end-to-end (33.6M against 34.8M at a thousand), because the matcher now runs at
55–350 million keys/s against the engine's 34 million candidates/s. Making the
lookup faster still would recover at most those 3.4%.

The earlier end-to-end figures that suggested a wide index helped were
measured with 6-symbol filters at 100 000 filters, where a hit occurs every
11 000 candidates. What that measured was the key-writing path, not matching —
precisely the error , warns about.
The measurements above use 8-symbol filters, which keeps the hit probability at
`9.1·10⁻⁷`.

## 5. Correctness

- **Byte-for-byte output compatibility.** 16 key directories produced by
  `mkp224o` were fed through our writer; all three files matched exactly
  (`tests/mkp224o_compat.rs`).
- **An independent verifier confirms the keys.** `verify-onion-address.py`
  shares no code with the generator and recomputes the address from the public
  key and the public key from the secret scalar (`tests/end_to_end.rs`).
- **tor accepts the keys.** A service directory holding only
  `hs_ed25519_secret_key` was given to tor 0.4.9.x with `DisableNetwork 1`; tor
  regenerated the public key and `hostname` and produced exactly the address the
  generator had reported.
- **Differential test against `curve25519-dalek`** over 1000 seeds, so a subtle
  error in the hand-written group layer surfaces in the test suite rather than
  in someone's live service.
- 56 tests in total.

## 6. Left undone

- **`linux/amd64` was not built or run.** No Linux machine, no Docker daemon and
  no cross-linker were available. Task 8.3 stays open.
- **The service was not published to the live Tor network.** Everything up to
  that point is verified — dictionary, hit, key verification, tor bringing the
  service up at the found address — but announcing a descriptor publicly is a
  deliberate act, not a test step.
- **SIMD, GPU, flexible matching, distributed mode and observability** are out of
  scope by design; each has its own change.
