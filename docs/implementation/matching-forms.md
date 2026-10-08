# Matching forms

**English** | [Русский](matching-forms.ru.md)

Matching forms beyond a literal prefix: substring, suffix, positional wildcards
and character classes.

## 1. Throughput per form

Apple M-series, 8 threads, NEON. Each filter is long enough that no hit lands
during the run, so every candidate walks the whole matcher. Best of three runs,
taken alternately in one sitting.

| Form | Filter | Throughput | Relative to prefix |
| --- | --- | --- | --- |
| prefix | `abcdefghij` | 51.9 M/s | 1.00x |
| wildcard | `a?cdefghij` | 40.8 M/s | 1.27x slower |
| character class | `a[b-e]cdefghij` | 39.1 M/s | 1.33x slower |
| suffix, 14 symbols | `suffix:abcdefghijkzad` | 30.5 M/s | 1.70x slower |
| substring | `contains:abcdefghij` | 25.7 M/s | 2.02x slower |
| suffix, 5 symbols | `suffix:aazad` | 10.5 M/s | 4.96x slower |

Two numbers explain the table.

A **wildcard or class** costs about a quarter. Both leave the byte-comparison
path — a fixed prefix is compared as bytes with one masked byte at the end,
while these walk symbol by symbol — but they are checked at one offset only.

A **substring** costs twice, and did cost three and a half times until section
8 measured why. It is not checked offset by offset over packed bits any more:
the candidate is encoded into a stack buffer once and the text is searched.
Section 9 records what that changed.

## 2. What the checksum costs

The first 51 symbols of an address are a function of the public key alone. Past
them each symbol depends on `SHA3-256(".onion checksum" || pubkey || 0x03)`, so
any candidate that cannot be ruled out from the key costs a hash.

The key prefilter is what keeps that rare. Each placement of the form is checked
as far as the key reaches; only a placement that survives is worth hashing. A
14-symbol suffix starts at symbol 42, so eight of its symbols lie inside the key
and reject all but one candidate in `32^8`. A 5-symbol suffix starts at symbol
51 and has nothing inside the key, so every candidate is hashed.

That is the whole difference between the last two rows of the table: 32.2 M/s
against 10.3 M/s, same form, same code.

| Field arithmetic | Prefix | Suffix needing the checksum | Ratio |
| --- | --- | --- | --- |
| NEON | 55.4 M/s | 10.4 M/s | 5.34x |
| scalar | 40.7 M/s | 10.6 M/s | 3.85x |

The design predicted 3.1x on ARM. The measured ratio is higher, and the table
says why: the checksum path runs at the same absolute speed either way, because
Keccak is scalar in both. Only the baseline moved. The prediction was made
against the scalar engine, where the measured figure is 3.85x; vectorising the
field arithmetic made key generation 36% cheaper and the ratio correspondingly
worse. The number that matters for a user is the absolute one, and it did not
change.

## 3. Vectorised Keccak: not built

The checksum path caps at about 10.4 M/s whatever the field arithmetic does, so
a four-lane Keccak is the obvious next lever. It was not built, because the
regime it would help is one where the search is already over.

A form is capped at 10.4 M/s only when nothing of it reaches into the key —
which, for a suffix, means five symbols or fewer. The last symbol of an address
is always `d` and the one before it is one of four, so a five-symbol suffix
carries 17 bits: one address in 131 072, found in hundredths of a second. A
suffix long enough to be worth waiting for is long enough to be prefiltered.

The decision is reversible and costs nothing to revisit: the hash sits behind
one call in `scan_batch`.

## 4. Cost to searches that use none of this

A set of literal prefixes must cost what it cost before these forms existed.
Two things were needed to keep that true.

The choice of matching pass is made **once per batch**, not once per candidate.
Folding the passes into one loop with a test inside cost about 1% on dictionary
search — small, but it was measurable and it was free to avoid. Dictionary
search is where it showed up: the bitmap rejects almost every candidate, so
there is little else per candidate for an extra field read to hide behind.

The filter's representative text moved into the boxed `Pattern`. Nothing in the
hot loop reads it, and as a `String` in `Filter` it was 24 bytes of every entry a
dictionary scan walks past. `Filter` is now 40 bytes, against 56 before the
change began.

