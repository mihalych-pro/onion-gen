# Vector field arithmetic

**English** | [Русский](simd.ru.md)

This document describes the vector field arithmetic and the techniques around
it.

The [README](../../README.md) gives the measured speeds. This document gives
the reasons behind them.

## 1. What the vector path achieved

The vector path is faster than our own scalar path on both architectures. It is
also faster than the quickest build of `mkp224o` on the same machine.

The two architectures are not symmetrical, and the cause is in the reference.
On ARM our scalar path is already level with `mkp224o`, because the quickest ARM
build of `mkp224o` is portable C. On x86 the quickest build is hand-written
assembly. Our scalar path stays behind it there, and the vector path closes the
gap and then passes it.

## 2. The work around the multiplication

Arithmetic alone does not make the vector path fast. Our vector multiplication
costs less than the assembly of the reference, and the work around it decides
the result. Three techniques carry that work.

**Unrolled byte packing.** The canonical 32-byte encoding runs once for each
candidate. A general bit-writer would loop over the limbs and use about fifty
operations each time. Straight-line byte assignments in the style of ref10 do
the same work.

**No canonical reduction in the hot loop.** A field element encodes differently
from its canonical form only when the value is in `[p, 2^255)`. That is 19
values out of `2^255`, and no run will produce one. The match test therefore
reads the unreduced bytes, and the reduction runs only for a candidate that
matched, where the address must be correct. The field addition and subtraction
are fixed-size array operations for the same reason.

**No separate array for the deferred reduction.** To hold `y·z⁻¹` so that the
program can pack a hit again costs throughput in cache pressure. To compute `y`
again from the stored `z⁻¹` is cheaper.

### Two techniques that do not help here

**Two interleaved chain sets.** The chain and the batch inversion are both
dependency chains, so eight candidates in flight instead of four should fill the
stalls. They do not, on either architecture.

**A Montgomery chain in two halves.** This is the same idea applied to the
inversion, and it makes the engine slower. A second inversion adds about 265
multiplications, and the chain is limited by throughput and not by latency.

## 3. Field multiplication

The vector multiplication at radix 2^25.5 is faster than the scalar
multiplication from `fiat-crypto` at radix 2^51. This holds on NEON and on AVX2.
To measure it in our own code, run `cargo bench --bench field-bench`.

The NEON result agrees with the C microbenchmark in `scripts/bench/neon/`. Two
independent measurements are the reason to keep both.

The benchmark runs four independent chains on every path. One chain of dependent
multiplications would measure latency, and four independent chains measure
throughput. A benchmark that gives one path to each of those answers compares
nothing.

## 4. Correctness

The easiest defect to introduce here is a vector path that computes the wrong
value. Nothing fails, the program prints addresses, and the keys behind those
addresses do not work.

- **Field operations against the scalar field.** Multiplication, squaring,
  addition, subtraction, inversion and canonical packing go lane by lane against
  `fiat-crypto`. The inputs are random and also at the edges: zero, one, `p-1`,
  all limbs at the maximum, and unreduced sums.
- **Limb bounds.** A vector implementation fails quietly here. Multiplication
  scales one operand by 19 or by 38, which depends on the parity of the limb. It
  then gives the result to an instruction that reads 32 bits. The ceilings
  therefore differ by parity and by operand. A test built from maximal limbs
  confirms that three stacked additions fit and four do not. Random limbs do not
  show this, because their margins hide a wrong bound.
- **The complete engine.** Both engines generate 122 880 candidates from the
  same seed. The test in `tests/engine_equivalence.rs` compares them byte for
  byte and also at the address level. This test finds a lane mix-up, where each
  candidate is a valid key but holds the wrong counter offset.
- **Both architectures natively.** The suite passes on `aarch64` and on
  `x86_64`. One test asserts which implementation the program selected, because
  a differential test that ran on the fallback path proves nothing.

## 5. What did not change

A dictionary of a thousand filters still costs almost the same as one filter.
The vector engine keeps the scaling that the matching work established. See
[matching-forms.md](matching-forms.md).

## 6. Where the next gain is

The arithmetic takes most of the time for each candidate on both architectures.
The remaining work takes the rest.

**The match test is no longer the place to look** at ordinary dictionary sizes.
It costs a small part of the budget for each candidate at a thousand filters,
and a larger part at a million. A case for more work on it is therefore a case
about large dictionaries, not about throughput in general.

**AVX-512 IFMA is the largest untested lever.** Its 52-bit multiply-accumulate
instruction would allow radix 2^51 in vector form, which is half the limbs and
half the operations. No machine here has that instruction: the x86 machine is
Broadwell, and Apple Silicon has no equivalent. It stays a hypothesis.

**A scalar path at radix 2^64 with `mulx` and `adc` is not worth the work.** The
quickest build of the reference works that way. Such a path would make faster a
path that we no longer use, because our vector multiplication is already quicker
than the assembly of the reference.
