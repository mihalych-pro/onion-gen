# Regular expressions

**English** | [Русский](regex-matching.ru.md)

The sixth matching form.

## 1. The cost was not where it was expected

The form sat in the plan marked expensive: the earlier estimate said it would
cost down to 20% of throughput. A measurement taken before any work began
(`benches/regex-cost.rs`, M1 Pro, one thread, section 2) said otherwise:

| what is measured | ns/candidate |
|---|---:|
| our prefix matcher, on bits | 4.5 |
| **the base32 encoding alone** | **40.7** |
| encode + literal search | 50.5 |
| encode + regex with a literal | 50.8 |
| our substring matcher (`contains:`) | 50.1 |
| encode + regex without a literal | 56.4 |

What costs is not the regex engine but leaving the bit level for text: 41 ns of
51. That is the whole shape of the form — not "make the engine faster" but
**do not reach it**.

## 2. Three paths, and what separates them

Literals obligatory for every accepted string are extracted from the expression
(`regex_syntax::hir::literal::Extractor`). Not one literal but a set of them:

```
^abc                          → ["abc"]
^my(shop|store)               → ["myshop", "mystore"]
(cat|dog)food                 → ["catfood", "dogfood"]
^ab.*cd$                      → ["ab"]
^[bcdfghjklmnpqrstvwxyz]{6}   → none
```

Two independent questions follow.

**What rejects the candidate.** An anchored expression gives its literals as
prefixes, compared directly against the packed key with nothing encoded. An
unanchored one gives substrings: there is no fixed position to compare against,
so the rejection happens over the text. With no obligatory literal there is no
rejection at all.

**Which text the engine runs over.** The 49 symbols readable from the key, if
the expression is anchored, bounded in length, and contains no look-around
other than `^`. Otherwise the full address, which costs a `SHA3-256` per
candidate.

That last condition is a whitelist rather than a list of the dangerous
look-arounds. The key text is a prefix of the address, so `$` and `\b` read the
two differently; enumerating the dangerous ones would put the burden on us to
have thought of all of them and to keep thinking of them, and one we failed to
list would not fail loudly — it would quietly run the engine over the wrong
text.

## 3. What it costs in the product

Section 3 of the same example, same thread, through `FilterSet::match_key`:

| set | ns/candidate |
|---|---:|
| a ten-symbol prefix, the floor | 4.6 |
| `regex:^abcdefghij`, the same ten symbols | 6.5 |
| `regex:^shop` | 6.5 |
| `regex:^my(shop|store)` | 6.8 |
| `regex:^a[2-7]{3}shop`, 216 literals with a shared head | 11.3 |
| `regex:^[2-7]{3}shop`, 216 literals sharing nothing | 11.5 |
| `regex:^[bcdfghjklmnp]{4}`, no literal | 65.8 |

Section 4, the forms that need the checksum, through `examine_key` +
`match_whole_address`:

| set | ns/candidate |
|---|---:|
| `regex:^shop.*` | 9.6 |
| `regex:^zz.*qd$` | 10.2 |
| `regex:zz[2-7]z`, a substring | 691 |

So an expression carrying an obligatory literal costs on the order of a prefix
rather than on the order of text — and that holds even when it reaches into the
checksum. Exactly two cases stay expensive, and both are expensive by nature:
an expression with no literal (the engine sees every candidate) and an
unanchored one (a substring has no fixed position to reject on).

### How these numbers were reached

The first honest measurement read 45.7 ns where it now reads 6.5. Three losses,
all in this form's own wiring, and none of them findable by reading the code:

1. **base32 was encoded before the literal was checked** — paying exactly the
   41 ns the literal exists to avoid. 45.7 → 5.9.
2. **A form reaching the checksum was never asked about its literals at all**,
   although they sit at the front where the key settles them, so the address
   was built for every candidate. `^shop.*` 346.8 → 9.6, `^zz.*qd$`
   351.7 → 10.2.
3. **`examine_key` encoded the text even where nothing reads it.**

Long literal lists get a bitmap — the same `BitmapIndex` a dictionary of
prefixes uses, with a second caller rather than a second implementation. The
threshold is 16 literals; below it the direct comparison wins, because a few
byte comparisons against a key already in a register beat a load from a table
that is not. Without the bitmap `^[2-7]{3}shop` cost 237.9 ns.

## 4. Correctness: why the comparison is not against ourselves

The speed-up here is built on a risk. A literal is extracted from the
expression, the candidate is rejected by it before the engine runs, and a
mistake in the extraction shows up not as a slowdown but as keys silently never
found: the rate is unchanged, the output is merely emptier than it should be,
and the only way to notice is to already suspect it.

So the ground truth is not a second implementation of ours but the expression
itself, applied to the finished address (`tests/regex_prefilter_is_sound.rs`).
Two tests:

- the **contract** test reads `Literals` naively and checks the promise is
  kept: 41 expressions × 1 000 000 addresses;