Measured against the commit preceding these forms, alternately, eight pairs, a
thousand-word dictionary: median 54.78 M/s before, 54.27 M/s after, ratio 0.991,
with three of eight pairs favouring the new build. A single prefix measures
identical. The residual is within run-to-run drift.

## 5. Character class ranges

`[b-e]` is `[bcde]`. Ranges exist because the useful ones, `a-z` and `2-7`, are
tedious to spell and easy to get wrong. The alphabet runs `a`–`z` then `2`–`7`,
so `0`, `1`, `8` and `9` are not symbols at all and a range cannot produce them;
`[0-9]` is rejected by name, and a range whose end precedes its start is rejected
as the typo it almost certainly is.

## 6. Found keys land in `./keys`

A hit becomes a directory named after its address, and an easy filter produces
them by the thousand — a twenty-second measurement of `suffix:zad`, which matches
one candidate in 128, wrote 9 429 of them. They go under `./keys` unless `-d` says
otherwise, so a run started anywhere does not bury the working directory.

An easy filter is also where throughput stops meaning anything: `suffix:zad`
reports 27 600 candidates per second, not because matching is slow but because
every 128th candidate is written to disk under the writer lock, at roughly 216
hits per second. The search is bound by reporting, not by searching.

## 7. Many filters of one general form: linear, and it shows

The bitmap index and the sorted length buckets are built over literal prefixes
only. A general form is checked by walking the list. Measured on the same
machine, filters long enough that no hit lands:

| Form | 1 | 10 | 100 | 1000 | Drop |
| --- | --- | --- | --- | --- | --- |
| prefix | 43.9 M/s | 53.0 M/s | 53.7 M/s | 53.4 M/s | none |
| wildcard | 43.2 M/s | 12.5 M/s | 2.3 M/s | 0.20 M/s | 183x |
| character class | 39.7 M/s | 12.6 M/s | 2.0 M/s | 0.20 M/s | 169x |
| suffix | 31.8 M/s | 6.4 M/s | 0.9 M/s | 0.10 M/s | 287x |
| substring | 12.9 M/s | 1.9 M/s | 0.20 M/s | 0.024 M/s | 526x |

**The 1.5x tolerance is not met, and not nearly.** Every general form scales
linearly with the filter count, so a thousand of them costs a thousand times one
of them. A prefix dictionary of the same size costs nothing, which is the whole
point of the bitmap: it is the difference between a structure and a loop.

This answers the question of which forms the bitmap serves. It serves a literal
prefix and nothing else, and it is already switched off for the rest — a set
holding only general forms builds no index and says `index disabled`. In a mixed
set it is built over the prefixes and consulted only on their path, so the
correctness of the general forms does not depend on it.

Making it serve the others is a different structure for each, not a setting:

- A **start-anchored wildcard or class** could be indexed by enumerating what
  its free positions can hold within the indexed span — 32 entries per wildcard
  symbol, a handful per class — and falling back to no index when that count
  runs away.
- A **suffix** is anchored too, just at the other end: the same bitmap taken at
  the fixed bit offset where its key-readable part begins would work unchanged.
- A **substring** has no fixed offset at all, so no bitmap over a fixed span can
  help. It needs something else — a set keyed on a window of symbols, probed
  once per offset, so that the cost follows the address length rather than the
  number of filters.

None of this is built. It is recorded here as the measured shape of the problem;
a set of general forms is usable in the tens, not in the thousands.

## 8. Regular expressions: what they would cost us

Measured by `benches/regex-cost.rs` with the `regex` crate, single thread, best
of three, against the same candidates. The reference numbers to compare with are
in 

**How many expressions could be prefiltered.** Of sixteen expressions of the kind
a person writes when hunting a vanity address, ten — 62% — carry a literal that
every match must contain. The six that do not are alternations with no common
part (`^(shop|store)`), and expressions built entirely from classes
(`^s[a-z]op`, `^[bcdfghjklmnpqrstvwxyz]{6}`).

**What a candidate costs.**

