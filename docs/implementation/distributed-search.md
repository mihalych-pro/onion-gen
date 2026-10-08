# Distributed search

**English** | [Русский](distributed-search.ru.md)

A master, workers and leases. The design and its diagrams are in
[design/distributed-search.md](../design/distributed-search.md) and deployment
in [deployment/fleet.md](../deployment/fleet.md); what is recorded here are the
outcomes.

## 1. What it came to

One binary, with a subcommand for the role. Without one the behaviour is what
it was, so nothing that already calls this had to change.

```
onion-gen -F test -d ./keys                       # a single-machine run
onion-gen master --listen :8080 --store /keys     # the orchestrator
onion-gen worker --master http://master:8080      # a worker
```

The master hands out leases on ranges of blocks, delivers the filter set, takes
in finds and shows the fleet. A worker takes a lease, searches, renews, hands
over what it finds, and accepts no incoming connection about work at all.

## 2. A worker reports where, not what

The master handed out the seed, so it already has it. The worker need only send
a block number and an offset; the master derives the key itself, with the same
`expanded_secret_at_offset` the worker would have used.

The gain is not in traffic. The gain is that **verification becomes possible**:
the master repeats the computation, builds the address and checks it against
the filter. A worker that is broken or substituted cannot put into the store
something it never found. Tested on its own: a position from a different seed
does not carry its address along with it.

## 3. The lease is sized to the worker

The spread in rates is measured and wide:

| machine | rate | blocks in a 60 s lease |
|---|---:|---:|
| RTX 4060, Vulkan | 960 M/s | 439,774 |
| Apple M1 Pro, Metal | 150 M/s | 68,527 |
| i9-9900K, processor | 67 M/s | 30,670 |

A factor of 14.3 between the ends, so a fixed size is wrong for someone
whatever it is. A worker reports its own rate and there is no need to check it:
an inflated one yields a range it cannot finish, the lease expires, and the cost
falls on whoever lied.

This is where the problem parts company with a mining pool, which it resembles
from outside. There a worker sends the server partial solutions — shares —
which occur far more often and are therefore useless as a result but useful as
evidence of work done. They exist to settle **payment**, not to check the
solution. Nothing here is apportioned by claimed rate, so there is nothing to
prove.

### How long a lease should be

Measured, and the arithmetic was wrong. Requests to the master take 8% of the
time at a one-second lease, while throughput falls by a factor of 24:

| lease | blocks/s |
|---|---:|
| 1 s | 6.4 |
| 5 s | 6.4 |
| 15 s | 57.4 |
| 60 s | 153.4 |

What costs is not the exchange but rebuilding the engine for every lease:
choosing the device, constructing the engines, starting the threads and
building the filter index all happen again. Sixty seconds is kept: requests
take 0.1% and an eviction loses half a minute of work on average. Below fifteen
seconds is not worth doing.

## 4. Where the limit is

Measured by `benches/master-load.rs` over real HTTP, and there are two
ceilings.

**Lease traffic constrains nothing.** 20,000–28,000 requests a second, latency
under 20 ms at four hundred concurrent. A worker sends about four requests a
minute, so a hundred nodes make 7 a second — three orders of magnitude of room.

**Finds are what bite.** Each one is a key derivation, an address and three
files under one lock: the refusal path handles 3,883 a second, and the
acceptance path includes the write and manages **185**.

Hence a four-symbol floor on a fleet's filter. But a floor is not a
sufficiency, and the program works the rate out at startup and says so:

| filter | finds a second from one RTX 4060 |
|---|---:|
| 4 symbols | 954 — five times what the store takes |
| 5 symbols | 28.6 |
| 6 symbols | 0.89 |

## 5. Two machines

Master on linux x86_64, workers on both:

| | alone | together |
|---|---:|---:|
| linux, 8 cores | 153.5 bl/s | 184.2 |
| mac, 8 threads | — | 153.2 |
| **together** | | **337.4** |

114 directories, 114 distinct addresses — not one duplicate, all matching the
filter, no rejections and no connection errors. Six keys were checked against
the independent verifier and against tor itself with `DisableNetwork 1`.