- the **product** test runs the same set through the matcher with all its
  shortcuts — literals read off the bits, the shared prefix, the decision made
  from 49 symbols.

Both were checked against deliberately broken code, and that was not a
formality:

| what was broken | missed | invented |
|---|---:|---:|
| the literal not tied to a position | 47 110 | 0 |
| the look-around whitelist removed | 0 | 44 |
| the shared prefix computed wrongly | 6 726 | 0 |
| literal rejection before the address removed | 3 404 | 0 |

The first two attempts were **not caught**: the set held no expression striking
the boundary between the key text and the full address, and six had to be
added. The fourth was first caught by one match out of one — `^shop.*` occurs
once in a million addresses, so over 200 000 keys a broken rejection produced a
signal indistinguishable from chance. Sought words are now planted into the
keys, and the count became 3 404 of 3 404.

## 5. What turned up outside the matcher

`has_patterns()` did not know about expressions. A set holding one was routed
into the loop written for literal prefixes, which asks only the byte
comparison: **a run over 3.6 billion candidates found nothing where it should
have found some 660 000.**

A test calling the matching entry points directly cannot see this — the defect
lives not in the matching but in the choice of loop, made once per batch. Hence
`tests/regex_matching.rs`, which goes through `run::run`, the same entry point
the binary uses. With the defect reintroduced, three of its five tests fail.

## 6. The device barely helps an expression

Measured on M1 Pro, eight threads:

| form | `--compute cpu` | `--compute auto` | gain from the card |
|---|---:|---:|---:|
| `abcdefghij` | 31.0 M/s | 144.5 M/s | **4.7×** |
| `regex:^abcdefghij` | 19.2 M/s | 25.9 M/s | 1.35× |
| `contains:abcdefghij` | 12.0 M/s | 17.2 M/s | 1.43× |

The reason is architectural and not new: what is handed to the card is the
prefix bitmap — the one structure with no pointers in it — and it rejects
candidates there. For the other forms there is nothing to hand over, the host
examines everything the card produces, and the host becomes the bottleneck. In
this respect an expression behaves like `contains:` rather than like a prefix,
while being 1.6× faster than `contains:` on the processor.

The gap between the microbenchmark (6.5 against 4.6 ns, a factor of 1.4) and
the full processor run (19.2 against 31.0 M/s, a factor of 1.6) remains
unexplained. It is recorded as measured, without a guess at the cause.

## 7. The other forms did not slow down

A paired measurement against the commit before this form existed (`46b1a58`),
alternating rather than one binary's block followed by the other's, because the
machine drifts and a block order would attribute that drift to the code. M1 Pro,
eight threads, `--compute cpu`, medians.

| form | before | after | ratio | pairs |
|---|---:|---:|---:|---:|
| prefix `abcdefghij` | 34.1 M/s | 33.8 M/s | 0.990 | 8 |
| suffix `suffix:abcdefghijkzad` | 28.9 M/s | 28.4 M/s | 0.982 | 8 |
| substring `contains:abcdefghij` | 16.4 M/s | 16.0 M/s | 0.977 | 12 |
| class `a[b-e]cdefghij` | 39.1 M/s | 38.5 M/s | 0.986 | 12 |

Substring and class first came out at 0.934 and 0.911 over eight pairs, which
looked like a regression. Over twelve they came to 0.977 and 0.986. What makes
the first result noise is not that it improved but the spread: for the class the
spread within one series was 32.9–40.9 M/s, that is 24%, and a median of eight
samples from such a distribution drifts by a tenth easily. On top of that
**both** binaries ran faster in the second series — the machine was not the same
between them, which is itself the argument for alternating.

`scripts/bench/run-form-acceptance.sh` was repaired along the way: it passed the
filter positionally, a form that disappeared when the CLI moved to flags, so the
script had been failing on every run. Three rows were added for expressions, one
per path, because a single "regex" figure would average three different costs
into one describing none.

## 8. What is not done, and why

- **Reachability is not decided in full.** Refused: an obligatory literal
  outside base32 (only when every branch is unreachable — `^(abc|0xy)`
  survives), a required length above 56, an expression accepting nothing. The
  rest goes to the search. A live example of the boundary:
  `regex:^[a-z]{4}2.*7d$` is unreachable because the protocol allows only
  `{a,i,q,y}` at position 54, and the cheap check does not catch it.
- **The "1.5× over a thousand filters" limit does not cover expressions.** A
  dictionary of a thousand regular expressions is not the case the form exists
  for, and the diagnostics say so rather than leaving it implied.
- **Unicode is off.** The haystack is 56 bytes of base32, so `.` means one
  symbol and `\w` is ASCII. With Unicode on both mean something else, and the
  difference would surface as a filter quietly matching the wrong thing.
- **For a set holding an expression the wait is a bound, not a figure.** What
  share of addresses an expression matches does not follow from its text.
  "Cannot be estimated" is kept apart from "will never match": those are
  opposite pieces of news.