| Step | Cost | Note |
| --- | --- | --- |
| our prefix matcher, by bits | 2.4 ns | what we do today |
| our substring matcher, by bits | 128.3 ns | 52 offsets, walked |
| base32 encoding alone | 37.5 ns | into a stack buffer |
| encode + literal search | 46.5 ns | |
| encode + regex, no literal | 54.0 ns | the worst case |
| encode + regex, with literal | 46.9 ns | |
| our substring prefilter, then regex | 130.0 ns | |

Three things follow.

**Prefiltering a regex with our substring matcher makes it worse, not better.**
130 ns against 46.9 ns for the plain regex. The plan assumed our matcher would
be the cheap thing in front of an expensive engine; it is the expensive thing.
And the prefilter is redundant besides: the `regex` crate extracts the literal
itself and scans for it with the same machinery, which is exactly why "encode +
regex, with literal" costs what "encode + literal search" costs.

**Our own substring matching is 2.8x more expensive than it needs to be.**
Walking 52 offsets over packed bits costs 128 ns; encoding once and searching
the text costs 46.5 ns. This is not about regex at all — it is a finding about
the substring form we just built, and it is not acted on here.

**Regex itself is affordable.** The engine's budget is 153 ns per candidate per
thread (52.3 M/s across eight). Replacing prefix matching with the worst-case
expression costs `153 − 2.4 + 54 = 205 ns`, which is 39.1 M/s — a 1.34x drop.
The reference pays between 1.25x (literal) and 2.13x (quantifiers) for the same
flexibility, and pays it from a much lower base: 10.2 M/s for a substring
expression against our 39.1. We would be roughly 3.8x faster while offering the
same thing.

**Decision.** Regex is worth having and belongs in a change of its own. It needs
a dependency, a syntax decision about how expressions sit alongside the existing
forms, and an answer to what happens past symbol 51 where the checksum begins.
That is a form of its own. What the numbers settle is that it is not
a performance question: the cost is 1.34x and the result still beats the
reference's regex several times over. The design point to carry forward is that
the engine does its own literal prefiltering, so nothing should be built in
front of it.

## 9. Substring matching rebuilt on text search

Section 8 measured our substring matcher at 128 ns a candidate against 46 ns for
encoding once and searching the text. Acted on:

- A literal substring is matched by `memchr::memmem` over the encoded candidate.
  The encoding is paid once however many substrings the set holds; a set with
  none never allocates the buffer at all.
- The checksum prefilter stopped re-examining placements that fit inside the
  key. Those were already settled by the key pass, and walking them again meant
  a substring paid for all 47 of its offsets twice per candidate. This was the
  larger half of the cost.

Measured against the commit before these forms, alternately, best of two:

| Set | Before | After | Gain |
| --- | --- | --- | --- |
| one substring | 12.3 M/s | 22.8 M/s | 1.85x |
| ten | 1.85 M/s | 4.00 M/s | 2.16x |
| a hundred | 0.18 M/s | 0.63 M/s | 3.62x |
| a thousand | 0.024 M/s | 0.067 M/s | 2.79x |

The linear growth of section 7 is unchanged — this lowers the constant, it does
not replace the missing structure. A thousand substrings is still a thousand
searches. What it does change is that each is a text search over 51 bytes rather
than a walk over 47 bit offsets.

A substring holding a class or a wildcard keeps the old path: there is no text to
hand a literal searcher. Anchored forms keep it too, because they are checked at
one offset and never needed a search.

Prefix and suffix are unaffected, which was the requirement: paired
measurements put a character class at 0.984 with three of six pairs favouring
the new build, and a 14-symbol suffix at 0.96 to 0.98.

## 10. Acceptance

**Every form finds addresses tor agrees with.** One key per form was given to
tor 0.4.9.12 with `DisableNetwork 1`; tor regenerated the hostname from the
secret key alone and produced exactly the address the generator had reported, for
prefix, substring, suffix, wildcard and class alike. The network stayed off
throughout: the service was never published, which is a deliberate act rather
than a test step. `scripts/verify/verify-with-tor.sh` does this for any key
directory.

**Both architectures agree.** Built and tested natively on `linux/amd64`
(Xeon E5-2683 v4, Debian 6.12, rustc 1.85.1): 107 tests pass, AVX2 is selected,
and all five forms find and verify exactly as they do on ARM.

**Throughput, both machines, one sitting**, one filter, eight threads, median of
five samples:

