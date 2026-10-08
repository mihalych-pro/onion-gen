# Regular expressions

**English** | [Русский](regex-matching.ru.md)

This document describes the sixth matching form and the structures that keep it
cheap.

## 1. The cost is the encoding, not the engine

The expensive part of this form is not the regular-expression engine. It is the
step that leaves the bit level for text. To produce the base32 encoding of a
candidate costs far more than any match test that works on the packed key.

The whole shape of the form follows from that. The goal is not a faster engine.
The goal is **to not reach the engine at all**.

## 2. Three paths, and what separates them

The program extracts from the expression the literals that every accepted
string must hold. It uses `regex_syntax::hir::literal::Extractor`, and it takes
a set of literals and not one literal:

```text
^abc                          → ["abc"]
^my(shop|store)               → ["myshop", "mystore"]
(cat|dog)food                 → ["catfood", "dogfood"]
^ab.*cd$                      → ["ab"]
^[bcdfghjklmnpqrstvwxyz]{6}   → none
```

Two independent questions follow.

**What rejects the candidate.** An anchored expression gives its literals as
prefixes, and the program compares them directly against the packed key with
nothing encoded. An unanchored expression gives substrings. They have no fixed
position to compare against, so the rejection happens over the text. An
expression with no obligatory literal has no rejection at all.

**Which text the engine runs over.** The engine runs over the 49 symbols that
the key gives, if the expression is anchored, bounded in length, and holds no
look-around other than `^`. Otherwise it runs over the full address, which costs
one `SHA3-256` for each candidate.

That last condition is a whitelist, and not a list of the dangerous
look-arounds. The key text is a prefix of the address, so `$` and `\b` read the
two differently. A list of dangerous constructs would put the burden on us to
think of all of them and to keep thinking of them. One that we failed to list
would not fail loudly. It would quietly run the engine over the wrong text.

## 3. Long literal lists get a bitmap

A set of literals above the threshold goes into a `BitmapIndex`, which is the
same structure that a dictionary of prefixes uses. There is a second caller and
not a second implementation.

The threshold is 16 literals, in `REGEX_BITMAP_FROM`. Below it the direct
comparison wins, because a few byte comparisons against a key that is already in
a register beat a load from a table that is not.

An expression that carries an obligatory literal therefore costs about what a
prefix costs, and that holds even when the expression reaches into the checksum.
Exactly two cases stay expensive, and both are expensive by nature:

- an expression with no literal, because the engine then sees every candidate;
- an unanchored expression, because a substring has no fixed position to reject
  on.

## 4. Correctness: the ground truth is the expression itself

The speed of this form rests on a risk. The program extracts a literal from the
expression and rejects the candidate by it before the engine runs. A mistake in
that extraction does not show up as a slowdown. It shows up as keys that the
program silently never finds: the rate does not change, the output is merely
emptier than it should be, and the only way to notice is to suspect it already.

The ground truth is therefore not a second implementation of ours. It is the
expression itself, applied to the finished address, in
`tests/regex_prefilter_is_sound.rs`. There are two tests:

- the **contract** test reads `Literals` naively and checks that the promise
  holds, over 41 expressions and 1 000 000 addresses;
- the **product** test runs the same set through the matcher with all its
  shortcuts: literals read off the bits, the shared prefix, and the decision
  made from 49 symbols.

Both tests run against deliberately broken code, and they catch it:

| What is broken | Missed | Invented |
|---|---:|---:|
| the literal not tied to a position | 47 110 | 0 |
| the look-around whitelist removed | 0 | 44 |
| the shared prefix computed wrongly | 6 726 | 0 |
| literal rejection before the address removed | 3 404 | 0 |

Two properties of that set are deliberate. It holds expressions that strike the
boundary between the key text and the full address, because a set without them
cannot see a whitelist that has been removed. It also plants the sought words
into the keys, because `^shop.*` occurs about once in a million addresses, and a
broken rejection would otherwise give a signal that chance explains just as
well.

## 5. The test must go through the real entry point

`has_patterns()` decides which loop a set of filters runs in. A set that holds
an expression must not go into the loop written for literal prefixes, because
that loop asks only for the byte comparison.

A test that calls the matching entry points directly cannot see an error here.
The choice of loop happens once for each batch, and it is outside the matching.
`tests/regex_matching.rs` therefore goes through `run::run`, which is the entry
point that the binary itself uses.

## 6. The device barely helps an expression

The program hands the device one structure: the prefix bitmap, which is the only
one with no pointers in it. The device rejects candidates against that bitmap.

An expression has nothing to hand over. The host therefore examines everything
that the device produces, and the host becomes the limit. In this respect an
expression behaves like `contains:` and not like a prefix, although it is faster
than `contains:` on the processor.

## 7. What is not done, and why

- **Reachability is not decided in full.** The program refuses an obligatory
  literal outside base32, but only when every branch is unreachable, so
  `^(abc|0xy)` survives. It also refuses a required length above 56 and an
  expression that accepts nothing. Everything else goes to the search. Here is a
  live example of the boundary: `regex:^[a-z]{4}2.*7d$` is unreachable, because
  the protocol allows only `{a,i,q,y}` at position 54, and the cheap check does
  not catch that.
- **The "1.5x over a thousand filters" limit does not cover expressions.** A
  dictionary of a thousand regular expressions is not the case that this form
  exists for, and the diagnostics say so instead of leaving it implied.
- **Unicode is off.** The haystack is 56 bytes of base32, so `.` means one
  symbol and `\w` is ASCII. With Unicode on, both mean something else, and the
  difference would surface as a filter that quietly matches the wrong thing.
- **For a set that holds an expression, the wait is a bound and not a figure.**
  The share of addresses that an expression matches does not follow from its
  text. The program keeps "cannot be estimated" apart from "will never match",
  because those are opposite pieces of news.

## 8. How the acceptance script treats this form

`scripts/bench/run-form-acceptance.sh` measures three rows for expressions and
not one: `regex-bits`, `regex-address` and `regex-open`. They are the three
paths of section 2. One figure for "regex" would average three different costs
into a number that describes none of them.
