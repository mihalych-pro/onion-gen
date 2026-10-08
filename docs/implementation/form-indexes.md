# Indexes for the general forms

**English** | [Русский](form-indexes.ru.md)

Structures that make a dictionary of substrings, wildcards, classes or suffixes
cost what a dictionary of prefixes costs.

## 1. What it was and what it is

Degradation from one filter to a thousand, Xeon E5-2683 v4, 8 threads, AVX2,
idle machine:

| Form | Before | After | At 1000 filters |
| --- | --- | --- | --- |
| prefix | 1.00x | 1.00x | 30.3 → 30.4 M/s |
| character class | 214x | **1.05x** | 0.1 → 28.3 M/s |
| suffix | 494x | **1.07x** | 0.1 → 26.6 M/s |
| wildcard | 214x | **1.18x** | 0.1 → 25.2 M/s |
| substring | 240x | **1.87x** | 0.07 → 8.7 M/s |

On ARM, where the base is faster, every form including the substring is inside
the 1.5x the spec asks for: 0.99x, 1.09x, 1.05x, 1.35x, 1.27x
(taken under load — the ratios hold,
the absolute figures do not compare with x86).

Several forms are also faster at *one* filter than before: the wildcard went
27.8 → 29.8 M/s and the suffix 25.4 → 28.6, because a bitmap helps a single
form too.

## 2. Three structures, because the forms differ in where they are anchored

**A literal substring gets an automaton.** Section 11 of
`matching-forms.md` records that story: the automaton alone changed nothing
until the checksum prefilter behind it stopped being linear, and a tail
prefilter removed the hash from all but a few candidates. Where it starts to
pay against one search per filter is measured by
`cargo bench --bench substring-scaling`.

**A form anchored to the start gets the same bitmap a prefix gets**, filled by
enumeration rather than as a contiguous run, because its fixed bits are
scattered. One wildcard inside the indexed span is 32 slots, a class of four is
four, two wildcards is a thousand. Past a budget the enumeration is not worth
doing, and such a form is left out of the index and checked one by one — the
index reports which forms it covers, because a prefilter that quietly fails to
cover something is not a prefilter but a wrong answer.

**A suffix gets the same bitmap at the other end.** Its window sits at the end
of the key-derived span, so the index is parameterised by a bit offset; offset
zero is the same code the prefix index has always used.

One mistake is worth recording. The suffix bitmap first went in front of the
ordinary key-side check, where it is useless: a suffix ends at symbol 55, so its
placement never fits inside the key and that check can only ever fail. The
linear walk was in the checksum prefilter, and that is where the bitmap
belongs.

## 3. Index width: chosen, not fixed

A fixed 24 bits was wrong at both ends. Matching in isolation
(`benches/index-width.rs`, x86):

| filters | 16 KiB | 32 KiB | 128 KiB | 2 MiB | 16 MiB |
| --- | --- | --- | --- | --- | --- |
| 1 000 | 93.3 | **100.1** | 95.3 | 51.0 | 47.3 |
| 10 000 | 54.3 | 80.1 | **83.6** | 49.0 | 46.3 |
| 100 000 | — | — | 34.9 | **45.5** | 36.0 |
| 1 000 000 | — | — | — | **25.9** | 25.4 |

Two things follow. At a thousand filters the old default was **half** the speed
of a 32 KiB map. And at a million filters the 2 MiB map beats the 16 MiB one
although its false-hit rate is eight times worse — that is the cache paying for
itself, and it is why the ceiling is 24 bits.

The width is chosen by building and measuring occupancy rather than by a
formula over the filter count, because a wildcard fills tens of slots and the
same count can mean very different occupancy.

**The target is not the same for every index, and getting that wrong cost a
factor of three.** The first version used one percent everywhere and dropped
the wildcard, class and suffix from 1.06–1.19x to 2.85–3.52x. An index is worth
only what stands behind it: behind the prefix index is a binary search over
length buckets, behind the pattern index is a walk over every form in the
group. With a thousand forms the same false hit costs a thousand times more, so
the tolerable rate falls in the same proportion. Prefixes now take 16 KiB and
the general forms 2 MiB, from one rule.

This also closed a puzzle from the previous change: prefix dictionary search
measured 0.978 against the commit before it, which I had attributed to code
layout. With the width chosen it measures 0.996. Part of that gap was real, and
it was the index being too wide.

## 4. Where the time goes now

Profiled with `perf` on x86, 8 threads:

| Workload | Top costs |
| --- | --- |
| 1000 prefixes | field multiply 76.7%, hot loop 11.0%, matching 4.3% |
| 1000 substrings | automaton 48.6%, field multiply 20.6%, encoding 12.2%, tail prefilter 6.3%, SHA3 3.6% |

For prefix search **matching is no longer worth optimising**: removing it
entirely would buy 4%. The remaining lever is the field arithmetic, and it is
already at 84% of what the hardware allows — 131 cycles per four-lane multiply
against a floor of about 109, set by one `vpmuludq` per cycle on port 0 for the
100 partial products plus 9 for the reduction.

The group level has no slack either, which was checked rather than assumed: a
chain step is 8 multiplications, the canonical count for extended twisted
Edwards coordinates, and Montgomery batch inversion adds 3 plus 1 for the
coordinate itself. Twelve per candidate, and the measured 15 is not extra work
but the same work at 40 cycles instead of the microbenchmark's 33, because in
the engine the multiply competes for cache.

That leaves two levers inside the field, both recorded rather than taken:
Karatsuba over the ten limbs, 75 partial products instead of 100, an upper
bound near 1.3x on x86; and AVX-512 IFMA, which has no hardware here.

Libraries offer nothing: `curve25519-dalek`'s AVX2 backend packs the limbs of
one point across lanes and unpacks them on every multiply, while we vectorise
across four independent candidates with no shuffles at all. `fiat-crypto` has
no vector backend. dalek does carry an IFMA backend, which will be a useful
reference when there is a machine to run it on.

## 5. What remains linear

A substring holding a class or a wildcard. It has no literal for a searcher and
no fixed position for a bitmap, so it is checked at every offset. Such a filter
is named in the diagnostics, because otherwise its cost would look inexplicable.
A dictionary is a list of words, so this does not arise from dictionaries.