| Form | Apple M-series, NEON | Xeon E5-2683 v4, AVX2 |
| --- | --- | --- |
| prefix | 55.2 M/s | 30.6 M/s |
| wildcard | 41.3 M/s (1.34x) | 27.6 M/s (1.11x) |
| character class | 39.9 M/s (1.38x) | 27.7 M/s (1.10x) |
| suffix | 32.4 M/s (1.70x) | 25.5 M/s (1.20x) |
| substring | 24.1 M/s (2.29x) | 16.0 M/s (1.91x) |

The forms cost noticeably less on x86, relative to prefix, than on ARM. Nothing
about matching changed between them: the base did. Key generation is 1.8x slower
on that Xeon, so matching is a smaller share of each candidate, and every ratio
shrinks towards one. The absolute cost of a form is the portable number; the
ratio is a property of the machine it was measured on.

**What the forms are worth, measured.** Hit frequency observed over hundreds of
millions of candidates, against the gain calculated in


| Form | Observed gain | Calculated | Equiv. symbols | Calculated |
| --- | --- | --- | --- | --- |
| prefix (control) | 1.0x | 1x | 0.00 | 0.00 |
| substring, 5 symbols | 52.1x | 48x | 1.14 | 1.12 |
| suffix, 6 symbols | 236.8x | 256x | 1.58 | 1.60 |
| wildcard, 2 free positions | 1045x | 1024x | 2.01 | 2.00 |
| class, 2 symbols at 2 positions | 4.0x | 4x | 0.40 | 0.40 |

Theory and practice agree to within the sampling error everywhere. The substring
comes out slightly above its calculated gain because the analysis counted
offsets conservatively; the suffix slightly below on 143 hits, which is under one
standard deviation.

## 11. Substring dictionaries, and what the measurements cost to get right

A thousand substrings ran at 0.07 M/s. They now run at 8.75 M/s — **127x** —
and a single substring is unchanged. Three attempts were needed, and the first
two taught more than the third.

**An Aho-Corasick automaton on its own changed nothing.** The linear cost simply
moved: every substring reaches the checksum at its last few offsets, so the
per-candidate checksum prefilter walked all thousand filters instead. That
prefilter exists to save a SHA3-256 worth about 200 ns and was costing tens of
microseconds — fifty times what it saved.

**Dropping it in favour of walking the whole address gave 44x**, at the price of
a hash for every candidate.

**A tail prefilter removed that too.** A substring that begins inside the key and
runs into the checksum is visible from the key alone: it is enough to ask
whether the key text's tail begins any needle. A small bitmap with a cheap hash
answers that in a few nanoseconds, flat in the number of filters. It may answer
"maybe" wrongly, never "no" — a false positive costs one unnecessary hash, a
false negative would silently lose addresses. A differential test over 40 000
keys checks the automaton path against a naive search of the whole address and
insists that tail matches actually occur in the sample.

Two settings of the automaton are not the defaults and both matter. The crate's
prefilter is switched **off**: the address alphabet is 32 symbols over 51 bytes,
no byte is rare, and the prefilter fires on nearly every candidate — 424 ns
against 197 without it. The kind is pinned to **DFA**: it is fastest at every
size measured, though it is ten times larger than the contiguous NFA, which is
twice as slow. The crossover from one search per filter to the automaton is at
**ten filters**, measured.

### What did not work, and what that cost

Two structural "improvements" were tried on the theory that the hot `FilterSet`
had grown: boxing the substring search out of it, and replacing a second index
vector with an ordered prefix and a count. The second made prefix-only search
**6% slower** — the opposite of its prediction — and was reverted.

The remaining difference on prefix-only search is 1.4–2.3%, consistently in one
direction (the new binary lost 8 pairs out of 8). It is **not extra work**:
measured with `perf`, both binaries retire the same instructions per candidate,
1590 against 1595, and differ only in IPC, 2.39 against 2.42. Same instructions,
slightly worse throughput, is the signature of code layout — where the linker
put the hot loop — rather than of anything the matcher does.

The lesson worth keeping: a single alternating pair cannot resolve a few
percent on this machine. It took eight pairs to see a consistent sign, and a
hardware counter to learn that the sign meant nothing about the work being done.
