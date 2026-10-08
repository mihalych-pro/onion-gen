# Matching forms

**English** | [Русский](matching-forms.ru.md)

This document describes the matching forms beyond a literal prefix: substring,
suffix, positional wildcards, character classes and regular expressions.

The [README](../../README.md) gives the measured speeds. Regular expressions
have a document of their own, [regex-matching.md](regex-matching.md).

## 1. What makes one form cost more than another

Two properties decide the cost of a form, and neither is the form's name.

**Whether the test can stay on the byte-comparison path.** A fixed prefix is a
byte comparison against the packed key, with one masked byte at the end. A
wildcard or a class leaves that path and walks symbol by symbol, but the
program still checks it at one offset only, so it costs a little more.

**Whether the form reaches past the key into the checksum.** This is the larger
of the two effects, and section 2 describes it.

A substring pays for a third reason: it has no fixed offset, so the program
must look for it everywhere in the text.

## 2. What the checksum costs

The first 51 symbols of an address are a function of the public key alone. Past
them, each symbol depends on `SHA3-256(".onion checksum" || pubkey || 0x03)`.
Any candidate that the key cannot rule out therefore costs a hash.

The key prefilter keeps that rare. The program checks each placement of the
form as far as the key reaches, and only a placement that survives is worth a
hash.

The two ends of this are both suffixes:

- A 14-symbol suffix starts at symbol 42, so eight of its symbols lie inside
  the key. The prefilter rejects all but one candidate in `32^8`.
- A 5-symbol suffix starts at symbol 51 and has nothing inside the key, so
  every candidate is hashed.

Same form and same code, and the cost differs by a large factor. The cause is
the placement and not the form.

## 3. Vectorised Keccak is not built

A form that nothing of reaches into the key is limited by the hash, whatever
the field arithmetic does. A four-lane Keccak is therefore the obvious next
lever, and it is not built, because the case it would help is one where the
search is already over.

For a suffix, that case means five symbols or fewer. The last symbol of an
address is always `d` and the one before it is one of four, so a five-symbol
suffix carries 17 bits. That is one address in 131 072, which a search finds in
a fraction of a second. A suffix long enough to be worth waiting for is long
enough to be prefiltered.

The decision is reversible and costs nothing to revisit: the hash sits behind
one call in `scan_batch`.

## 4. A search that uses none of this pays nothing for it

A set of literal prefixes must cost what it costs without these forms. Two
things keep that true.

**The program chooses the matching pass once for each batch**, and not once for
each candidate. One loop with a test inside would cost a dictionary search
measurably. A dictionary search is where it shows: the bitmap rejects almost
every candidate, so there is little else for an extra field read to hide
behind.

**The representative text of a filter sits in the boxed `Pattern`.** Nothing in
the hot loop reads that text, and as a `String` in `Filter` it would be 24 bytes
in every entry that a dictionary scan walks past.

The same reasoning puts the substring search behind a `Box`. Held inline it
grows `FilterSet`, which the hot loop reads, and the forms that never use it
pay for the size.

## 5. Character class ranges

`[b-e]` means `[bcde]`. Ranges exist because the useful ones, `a-z` and `2-7`,
are tedious to spell and easy to get wrong.

The alphabet runs `a` to `z` and then `2` to `7`. The symbols `0`, `1`, `8` and
`9` are therefore not in it, and no range can produce them. The program rejects
`[0-9]` by name. It also rejects a range whose end comes before its start, as
the typo that it almost certainly is.

## 6. Where found keys go

A hit becomes a directory named after its address, and an easy filter produces
them by the thousand. They go under `./keys` unless `-d` says otherwise, so a
run that starts anywhere does not bury the working directory.

An easy filter is also where throughput stops meaning anything. With
`suffix:zad`, which matches one candidate in 128, the run is limited by the
writer lock and not by the match test. For such a search, use `--db`, which
takes finds far faster than a directory for each key. See
[../deployment/databases.md](../deployment/databases.md).

## 7. A dictionary of general forms

Every form meets the requirement that a thousand filters cost no more than 1.5
times one filter. Each anchor needs a structure of its own to get there, and
[form-indexes.md](form-indexes.md) describes all three.

One case stays linear: a substring that holds a class or a wildcard. It has no
literal for a searcher and no fixed position for a bitmap, so the program
checks it at every offset. The diagnostics name such a filter. A dictionary is
a list of words, so this case does not arise from dictionaries.

## 8. How a literal substring is matched

**Below ten literals, the program searches for each one.** It uses
`memchr::memmem::Finder` over the encoded candidate. The encoding is paid once,
however many substrings the set holds, and a set with none never allocates the
buffer.

**From ten literals up, the program builds an Aho-Corasick automaton.** The
threshold is `AUTOMATON_THRESHOLD`, and it is measured and not chosen.

Two settings of the automaton are not the defaults, and both matter:

- **The prefilter of the crate is off.** The address alphabet is 32 symbols
  over 51 bytes, so no byte is rare, and the prefilter fires on nearly every
  candidate. It costs more than it saves here.
- **The kind is pinned to DFA.** It is the fastest at every size measured. It
  is also ten times larger than the contiguous NFA, which is twice as slow.

**A tail prefilter keeps the hash away.** A substring that begins inside the key
and runs into the checksum is visible from the key alone: it is enough to ask
whether the tail of the key text begins any needle. A small bitmap with a cheap
hash answers that, and the answer does not get slower as filters are added.

That prefilter may answer "maybe" wrongly. It may never answer "no" wrongly. A
false positive costs one unnecessary hash. A false negative would silently lose
addresses. A differential test over 40 000 keys checks the automaton path
against a naive search of the whole address, and it insists that tail matches
actually occur in the sample.

## 9. What each form is worth

A form is worth the symbols of address length that it buys. The program
calculates that from the shape of the filter, and a count over hundreds of
millions of candidates confirms the calculation:

| Form | Observed gain | Calculated | Equivalent symbols |
|---|---:|---:|---:|
| prefix (control) | 1.0x | 1x | 0.00 |
| substring, 5 symbols | 52.1x | 48x | 1.14 |
| suffix, 6 symbols | 236.8x | 256x | 1.58 |
| wildcard, 2 free positions | 1045x | 1024x | 2.01 |
| class, 2 symbols at 2 positions | 4.0x | 4x | 0.40 |

Theory and practice agree inside the sampling error. The substring comes out
slightly above its calculated gain, because the analysis counts offsets
conservatively. The suffix comes out slightly below on 143 hits, which is less
than one standard deviation.

## 10. Correctness

**Every form finds addresses that tor agrees with.** One key for each form goes
to tor with `DisableNetwork 1`. tor regenerates the hostname from the secret
key alone and produces the same address that the generator reported, for
prefix, substring, suffix, wildcard, class and regular expression alike. The
network stays off, and the program publishes nothing.
`scripts/verify/verify-with-tor.sh` does this for any key directory.

**Both architectures agree.** The suite runs natively on `aarch64` and on
`x86_64`, and every form finds and verifies the same way on both.