An observation along the way: the routing between the machines turned out to be
one-way — the mac could reach linux and not the other way round. The master had
to go on linux. That is exactly the case the design is for: were it the other
way, the fleet would not have come together at all.

## 6. Metrics: each about itself

The master knows the shape of the fleet — leases out and returned, finds
accepted, positions refused. A worker knows its own hardware: its measured
rate, blocks finished, readiness, and **how many finds it is holding** because
the master could not take them. That last number is how an outage becomes
visible, and forwarding through a summary would lose it.

The format is built by the official `prometheus-client` from
`github.com/prometheus/client_rust`. A worker's name is made safe before it
becomes a label: the library does not escape label values — checked against the
library, not assumed — and a quote would end the label early and take the rest
of the exposition with it.

## 7. The probes answer different questions

**Liveness does not depend on reaching the master.** A worker whose master has
gone is doing useful work: finishing the range it holds and keeping the finds it
makes. Restarting it throws both away, and it would happen to every worker at
once — one component's failure becoming the whole fleet's lost work.

**Readiness does depend on it.** A pod with nowhere to get work is not a worker,
and a rollout should stop on such pods.

Checked on a live run: master killed, `/health` answers 200 "searching",
`/ready` answers 503 "no work from the master", and the worker carries on.

## 8. Acceptance

The single-machine run did not slow down. Alternating paired measurement
against the commit before the distributed mode existed, eight pairs, M1 Pro, eight
threads, `--compute cpu`:

| form | before | after | ratio |
|---|---:|---:|---:|
| prefix `abcdefghij` | 47.7 M/s | 47.0 M/s | 0.986 |
| substring `contains:abcdefghij` | 22.2 M/s | 21.6 M/s | 0.972 |
| suffix `suffix:abcdefghijkzad` | 45.6 M/s | 45.8 M/s | 1.004 |
| regex `regex:^abcdefghij` | 44.1 M/s | 45.2 M/s | 1.025 |

The ratios fall on both sides of one — had the loop gained work, all four would
have gone down. Both sides were built with the same rustc, so the toolchain
upgrade is outside the comparison: what is measured is this code, not the
compiler.

Secrets stay out of the output: a run with a known seed, then a search for it in
the master's log, the worker's log, the fleet view, both sides' metrics and the
worker's own files — zero occurrences everywhere.

Keys that travelled the whole path, worker to master to store, pass the
independent verifier and tor with `DisableNetwork 1`.

## 9. What turned up along the way

Three defects were caught by running the thing end to end, and no unit test saw
any of them.

**The block offset.** The worker searched blocks from zero and reported them
shifted by the start of its lease — different seeds, so the master derived a
different key and refused perfectly good finds. 4,387 rejections against 3,264
finds.

**The heartbeat thread never stopped.** `held` stayed set after the search
finished, `join` blocked for ever, and the worker held one lease for the life of
the process. From outside it looked like running but doing nothing.

**A finished range was never released** — it expired and went back out to be
searched again. The worker now names the lease it finished when it asks for the
next one.

On top of that, the rate measurement demanded half a second of work while the
first lease of 256 blocks finishes sooner: no rate was ever measured, every
later lease stayed at the starting size, and the fleet view showed zero.

And separately, a defect with nothing to do with distribution but fixed here
because without it an orchestrator cannot control workers: an interrupted run
exited 0, the same as one that found everything. Under a Job wanting one
completion that would tear down the whole fleet without a key found.

## 10. What is not here, and why

- **The master is not fault-tolerant.** Losing it stops the handout but loses
  neither finds nor completed work: workers finish what they hold and keep what
  they find. A replicated master is a separate question, and answering it
  before anyone complains is guesswork.
- **There is no fleet-wide limit.** An exact count would cost the coordination
  being avoided, and spare keys are free.
- **Finds are not encrypted.** Not asked for; the master keeps them in the
  ordinary tor layout.
- **Kubernetes is unverified on a cluster.** The chart is written and checked
  without one, but never deployed: no machine here has a cluster.
