# Progress and resuming

**English** | [Русский](progress-and-resume.ru.md)

How long the wait is, how not to start it over, and how to read the output with
a program. And one matching defect, found precisely because the estimate of the
wait had to be checked by counting.

## 1. The defect the estimate found

The estimate is computed from the shape of the filters. The only way to check it
is to count: run candidates and compare the share that hit with the share
predicted. Five forms out of six agreed inside two sigma. The substring did not:
7.2% fewer hits than predicted.

Broken down by position it was not scatter but a rule. A substring at offset 47
was missed **always**, at offset 48 **never**:

```
address   ...i32lab[c]tjobyd     ← symbol 49 is 'c'
key       ...i32lab[a]t          ← symbol 49 is 'a'
```

The engine defers the ed25519 sign bit: computing it costs a field multiply that
almost every candidate would waste. That bit is the top bit of the last key
byte, which base32 numbers as bit 248, and it lands **inside symbol 49**, where
it is worth two. So symbol 49 of a candidate is symbol 49 of its address with
the value-two bit forced off. A substring at offset 47 needs `c` (=2) at
position 49, which requires that bit set — and the key text never has it. One at
offset 48 needs `b` (=1), where the bit is zero anyway — and it was always
found.

The hot loop read 51 symbols. That is two mistakes, not one:

- **misses**: 7.6% of the occurrences of a three-symbol substring were never
  found;
- **inventions**: 374 matches in 200,000 candidates where the cleared bit
  happened to spell the needle. That is the worse half: such a key was recorded
  as a hit, and its address does not contain the substring that was asked for.

A second defect sat next to it: literal substrings were always dropped from the
list of forms the address has to be built for — but what answers for them is the
automaton's tail prefilter, and the automaton is only built from ten filters up.
A set holding one substring never asked for the address at all.

**The fix.** The readable span is cut from 51 symbols to 49. Prefilters may
still use the other two, but they read them treating the sign bit as unknown: a
prefilter is allowed to say "maybe" too often and is never allowed to say "no"
wrongly. Without that, the suffix index would have lost every eight- and
nine-symbol suffix, because the two symbols it pins them by would have gone.

The price: the substring form is 1.6% slower (6 pairs, median 0.984); the prefix
form is untouched (4 pairs, median 0.995).

**What was missing that would have caught it sooner.** Every earlier check of
the matcher compared code with code: the vector path against the scalar one, the
device against the processor. Both sides read the same 51 symbols and were
therefore wrong together. There is now a check against something that cannot be
wrong — the printed address, searched with an ordinary string search
(`tests/matching_finds_everything.rs`). It was verified to fail on the old code.

## 2. The chance of a hit

Computed from the parsed filter rather than guessed. At each position of the
address it is known how many values the protocol allows and how many of them the
filter accepts:

| Position | Values |
|---|---:|
| 0…53 | 32 |
| 54 | 4 |
| 55 | 1 |

Everything follows from that: a prefix of `n` symbols is `32^-n`; a suffix of
the same length is cheaper, because the protocol pays for the last two positions
itself (a three-symbol suffix is 128 candidates against a prefix's 32,768); a
class is its size as a share; a free position costs nothing. For a set, the
chance of missing all of them is the product.

For a substring the placements are summed. They overlap, so the sum is an upper
bound, but with terms this small the second-order correction is under one part
in a million.

Checked by counting (`cargo bench --bench hit-rate`), six forms, about
4000 hits each, two sigma around 3%:

| Form | candidates | predicted | found | found/predicted |
|---|---:|---:|---:|---:|
| prefix, 3 symbols | 131,072,000 | 4000 | 4064 | 1.016 |
| three prefixes | 43,692,032 | 4000 | 3967 | 0.992 |
| class at one position | 32,768,000 | 4000 | 3924 | 0.981 |
| free position | 4,096,000 | 4000 | 3931 | 0.983 |
| suffix, 3 symbols | 512,000 | 4000 | 4073 | 1.018 |
| substring, 3 symbols | 2,523,136 | 4004 | 3941 | 0.984 |

## 3. What is visible before the run and during it

Before the search, the expected number of candidates. Eight symbols is 1.10
trillion, ten is 1.13e15 — that is the two-letter difference the whole thing is
for.

During it, two numbers, and neither is a progress bar. Searching random keys is
memoryless: what has been examined does not bring the next one closer, and
"percent done" would be a lie. Two things are true instead:

- **the chance a hit would already have happened** by now;
- **the median time remaining**, which for such a process is the same number at
  every moment — not a defect of the estimate but a property of the problem.

No time is named until the speed has been measured: before the first statistics
interval any number would be invented.

The fields come after `calc/sec`, so the measurement harness is untouched.

## 4. Continuing

The search space is cut into blocks, each seeded from a root seed and a counter.
So where a run stopped is exactly two numbers, and they go into a file.

- **On a timer, not per block.** Blocks are claimed thousands of times a second;
  a file system in the hot loop would cost more than the ten seconds it saves.
- **Through a temporary file.** A crash mid-write cannot leave half a file where
  a resume would read it.
- **With mode `0600`.** The file derives every key of the run, so it is as
  secret as a key, and the program says so the first time it writes.
- **With the filters checked.** Continuing a different set is refused: otherwise
  two searches would merge into one count.

Nothing is examined twice: the block counter only goes up. Something is skipped,
bounded by the blocks in flight when the run stopped — hundreds of thousands of
candidates against trillions. The alternative would be worse: re-examining would
spend time on what is known to have been checked.

The cost turned out to be not in the writing but in the thread that does it. The
first version woke it five times a second, and on a machine whose every core is
searching that was worth a percent. Once a second costs nothing (4 pairs, median
1.008) and notices a stop just as promptly.

## 5. Machine-readable output

`--json` prints `start`, `stats`, `hit` and `summary`, one object per line. The
prose diagnostics are switched off with it: they answer the same question, and
mixing them into a stream a parser reads defeats the flag. Everything the prose
said moved into the `start` event.

A hit goes to the output stream and the rest to the diagnostic one, as without
the flag: redirecting one still separates them. A test parses every line of a
real run, so a human-readable line added later breaks the test rather than the
user.

## 6. How the machine is shared

A device has a half that runs on the processor: it queues launches and collects
what survived. With every core taken by the search, that half has no core.
Measured on an i9-9900K with an RTX 4060, ten-symbol filter:

| Threads | Vulkan | CUDA |
|---:|---:|---:|
| 1 | 907.0 M/s | 772.7 M/s |
| 8 | 948.1 | — |
| 14 | 962.9 | — |
| 15 | **963.8** | **834.7** |
| 16 | 958.7 | 827.5 |

The processor adds 57–62 M/s to the device, about 7% — but only while the host
half has somewhere to run. So the default now leaves a core to each working
device.

`-t 0` is allowed and means "the device only"; with no device it is refused with
an explanation rather than quietly doing nothing.

## 7. What is not here

- **Control while running.** No socket, no API: a filter cannot be added and the
  thread count cannot be changed in a running search. That is a separate surface
  with a separate security model.
- **Prometheus metrics.** For a single run a line of JSON covers the same case
  without putting a server inside the generator. For a distributed one it does
  not, and on 2026-09-23 the item went back on the plan: collecting the stderr of
  dozens of machines is precisely the work metrics remove. What closes it is
  decided together with the distributed mode itself.
- **An exact time to the next hit.** There is no such thing: the distribution is
  geometric, and the median is the most definite thing that can honestly be
  named.
