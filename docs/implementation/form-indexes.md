# Indexes for the general forms

**English** | [Русский](form-indexes.ru.md)

These structures make a dictionary of substrings, wildcards, classes or suffixes
cost what a dictionary of prefixes costs.

The [README](../../README.md) gives the measured speeds.

## 1. The requirement

The specification asks that a dictionary of a thousand filters cost no more than
1.5 times what a single filter costs. Every form meets that target on both
architectures.

A prefix meets it through a bitmap index. The other forms need structures of
their own, because they are anchored differently.

## 2. Three structures, because the forms differ in anchor

**A literal substring gets an automaton.** Section 11 of
[matching-forms.md](matching-forms.md) records that story. The automaton alone
changed nothing until the checksum prefilter behind it stopped being linear, and
until a tail prefilter removed the hash from all but a few candidates. To find
where it starts to pay against one search for each filter, run `cargo bench
--bench substring-scaling`.

**A form anchored to the start gets the bitmap that a prefix gets.** The program
fills it by enumeration and not as a contiguous run, because the fixed bits of
such a form are scattered. One wildcard inside the indexed span is 32 slots. A
class of four symbols is four slots. Two wildcards are more than a thousand.

Past a budget the enumeration is not worth the time. The program leaves such a
form out of the index and checks it one by one. The index reports which forms it
covers, because a prefilter that quietly fails to cover something is not a
prefilter. It is a wrong answer.

**A suffix gets the same bitmap at the other end.** Its window sits at the end of
the span that the key derives, so a bit offset parameterises the index. Offset
zero is the same code that the prefix index has always used.

The suffix bitmap belongs in the checksum prefilter, and not in front of the
ordinary key-side check. A suffix ends at symbol 55, so its placement never fits
inside the key, and that check can only fail.

## 3. The index width is chosen, not fixed

A fixed width was wrong at both ends. The width now moves between 16 and 24
bits. Below 16 bits the map saturates faster than it saves. Above 24 bits the
map stops paying for the cache that it displaces.

The two index kinds choose the width differently, and the reason is in the
forms:

- **The prefix index counts the filters.** Each literal prefix occupies one
  slot, so the count is enough.
- **The general-form indexes build and measure.** One wildcard inside the span
  fills 32 slots and a class fills several, so the same count can mean very
  different occupancy. The program builds the index at each width in turn and
  takes the narrowest one whose occupancy is inside the target. This costs a few
  milliseconds once.

**The target rate is not the same for every index, and one rate for all three
was a mistake.** An index is worth only what stands behind it. Behind the prefix
index is a binary search over length buckets, so a false hit costs almost
nothing, and one false hit in a hundred is acceptable. Behind the pattern and
suffix indexes is a walk over every form in the group. With a thousand forms the
same false hit costs a thousand times more, so the tolerable rate falls in the
same proportion. The code divides the target by the number of forms behind the
walk.

## 4. Where the time goes now

For a prefix search, **the match test is no longer worth more work**. To remove
it completely would buy very little. The remaining lever is the field
arithmetic, and that is already close to what the hardware allows.

For a substring search the automaton takes the largest share, and the field
arithmetic comes second.

The group level has no margin either, which we checked and did not assume. A
chain step is 8 multiplications, the canonical count for extended twisted
Edwards coordinates. Montgomery batch inversion adds 3 more, plus 1 for the
coordinate itself. That is twelve for each candidate. The engine measures more
than twelve, and the difference is not extra work: it is the same work at a
higher cost for each multiply, because in the engine the multiply competes for
cache.

Two levers remain inside the field, and we recorded both instead of taking
them. Karatsuba over the ten limbs needs 75 partial products in place of 100, and
[speed.md](speed.md) records why it is still slower. AVX-512 IFMA has no
hardware here.

Libraries offer nothing for this shape of work. The AVX2 backend of
`curve25519-dalek` packs the limbs of one point across the lanes and unpacks
them on every multiply. We vectorise across four independent candidates with no
shuffles. `fiat-crypto` has no vector backend. The IFMA backend of dalek will be
a useful reference when there is a machine to run it on.

## 5. What remains linear

A substring that holds a class or a wildcard remains linear. It has no literal
for a searcher and no fixed position for a bitmap, so the program checks it at
every offset. The diagnostics name such a filter, because its cost would
otherwise look inexplicable. A dictionary is a list of words, so this case does
not arise from dictionaries.
