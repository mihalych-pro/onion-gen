# The search engine

**English** | [Русский](search-engine.ru.md)

This document describes how the engine produces candidates and how it decides
which ones to keep.

The [README](../../README.md) gives the measured speeds.

## 1. Candidates are not independent keys

A search over independent keys would cost one scalar multiplication for each
candidate. This engine never does that. Each candidate is the previous point
plus a constant, so the whole batch costs one field inversion and not one
inversion for each candidate. That is Montgomery batch inversion, and it is the
price of entry for this kind of program.

The program carries two engines on the processor, and it chooses between them
at startup.

**The paired engine, in `src/pairs.rs`, runs on the scalar path.** It keeps a
table of affine offsets `Q(m) = 8m·G` and reads two candidates from one base
point `P`. On an Edwards curve the sum and the difference share their expensive
terms:

```text
y(P+Q) = (x1y1 - x2y2) / (x1y2 - y1x2)
y(P-Q) = (x1y1 + x2y2) / (x1y2 + y1x2)
```

Only `x1y2` and `y1x2` are multiplications. The products `x1y1` and `x2y2` are
stored with the points. Two multiplications therefore produce two candidates,
against about eight for one point addition, and the base point moves once for
each round instead of once for each candidate.

**The chain engine, in `src/batch.rs`, runs on the vector path.** It pays a full
point addition for each candidate, and it processes four candidates in each
vector lane. See [simd.md](simd.md).

The step between candidates is 8. Clamping requires the low three bits of the
scalar to stay zero, and a multiple of eight leaves them alone.

## 2. The match test never builds an address it does not need

To derive an address costs a SHA3-256 checksum, a base32 encoding and a heap
allocation. The engine avoids all three for almost every candidate.

**The first test runs while the candidate is still four limbs in registers.**
`FilterSet::leading_probe` builds a probe from the leading bits. The first eight
bytes of the canonical form are the first limb, and canonicalisation changes
that limb by at most two subtractions of the modulus. A probe that tests all
three forms can therefore say "no", and it can never say "no" wrongly.

**The next test reads the packed public key.** The first 49 symbols of the
address come out of the packed key directly. See
[progress-and-resume.md](progress-and-resume.md) for why the span stops at 49.

**The address is built only for a candidate that survives.** A filter that
reaches past the key is prefiltered on the key and confirmed against the full
address afterwards, which happens too rarely to matter.

## 3. The field defers what it can

`fiat-crypto` multiplies unreduced operands. To carry after every addition and
then relax again before the multiplication is therefore pure waste. The field
addition and subtraction leave their results unreduced, and the canonical
reduction runs only where the encoding must be exact.

## 4. The index and the exact check

A bitmap index says that a candidate *might* match. Something must then say
which filter matched, and that second step decides the cost at large filter
counts.

**Filters sit in buckets by length.** Each bucket holds a sorted array of
64-bit key prefixes, so a bitmap hit costs one binary search for each distinct
length. A dictionary holds one or two lengths and not fifty.

**Below 64 filters the program scans instead.** The arrays fit in cache at that
size, and a scan has no branch misprediction to pay for.

**The index width follows the filter set.** It is not fixed. See
[form-indexes.md](form-indexes.md) for how the program chooses it and why the
two index kinds choose differently.

The sorted check also rescues the case that no index can help. A thousand
two-symbol filters saturate any index, so the program discards the index. The
sorted lookup then carries the whole match test on its own.

## 5. A dictionary is the strongest lever

A thousand names raise the hit rate a thousandfold, which is worth two prefix
symbols. No optimisation of the search core comes close to that, so the ceiling
on dictionary size is a product question and not a curiosity.

A million names load in a fraction of a second and cost almost nothing against
a thousand names. Adding filters is therefore free in practice, and a dictionary
is worth its full multiplier.

## 6. Correctness

- **Byte-for-byte output compatibility.** The writer takes 16 key directories
  produced by `mkp224o`, and all three files match exactly, in
  `tests/mkp224o_compat.rs`.
- **An independent verifier confirms the keys.** `verify-onion-address.py`
  shares no code with the generator. It computes the address again from the
  public key, and the public key again from the secret scalar. See
  `tests/end_to_end.rs`.
- **tor accepts the keys.** A service directory that holds only
  `hs_ed25519_secret_key` goes to tor with `DisableNetwork 1`. tor regenerates
  the public key and the `hostname` file, and it produces the same address that
  the generator reported.
- **A differential test against `curve25519-dalek`** covers the hand-written
  group layer, so a subtle error there surfaces in the test suite and not in
  somebody's live service.
- **A test against the printed address.** `tests/matching_finds_everything.rs`
  searches the finished address with an ordinary string search. A comparison of
  our code against our own other code cannot find an error that both sides
  share.

## 7. What the engine does not do

- **It does not publish anything.** Every check against tor runs with
  `DisableNetwork 1`. To announce a descriptor is a deliberate act and not a
  test step.
- **It does not write keys from the device.** The device reports positions, and
  the host derives the key. See [cuda-path.md](cuda-path.md).
