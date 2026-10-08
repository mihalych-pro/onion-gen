# Scoring finds

**English** | [Русский](hit-scoring.ru.md)

This document describes how the program scores a find, and why it scores it that
way.

## 1. Rarity, and not taste

A broad filter produces finds in a stream. The filter `contains:abc` matches
about one candidate in 630, which over a run is thousands of directories. Anyone
can argue about which of them is better, so we replaced that question with one
that we can measure. **"Prettier" means "rarer".**

The replacement is not cosmetic. It lets a feature fail to qualify, and one
feature did. See the end of section 2.

## 2. The features and their measured rarity

The calibration uses five million random addresses from the encoder that the
program itself uses, so the checksum and the fixed tail are real. The code is in
`benches/score-calibration.rs`, section 1. The protocol fixes the last two
symbols of an address, and no feature uses them.

| Family | Level | Share of addresses | Bits |
|---|---:|---:|---:|
| run of identical symbols | 3 | 4.8% | 4.4 |
| | 4 | 0.15% | 9.4 |
| | 5 | 0.0049% | 14.3 |
| two-symbol tiling (`abab`) | 2 | 4.6% | 4.4 |
| | 3 | 0.0043% | 14.5 |
| palindrome | 4 | 9.4% | 3.4 |
| | 6 | 0.30% | 8.4 |
| | 8 | 0.0086% | 13.5 |
| | 9 | 0.0045% | 14.4 |
| few digits (non-digits of 54) | 46 | 29% | 1.8 |
| | 50 | 1.8% | 5.8 |
| | 52 | 0.12% | 9.7 |
| | 54 | 0.0017% | 15.9 |

The program extrapolates past the measured range only where the slope is a fact
and not a curve fit. One more symbol in a run is one more symbol that had to come
up the same. That is 32 times rarer, so five more bits, and the measured steps
are +5.0 and +4.9. The alphabet says this, and no fit is necessary. A tiling unit
is two symbols, so the step is ten bits.

Palindromes have no such slope, because odd and even lengths behave differently.
That family therefore saturates at its last measured level and says so. It does
not invent a number.

### A feature that is deliberately absent

"An address without digits reads better" is true, and the table above grades it.
"A long stretch of letters" sounds like the same claim, and it is not a feature
here:

| Letters-only run | Share of addresses |
|---|---:|
| 10+ | 79% |
| 12+ | 60% |
| 14+ | 42% |
| 16+ | 28% |
| 20+ | 11% |

Up to 14 symbols such a run describes most addresses, and not the good ones.
Above 16 symbols it is no longer too common, and there a second reason decides.
It is not a second feature. It is the same one. The run and the digit count are
two views of one sparsity, and to keep both would count that sparsity twice. The
program keeps the count, because the count grades further.

## 3. Placement is the only part that knows the filter

For a form that could match in several places, the program derives the value of
where the form landed. It does not measure it. A form with `p` placements lands
at or before offset `k` in about `(k + 1) / p` of the addresses that it matches
at all. To land at the very front is therefore worth `log2(p)` bits.

A form anchored to the start has one placement and contributes nothing. That is
correct, and it is not a special case.

## 4. What it costs

Scoring runs only on a find, and a find already writes a key to disk. A disk
write is many thousands of times more expensive than all four feature tests
together. The correct comparison is therefore against the write, and not against
zero.

The palindrome scan is quadratic, and it was the only candidate for removal on
cost. Against the write, the question does not arise.

The disk is also why the write threshold is useful. On a short filter, finds
arrive much faster than a disk can take them, so the disk becomes the limit long
before the match test does. The threshold saves the write, not the scoring.

## 5. The total is a sort key, not a probability

The families overlap. A run of three identical symbols *is* a palindrome of
three, so to add their rarities counts one event twice. The search also takes
whichever feature happens to be present, which is the multiple-comparisons
mistake in its usual form.

The calibration shows how much this matters:

| Score | Measured share | One address in | A sum of bits would say |
|---:|---:|---:|---:|
| 2 | 32% | 3 | 4 |
| 8 | 2.1% | 47 | 256 |
| 12 | 0.40% | 252 | 4 096 |
| 16 | 0.059% | 1 704 | 65 536 |
| 20 | 0.0087% | 11 442 | 1 048 576 |
| 24 | 0.00094% | 106 383 | 16 777 216 |

The sum overstates rarity by nearly a hundred times. The program therefore shows
the measured share and not the sum. The overlap and the multiple comparisons are
already inside the measurement.

We took the table over five million addresses and checked it on five million
more that it had not seen. All thirteen cuts agreed inside three standard
errors. Past the last measured point the answer saturates: the sample held ten
addresses above it, which is enough to say "rarer than this" and not enough to
say how much rarer.

## 6. What a run looks like

```text
$ onion-gen -F contains:abc --min-score 8 -n 5
ieefv4aao2bt2ms7aj6hylmkhf6fmdmdwyggchxc7cxyabcc65yng3yd.onion
onion-gen: score 9.1 (few digits 3.3, placement 5.8), about one address in 74
...
onion-gen: 5 hit(s) from 75776 candidates (0 on the device)
onion-gen: 70 find(s) scored below the threshold and were not written
onion-gen: best find: oj33abc… at 12.3, about one address in 301
```

Three decisions are worth a name.

**The threshold is off by default.** The cost of a mistake is not symmetrical.
You can delete a spare directory. A key that the program did not write is gone,
because the same address will not come up twice.

**The program counts rejected finds apart from kept finds.** Otherwise `--limit
5 --min-score 20` would stop after five rejections and write nothing.

**The program names the best find at the end.** It never reorders the output.
Finds go out as they arrive, because to hold them back for the sake of order
would lose all of them on an interrupt. The best find can therefore be anywhere
in a long list.

## 7. How this is tested

Scoring runs only on a find, so the test is the claim that the search loop is
untouched. The run gets filters long enough that it finds nothing, and any
difference is then the hot loop and nothing else. The test measures four forms:
a prefix, a substring, a suffix and a regular expression.

The independent verifier and tor with `DisableNetwork 1` check the keys that
pass the threshold. The network stays off, and the program publishes nothing.

## 8. What is not done, and why

- **No dictionary words.** That needs a word list, and the choice of list is a
  separate question with a different answer for each language. The user already
  has a filter for the words that they want.
- **No reordered output.** Finds arrive in a stream. To collect and sort them
  would lose all of them on an interrupt.
- **Scoring does not know about the filter.** The program passes the placement
  in as a number. Scoring that worked the placement out for itself would need
  the packed bytes and the printed address, which are both `&[u8]`. The compiler
  cannot tell them apart, so the boundary keeps them apart instead.
