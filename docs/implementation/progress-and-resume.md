# Progress and resuming

**English** | [Русский](progress-and-resume.ru.md)

This document describes how the program estimates the wait, how it continues a
search instead of starting again, and how a program can read its output.

## 1. Why the readable span is 49 symbols

The engine defers the ed25519 sign bit, because to compute it costs a field
multiply that almost every candidate would waste. That bit is the top bit of the
last key byte. Base32 numbers it as bit 248, and it lands inside symbol 49,
where it is worth two.

Symbol 49 of a candidate is therefore symbol 49 of its address with the
value-two bit forced off. A match test that reads symbol 49 as if it were final
gives two kinds of wrong answer. It misses a filter that needs the bit set. It
also invents a match when the cleared bit happens to spell what the filter
wants, and the address of such a key does not hold what the user asked for.

The hot loop therefore reads 49 symbols. A prefilter may still use symbols 49
and 50, but it must read them with the sign bit unknown. **A prefilter may say
"maybe" too often. It may never say "no" wrongly.** Without that rule the suffix
index would lose every eight-symbol and nine-symbol suffix, because the two
symbols that pin them would go.

One check guards this, and it does not compare code with code. A comparison of
the vector path against the scalar path, or of the device against the processor,
cannot find an error that both sides share. The test in
`tests/matching_finds_everything.rs` searches the printed address with an
ordinary string search, which is something that cannot be wrong.

## 2. The chance of a hit

The program computes the chance from the parsed filter. It does not guess. At
each position of the address it knows how many values the protocol allows, and
how many of those the filter accepts:

| Position | Values |
|---|---:|
| 0…53 | 32 |
| 54 | 4 |
| 55 | 1 |

Everything follows from that table. A prefix of `n` symbols is `32^-n`. A suffix
of the same length is cheaper, because the protocol pays for the last two
positions itself. A class is its size as a share. A free position costs nothing.
For a set of filters, the chance of a miss on all of them is the product.

For a substring the program adds the placements. They overlap, so the sum is an
upper bound, but with terms this small the second-order correction is less than
one part in a million.

`cargo bench --bench hit-rate` checks the estimate by counting. It runs six
forms to about 4000 hits each, where two sigma is about 3%:

| Form | Candidates | Predicted | Found | Found/predicted |
|---|---:|---:|---:|---:|
| prefix, 3 symbols | 131 072 000 | 4000 | 4064 | 1.016 |
| three prefixes | 43 692 032 | 4000 | 3967 | 0.992 |
| class at one position | 32 768 000 | 4000 | 3924 | 0.981 |
| free position | 4 096 000 | 4000 | 3931 | 0.983 |
| suffix, 3 symbols | 512 000 | 4000 | 4073 | 1.018 |
| substring, 3 symbols | 2 523 136 | 4004 | 3941 | 0.984 |

## 3. What the program shows before and during a run

Before the search the program shows the expected number of candidates. Eight
symbols is 1.10 trillion and ten symbols is 1.13e15. That is the two-symbol
difference that the whole program exists for.

During the search it shows two numbers, and neither is a progress bar. A search
over random keys has no memory. What the program has examined does not bring the
next candidate closer, and a "percent done" figure would be a lie. Two other
things are true:

- the chance that a hit would already have happened by now;
- the median time that remains, which for such a process is the same number at
  every moment. That is a property of the problem and not a defect of the
  estimate.

The program names no time until it has measured the speed. Before the first
statistics interval, any number would be an invention.

These fields come after `calc/sec`, so a measurement harness that reads that
field keeps working.

## 4. Continuing a run

The program cuts the search space into blocks. A root seed and a counter seed
each block, so the place where a run stopped is exactly two numbers, and those
two numbers go into a file.

- **On a timer, and not for each block.** The program claims blocks thousands of
  times a second. A file system in the hot loop would cost more than the ten
  seconds that it saves.
- **Once a second.** This costs nothing measurable and notices a stop just as
  quickly. A thread that wakes more often takes a core away from the search.
- **Through a temporary file.** A crash during a write must not leave half a
  file where a resume would read it.
- **With mode `0600`.** The file derives every key of the run, so it is as
  secret as a key. The program says so the first time that it writes the file.
- **With the filters checked.** The program refuses to continue a different
  filter set, because two searches would otherwise merge into one count.

The program examines nothing twice, because the block counter only goes up. It
does skip some work, bounded by the blocks in flight when the run stopped. That
is hundreds of thousands of candidates against trillions. The alternative is
worse: to examine them again would spend time on work that is known to be done.

## 5. Machine-readable output

`--json` prints `start`, `stats`, `hit` and `summary`, one object for each line.
The flag also switches the prose diagnostics off. They answer the same question,
and to mix them into a stream that a parser reads defeats the flag. Everything
that the prose says is in the `start` event.

A hit goes to the output stream and the rest goes to the diagnostic stream, as
it does without the flag. A redirect of one stream therefore still separates
them. A test parses every line of a real run, so a human-readable line added
later breaks the test and not the user.

## 6. How the program shares the machine

A device has a half that runs on the processor. That half queues launches and
collects what survived. When the search takes every core, that half has no core
to run on, and the device loses more than the extra core gives.

The default therefore leaves one core to each working device.

`-t 0` is allowed and means "the device only". With no device the program
refuses it and explains why, instead of doing nothing quietly.

## 7. What is not here

- **Control during a run.** There is no socket and no API. You cannot add a
  filter or change the thread count in a running search. That is a separate
  surface with a separate security model.
- **An exact time to the next hit.** No such thing exists. The distribution is
  geometric, and the median is the most definite figure that anyone can name
  honestly.

Prometheus metrics are not on this list. The master and the worker both serve
them, together with liveness and readiness probes. See
[distributed-search.md](distributed-search.md).
