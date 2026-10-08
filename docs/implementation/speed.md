# Speed: how we measure it, and where it comes from

**English** | [Русский](speed.ru.md)

The [README](../../README.md) holds the measured speeds. This document holds the
method behind them and the list of techniques that produced them, including the
techniques that we measured and then rejected.

## 1. How to measure

**Build the reference in every configuration, then use the quickest one.**
`mkp224o` is not one program. It has about a dozen build configurations, and on
one machine the slowest and the quickest differ by a factor of two. A comparison
against a default build gives a flattering number that means nothing. On ARM the
quickest build is portable C. On x86 it is hand-written assembly.

**Measure all programs alternately in one sitting.** The same binary on
different days gives different results, because the thermal state and the
background load change. Only figures from one sitting are comparable. Do not
compare a figure from one table with a figure from another.

**Name the path for each row, and pin it.** Use `--compute cpu` for a processor
row. Without it the program takes whatever it finds, and on a machine with a
usable device a row that says "processor" measures the device instead. This
happened to one acceptance table before the scripts recorded their own
configuration.

**Search for something that nothing will find.** Give each program a filter long
enough that no hit lands in the measurement window. A hit stops the run and
makes the sample shorter than the others.

**Rotate the programs.** Run them in turn, so that whatever else the machine
does lands on all of them equally. Take three slices for each machine and report
the median.

**Use the released binary where the program publishes one.** Build from source
only when there is no release, and say which one the figure comes from.

### What the scripts record

`scripts/bench/run-form-acceptance.sh` measures our own throughput for each
matching form. Its CSV header records the date, the architecture, the operating
system, the power state, the arithmetic path, the thread count and the sample
count. A figure without that header cannot be placed later.

### Conditions for the README figures

The README names the machine, the operating system and the thread count for each
column. The measurement window is a set of fixed intervals after a discarded
warm-up, and the figure is the median.

## 2. Where the speed comes from

**A chain of additions with one shared inversion.** Candidates are not
independent keys. Each point is the previous point plus a constant, so a
complete batch costs one field inversion instead of one inversion for each
candidate. The reference does this too. It is the price of entry, not an
advantage.

**The match test reads the packed key, not the address.** The first 49 symbols
of the address come out of the packed key directly. The deferred sign bit lands
beyond the reach of any filter. The program computes the checksum only after a
candidate matches.

**No canonical reduction in the hot loop.** The reduction changes the encoding
only for values in `[p, 2^255)`. That is 19 values out of `2^255`, and no run
will produce one.

**Unrolled byte packing.** The canonical 32 bytes come from direct assignments
in the style of ref10. A general bit-writer would loop over the limbs and use
about fifty operations for each candidate, and none of that is arithmetic.

**Four field elements in each vector, radix 2^25.5.** NEON multiplies 32 bits by
32 bits, but not 64 by 64. The representation that vectorises is therefore ten
32-bit limbs, and not the five 64-bit limbs of the scalar path. That is twice
the operations across four lanes, and the four lanes win.

**A prefix bitmap that does not grow with the dictionary.** The index keeps a
fixed size for any filter set, so a thousand names cost what one name costs. The
program switches the bitmap on from the measured fill of the index that it just
built. A rule based on filter length alone was tested, and it gave up a large
factor on two real configurations.

**The device.** A device is much faster than the processor of the same machine.
The portable path is quicker than the vendor path on the card of NVIDIA.

**More machines.** Distributed search adds workers under a master. The master
hands out seeds and block ranges, and two machines did more work than one with
no duplicated work.

## 3. What does not work here

**Karatsuba.** The textbook split, which halves the limbs by position, is
incorrect for this representation: the limbs hold 26 bits and 25 bits in turn,
so a product of two odd-index limbs carries one more factor of two, and the
identity needs one factor for a complete sub-product. A split by the parity of
the index is correct, and it drops the partial products from 100 to 75.

It is still slower under NEON and under AVX2. The cause is structural.
Karatsuba must hold 27 accumulators, nine for each of three sub-products, where
the schoolbook form holds ten. NEON has 32 registers and AVX2 has 16, so the
saving on 25 multiplications goes to register spills and to the extra additions
and shuffles that the vector units are already full of.

**Two interleaved chain sets.** Eight candidates in flight should fill the
stalls that the data dependency leaves in the chain and in the batch inversion.
They do not, on either architecture.

**A Montgomery chain in two halves.** This is the same idea applied to the
inversion, and it makes the engine slower. A second inversion adds about 265
multiplications, and the chain is not limited by latency.

## 4. What closed the gap with prefix32

Four techniques come from [`prefix32`](https://github.com/0xROOTPLS/Prefix32).
To measure them here, run `cargo bench --bench field-radix` for the field and
`cargo bench --bench pair-scheme` for the scheme.

**A saturated radix 2^64 over four limbs**, in place of the redundant radix
2^51 over five limbs from `fiat-crypto`. Fewer limbs give fewer partial
products. The carry chains that the redundant form avoids cost less than the
products that it adds.

**Written out, not looped.** One carry chain for each row of the schoolbook
form, so that every partial product stays in a register.

**The program tests a candidate while it is still four limbs.** To reduce every
candidate to its canonical 32 bytes, only for the index to refuse almost all of
them, is work that the index can avoid. The first eight bytes of the canonical
form are the first limb. Canonicalisation changes that limb by at most two
subtractions of the modulus, and each subtraction adds 19. A test of all three
forms can say "no", and it can never say "no" wrongly.

**One pass fewer over the batch.** The program folds the running product of the
denominators into the numerators as it makes them. The trick of Montgomery
therefore needs neither a third array nor a forward pass of its own.

### A note on the x86 column

The field is four 64-bit limbs that go through add-with-carry chains. The
instructions `mulx`, `adcx` and `adox` exist for that work, and the baseline
x86-64 target has none of them. `prefix32` solves this with a build for the host:
its repository sets `target-cpu=native`, and its released binary does not hold
those instructions. We compile the engine twice instead and choose once at
startup, so one portable binary carries both paths.

The analysis of the engine is in [search-engine.md](search-engine.md).
