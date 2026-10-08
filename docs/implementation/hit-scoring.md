# Scoring finds

**English** | [Русский](hit-scoring.ru.md)

Scoring finds.

## 1. Why rarity rather than taste

A broad filter produces finds in a stream: `contains:abc` is one find in 630
candidates, which over a run is thousands of directories. Which of them is
better can be argued about indefinitely, so the question was replaced with one
that can be measured: **"prettier" means "rarer"**.

The replacement is not cosmetic. It lets a feature fail to qualify — and one
did, as below.

## 2. The features and their measured rarity

Five million random addresses produced by the same encoder the product uses, so
the checksum and the fixed tail are real (`benches/score-calibration.rs`,
section 1). The last two symbols of an address are fixed by the protocol and
take no part in any feature.

| family | level | share of addresses | bits |
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

Levels past the measured range are extrapolated only where the slope is a fact
rather than a curve fit. One more symbol in a run is one more symbol that had
to come up the same, so 32 times rarer, so five more bits; the measured steps
are +5.0 and +4.9, which is the alphabet saying so rather than a fit. A tiling
unit is two symbols, hence ten. Palindromes have no such slope — odd and even
lengths behave differently — so that family saturates at its last measured
level and says so, instead of inventing a number.

### The feature that did not qualify

"An address without digits reads better" is true, and worth 15.9 bits. "A long
stretch of letters" sounds like the same claim; the measurement says otherwise:

| letters-only run | share of addresses |
|---|---:|
| 10+ | 79% |
| 12+ | 60% |
| 14+ | 42% |
| 16+ | 28% |
| 20+ | 11% |

Up to 14 symbols the feature describes most addresses rather than the good
ones. Above 16 it is no longer too common — and there the second reason
decides: it is not a second feature but the same one. The stretch and the digit
count are two views of the same sparsity, and keeping both would count it
twice. The count is kept because it grades further: 1.8 to 15.9 bits against
1.9 to 3.1.

## 3. Placement is the only part that knows what was asked for

For a form that could match in several places the value of where it landed is
derived rather than measured: a form with `p` placements lands at or before
offset `k` in about `(k + 1) / p` of the addresses it matches at all, so
landing at the very front is worth `log2(p)` bits. For `contains:abc` that is
5.6 bits at the first symbol and 0.03 at the last.

A form anchored to the start has one placement and contributes nothing, which
is correct rather than a special case.

## 4. What it costs

| | ns |
|---|---:|
| run of identical symbols | 33 |
| two-symbol tiling | 97 |
| palindrome | 111 |
| digit count | 10 |
| **all together** | **254** |
| **writing the key to disk** | **5 400 000** |

The second number is what to compare against, not zero: scoring is 0.005% of
what a find already spends. The palindrome scan is quadratic and was the only
candidate for removal on cost; at these numbers the question does not arise.

The disk figure deserves attention of its own: **185 keys per second**. With a
three-symbol filter at 150 M/s, finds arrive at 4 600 a second, so the disk
becomes the bottleneck twenty-five times sooner than the matcher does. That is
what makes the write threshold useful — it saves the write, not the scoring.

## 5. Why the total is a sort key and not a probability

The families overlap: a run of three identical symbols **is** a palindrome of
three. Adding their rarities counts one event twice. On top of that the search
is for whichever feature happens to be present, which is the
multiple-comparisons mistake in its usual form.

How much this matters is visible in the calibration itself:

| score | measured share | one address in | "bits" would say |
|---:|---:|---:|---:|
| 2 | 32% | 3 | 4 |
| 8 | 2.1% | 47 | 256 |
| 12 | 0.40% | 252 | 4 096 |
| 16 | 0.059% | 1 704 | 65 536 |
| 20 | 0.0087% | **11 442** | **1 048 576** |
| 24 | 0.00094% | 106 383 | 16 777 216 |

The sum overstates rarity by nearly a hundredfold. So what is shown to anyone
is not the sum but the measured share: the overlap and the multiple comparisons
are inside the measurement already.

The table was taken over five million addresses and checked on five million it
had not seen. All thirteen cuts agreed within three standard errors. Past the
last measured point the answer saturates: the sample held ten addresses above
it, which is enough to say "rarer than this" and not enough to say how much.

## 6. What changed about a run

```
$ onion-gen -F contains:abc --min-score 8 -n 5
ieefv4aao2bt2ms7aj6hylmkhf6fmdmdwyggchxc7cxyabcc65yng3yd.onion
onion-gen: score 9.1 (few digits 3.3, placement 5.8), about one address in 74
...
onion-gen: 5 hit(s) from 75776 candidates
onion-gen: 70 find(s) scored below the threshold and were not written
onion-gen: best find: oj33abc… at 12.3, about one address in 301
```

Three decisions are worth naming.

**The threshold is off by default.** The cost of being wrong is asymmetric: a
spare directory can be deleted, a key that was not written is gone for good —
the same address will not turn up twice.

**Finds turned away are counted apart from finds kept.** Otherwise `--limit 5
--min-score 20` would stop after five rejections and write nothing.

**The best find is named at the end.** The output is never reordered — finds go
out as they arrive, and holding them back for the sake of order would lose them
all on an interrupt — so the best one can be anywhere in a long list.

## 7. Acceptance

Scoring runs only on finds, so what is tested is the claim that the search loop
is untouched: the filters are long enough that nothing is found during a run,
and any difference is then the hot loop and nothing else. Alternating paired
measurement against the commit before scoring existed, M1 Pro, eight threads,
`--compute cpu`, median of ten pairs:

| form | before | after | ratio |
|---|---:|---:|---:|
| prefix `abcdefghij` | 39.1 M/s | 39.5 M/s | 1.012 |
| substring `contains:abcdefghij` | 18.0 M/s | 17.9 M/s | 0.998 |
| suffix `suffix:abcdefghijkzad` | 37.8 M/s | 38.3 M/s | 1.014 |
| regex `regex:^abcdefghij` | 33.3 M/s | 32.7 M/s | 0.980 |

The ratios fall on both sides of one, which is the result being looked for: had
the loop gained work, all four would have gone down.

Keys written through the threshold were checked by the independent verifier and
by tor itself with `DisableNetwork 1`: 5 of 5. The network stayed off and
nothing was published.

## 8. What is not done, and why

- **No dictionary words.** That needs a word list, and which one is a separate
  question with a different answer per language. The user already has a filter
  for saying which words they want.
- **The output is not reordered.** Finds arrive in a stream; collecting and
  sorting them would lose everything on an interrupt.
- **Scoring does not know about the filter.** The placement is passed in as a
  number. The first version worked it out itself and handed `match_offset` the
  printed address where it expects packed bytes: both sides are `&[u8]`, so the
  compiler said nothing and the placement contribution silently went to zero.
