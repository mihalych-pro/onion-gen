# Karatsuba in this field

**English** | [Русский](karatsuba.ru.md)

Karatsuba multiplication in the field: correct, and slower. This document
exists so that nobody spends the same week finding out again.

The implementation is not in the tree. It is at commit `fee714a`, with its
tests, and can be recovered from there.

## 1. Why it was worth trying

The processor goal of 2–5x is not met — 1.42–1.50x on ARM, 1.10–1.13x on x86 —
and the profile says where the only remaining room is: the field multiply takes
**76.7%** of the time, the rest of the hot loop 11%, matching 4.3%.

The multiply itself is already at 84% of what the hardware allows: 131 cycles
per four-lane call against a floor near 109, set by one `vpmuludq` a cycle on a
single port. There is no slack in the implementation. The only lever left is
doing fewer multiplications, and Karatsuba is **75 partial products against
100**.

## 2. The obvious Karatsuba is wrong here

Splitting the ten limbs in half by position — the textbook form — does not work
on this representation, and that was established by computation rather than
argument: it disagrees with the schoolbook form on **every one** of 2000 random
inputs.

The reason is the radix. The limbs alternate 26 and 25 bits, so a product of two
odd-indexed limbs carries an extra factor of two, while the identity needs one
factor across a whole sub-product. Splitting by position mixes the parities and
the factor stops being uniform.

## 3. Splitting by parity does work

Split the limbs by the parity of their index instead. Then each sub-product has
a single factor: even times even is one, odd times odd is two, mixed is one.

```
f·g = FeGe + [(Fe+Fo)(Ge+Go) − FeGe − FoGo] + 2·FoGo
```

with the even product landing at original limb `2m`, the cross term at `2m + 1`
and the odd product at `2m + 2`.

Correct on 5000 random inputs in a model, then on a million in the
implementation, then against `fiat-crypto` — a different radix and a different
provenance — on a hundred thousand, then lane for lane against the vectorised
schoolbook form on two hundred thousand. Edge inputs were included: all-zero,
every limb at its ceiling, `p` and `p − 1`.

The subtraction that produces the cross term can never go negative, which is
worth stating because it is where the danger would be: `(Fe+Fo)(Ge+Go)` is
identically `FeGe + FoGo + cross` and every limb in this representation is
non-negative.

## 4. And it is slower

| | schoolbook | Karatsuba | ratio |
|---|---|---|---|
| NEON | 6.69 ns | 9.84 ns | **0.68x** |
| AVX2 | 14.26 ns | 19.56 ns | **0.73x** |
| one lane at a time, ARM | 34.0 ns | 27.4 ns | 1.24x |
| one lane at a time, x86 | 53.9 ns | 64.1 ns | 0.84x |

The saving is real where registers are plentiful and additions are free: ARM
scalar gains 24%. It disappears under vectorisation, and the reason is
structural rather than incidental. Karatsuba has to hold **twenty-seven
accumulators** — nine for each of three sub-products — against the schoolbook
form's ten. NEON has thirty-two registers and AVX2 sixteen. What is saved on
twenty-five multiplies is lost to spilling and to the extra work: the cross term
needs eighteen 64-bit subtractions, and folding needs thirteen scalings by
nineteen, which without a 64-bit multiply is shifts and adds.

x86 is the sharper case: there Karatsuba loses even before vectorisation.

## 5. What this leaves

Nothing on the processor, within this representation. The multiply is at 84% of
the port limit, the group level is at its canonical count of eight
multiplications per addition, and the one algorithmic lever measures worse.

The remaining ideas both need hardware that is not here: AVX-512 IFMA, whose
52-bit multiply-accumulate would halve the limb count, and which projects to
about 1.66x on x86 against the present 1.10x. Everything else points at the
device, where the same work already runs seven times faster than the processor.
