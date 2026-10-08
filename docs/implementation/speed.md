# Speed: the measurements and where they come from

**English** | [Русский](speed.ru.md)

The full comparison against `mkp224o`, build by build, and the list of
techniques the speed came from — including the ones that were measured and
thrown away. The short version is in the [README](../../README.md).

## Speed against mkp224o

`onion-gen` is measured against [`cathugger/mkp224o`](https://github.com/cathugger/mkp224o),
the established C implementation, at commit `5172c0fd`. On each machine the
reference is built in every configuration it offers and the fastest one is used
— on ARM that is portable C, on x86 hand-written assembly.

**One filter, candidates per second, higher is better:**

| Threads | ARM: mkp224o | ARM: onion-gen | | x86: mkp224o | x86: onion-gen | |
|---:|---:|---:|---:|---:|---:|---:|
| 1 | 5 887 160 | 8 345 600 | **1.42x** | 3 749 040 | 4 176 282 | **1.11x** |
| 2 | 11 293 700 | 16 502 374 | **1.46x** | 7 302 740 | 8 031 027 | **1.10x** |
| 4 | 22 266 900 | 31 941 018 | **1.43x** | 14 093 800 | 15 468 954 | **1.10x** |
| 6 | 30 469 300 | 42 943 693 | **1.41x** | 20 329 800 | 22 963 814 | **1.13x** |
| 8 | 30 528 000 | 45 754 778 | **1.50x** | 26 793 600 | 29 355 622 | **1.10x** |

The gap is wider on ARM because the reference has no vector implementation
there; on x86 its fastest build is hand-written SUPERCOP assembly, which our
scalar path does not match and our vector path overtakes.

### Every build against every path

The reference is not one program but a dozen build configurations, and they
differ by a factor of two on the same machine. Comparing against its *best*
build is the only honest thing to do, so that is what the table above does —
but it hides how much tuning that takes, which is what this one shows. One
sitting per machine, eight threads, one 6-symbol filter.

**Apple M1 Pro, 8 threads:**

| Build | candidates/s | vs its best |
|---|---:|---:|
| `donna/pcre2` | 21,477,900 | 0.79x |
| `donna/intfilter-binsearch-OMITMASK` | 24,459,200 | 0.90x |
| `donna/binfilter` | 24,525,000 | 0.90x |
| `donna/ibb/batch8192` | 24,813,400 | 0.91x |
| `donna/intfilter` | 25,407,800 | 0.94x |
| `donna/ibb/batch512` | 26,080,000 | 0.96x |
| `donna/intfilter-binsearch-besort` | 27,157,000 | 1.00x |
| **onion-gen, processor, scalar** | 21,627,392 | 0.80x |
| **onion-gen, processor, NEON** | **34,072,576** | **1.25x** |
| **onion-gen, Metal** | **86,050,816** | **3.17x** |
| **onion-gen, both** | 86,341,120 | 3.18x |

**Intel Xeon E5-2683 v4, 8 threads, no device:**

| Build | candidates/s | vs its best |
|---|---:|---:|
| `donna-sse2/intfilter-binsearch-besort` | 12,376,900 | 0.51x |
| `donna-sse2/intfilter` | 12,499,000 | 0.51x |
| `donna-sse2/binfilter` | 12,671,800 | 0.52x |
| `amd64-51-30k/intfilter-binsearch-besort` | 19,720,200 | 0.81x |
| `donna/intfilter-binsearch-besort` | 20,022,800 | 0.82x |
| `amd64-51-30k/binfilter` | 20,600,400 | 0.84x |
| `donna/intfilter` | 20,657,500 | 0.84x |
| `amd64-51-30k/intfilter` | 20,809,400 | 0.85x |
| `donna/binfilter` | 20,855,900 | 0.85x |
| `amd64-64-24k/intfilter-binsearch-besort` | 23,773,800 | 0.97x |
| `amd64-64-24k/binfilter` | 24,096,900 | 0.98x |
| `amd64-64-24k/intfilter` | 24,464,500 | 1.00x |
| **onion-gen, processor, scalar** | 18,405,376 | 0.75x |
| **onion-gen, processor, AVX2** | **26,098,176** | **1.07x** |

Three things are worth reading out of this. **Picking the wrong build of the
reference costs more than everything our vector work buys** — `donna-sse2` on
x86 runs at half the speed of `amd64-64-24k`, and a reader comparing against a
default build would get a flattering number that means nothing. **Our scalar
path is behind on both machines**, by a fifth on ARM and a quarter on x86; the
vector path is what closes it and passes. **Adding processor threads to a
running device buys nothing measurable** — 86.3 against 86.1 million — because
the card is an order of magnitude ahead and the cores it needs for feeding are
the ones it takes.

These ratios are lower than the headline table's because this sitting ran on a
busier machine; within one table the comparison holds, across tables it does
not. That is the same rule as everywhere else here.

**The difference widens with dictionaries.** Searching for many names at once is
the strongest lever there is against address length: a thousand names raise the
hit rate a thousandfold, far more than any arithmetic optimisation. That only
pays off if adding filters is free.

| Filters | onion-gen | mkp224o besort | mkp224o default |
|---:|---:|---:|---:|
| 1 | 30 000 333 | 31 815 400 | 29 071 300 |
| 10 | 31 522 406 | 29 644 800 | 25 899 500 |
| 100 | 31 340 544 | 27 042 800 | 10 163 500 |
| 1 000 | 29 902 438 | 22 697 500 | 1 548 470 |
| **Degradation 1 → 1000** | **1.00x** | 1.40x | **18.77x** |

With a thousand names `onion-gen` is **1.3x** faster than the best-tuned build
of the reference and **19x** faster than its default build. Adding filters costs
nothing measurable, so a dictionary is worth its full multiplier.

Filters of differing lengths change nothing either — index memory stays at
2 MiB whatever the set. The reference build that falls into its `OMITMASK` trap
could not even finish preparing the same filter set within 400 seconds.

**Dictionaries scale to a million names.** Adding names costs almost nothing,
so the full multiplier is available:

| Names in the dictionary | candidates/s | Load time |
|---:|---:|---:|
| 1 000 | 34 842 624 | < 0.01 s |
| 100 000 | 34 649 702 | 0.02 s |
| 1 000 000 | 33 602 355 | 0.16 s |

A million names raise the hit rate a thousandfold over a thousand names — worth
about two extra prefix symbols, which is more than any planned optimisation of
the search core will buy.

### Measurement conditions

Apple M1 Pro (6P + 2E), macOS 27.0.0, mains power, 8 threads unless stated,
6-symbol filters, median of 5 intervals of 5 s after a discarded warm-up.

**Both implementations were measured alternately in one sitting.** This matters:
the same binary measured on different days differed by about 20% as thermal
state and background load changed. Only figures from one sitting are comparable
— absolute numbers from this table will not reproduce on a different machine or
a differently loaded one.

**Every row names the path that produced it.** The CPU rows pin `--compute cpu`.
Without that the binary takes whatever it finds, and on a machine with a usable
card a row labelled "scalar" quietly measures the GPU — which is exactly what
happened to one acceptance table before the scripts started recording their own
configuration. Each CSV now carries the arithmetic and the devices in its
header, and the run refuses to start if it cannot read them.

Reproduce it yourself: take each program's released binary where it publishes
one, run a real search on a filter long enough that nothing is found, and take
the four in rotation so that whatever else the machine is doing lands on all of
them equally. The figures in the README were taken that way, three slices per
machine, median reported.

The analysis is in
[docs/implementation/search-engine.md](search-engine.md).

## Where the speed comes from

Every entry below was measured, and the two at the end were measured and thrown
away. They are listed because a technique that sounds right and does not work is
worth as much to the next person as one that does.

**A chain of additions with one shared inversion.** Candidates are not
independent keys: each point is the previous one plus a constant, so a whole
batch costs a single field inversion instead of one per candidate. The reference
does this too — it is the price of entry, not an advantage.

**Matching reads the packed key, not the address.** The first 49 symbols of the
address fall out of the packed key directly. The deferred sign bit lands beyond
any filter's reach and the checksum is only computed once a candidate has
already matched. Canonical reduction likewise moved out of the hot loop: it
changes the encoding only for values in `[p, 2^255)`, 19 of them out of `2^255`,
and no run will ever produce one. Worth 27.9 → 29.7 million candidates a second
on x86.

**Unrolled byte packing.** The canonical 32 bytes used to be assembled by a
general bit-writer looping over limbs, about fifty operations for every single
candidate. Direct assignments in the style of ref10 replaced it: 25.6 → 27.9
million on x86. This was the largest single win in the vector work, and none of
it is arithmetic.

**Four field elements per vector, radix 2^25.5.** NEON multiplies 32×32 but not
64×64, so the vectorisable representation is ten 32-bit limbs rather than the
five 64-bit limbs the scalar path uses. Twice the operations, four lanes:
**1.88x** on the multiply with NEON, **1.75x** with AVX2, which lands as
1.38–1.68x on end-to-end throughput.

**A prefix bitmap that does not grow with the dictionary.** The index is 2 MiB
whatever the filter set, so a thousand names cost what one name costs — 1.00x
degradation, against 18.77x for the reference's default build. It is switched on
by the measured fill of the index it just built, not by the length of the
filters: a rule based on length alone was tested and gave up 4.6x and 59x on two
real configurations.

**The graphics device.** 3.44x over the same machine's processor on an M1 Pro
through Metal, 14.3x on an RTX 4060 through Vulkan. The portable path beats the
vendor one on NVIDIA's own card.

**More machines.** Distributed search adds workers under a master that hands out
seeds and block ranges; two machines measured 337.4 blocks a second against
153.5 for one, with no duplicated work.

### Measured and rejected

**Karatsuba by parity split.** The arithmetic is correct and the operation count
drops, and it is still **slower**: 0.68x with NEON, 0.73x with AVX2. A field
multiply here is bound by throughput, and Karatsuba trades multiplies for
additions and shuffles that the vector units are already saturated with.

**Two interleaved chain sets, and splitting the Montgomery chain.** Both were
meant to fill stalls left by the data dependency in the chain and in the batch
inversion. The first showed no difference on either architecture; the second
cost 8%, because an extra inversion is about 265 multiplies and the chain is not
latency-bound in the first place.

## Closing the gap with prefix32 on the processor

Reading [`prefix32`](https://github.com/0xROOTPLS/Prefix32) gave four
differences, each measured here before it was adopted. The measurements are
reproducible: `cargo bench --bench field-radix` for the field, `--bench
pair-scheme` for the scheme.

**The field: a saturated radix 2^64 over four limbs**, instead of
`fiat-crypto`'s redundant radix 2^51 over five. Fewer limbs mean fewer partial
products; the carry chains the redundant form avoids cost less than the
products it adds. Measured on the multiply alone: 1.23x on an M1 Pro, 1.58x on
a Xeon E5-2683 v4.

**Written out rather than looped.** One carry chain per row of the schoolbook,
so every partial product stays in a register: 8.1 ns against 10.3 for the same
arithmetic in a loop.

**A candidate is tested while it is still four limbs.** Reducing every
candidate to its canonical 32 bytes, only for the index to decline almost all
of them, is work the index can avoid: the first eight bytes of the canonical
form are the first limb, and canonicalising changes that limb by at most two
subtractions of the modulus, which add 19 each. Testing all three forms can say
"no" and never say it wrongly.

**One pass fewer over the batch.** The running product of the denominators is
folded into the numerators as they are produced, so Montgomery's trick needs
neither a third array nor a forward pass of its own. Four array accesses per
candidate instead of seven.

Where that leaves it, eight threads, one six-symbol filter, slices in rotation:

| | M1 Pro | Xeon E5-2683 v4 |
|---|---:|---:|
| at the start | 63.1 | 24.3 |
| now | **~100** | **~65** |
| `prefix32`, built from its own source | 97.5 | 65.1 |
| `prefix32`, the binary it ships | — | 58.5 |

A note on the x86 column. The field is four 64-bit limbs fed through
add-with-carry chains, which is what `mulx` and `adcx`/`adox` exist for, and
the baseline x86-64 target has none of them. `prefix32` solves this by building
for the host: its repository sets `target-cpu=native`, and its released binary
does not have those instructions. The engine here is compiled twice instead and
chosen once, so one portable binary carries both — which is why the figure above
is the portable build.
