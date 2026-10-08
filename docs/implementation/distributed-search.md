# Distributed search

**English** | [Русский](distributed-search.ru.md)

This document describes how a master and its workers divide a search. The
design and its diagrams are in
[design/distributed-search.md](../design/distributed-search.md), and deployment
is in [deployment/fleet.md](../deployment/fleet.md).

## 1. One binary, three roles

A subcommand selects the role. Without a subcommand the program does a
single-machine run, so a caller that does not know about the fleet needs no
change.

```text
onion-gen -F test -d ./keys                       # a single-machine run
onion-gen master --listen :8080 --store ./keys    # the orchestrator
onion-gen worker --master http://master:8080      # a worker
```

The master hands out leases on ranges of blocks, delivers the filter set, takes
in finds and shows the fleet. A worker takes a lease, searches, renews the
lease, hands over what it finds, and accepts no incoming connection about work.

## 2. A worker reports where, not what

The master handed out the seed, so the master already has it. The worker
therefore sends a block number and an offset. The master derives the key
itself, with the same `expanded_secret_at_offset` that the worker would use.

The gain is not in traffic. The gain is that **the master can verify a find**.
It repeats the computation, builds the address and checks it against the
filter. A worker that is broken or substituted cannot put into the store
something that it never found. A position from a different seed does not carry
its address with it, and a test covers that case on its own.

## 3. The lease is sized to the worker

Rates across a fleet differ by more than an order of magnitude, from a
processor to a fast device. A fixed lease size is therefore wrong for somebody,
whatever size it is.

A worker reports its own measured rate, and the master sizes the lease by time
and not by block count. There is no need to check the claim. An inflated rate
yields a range that the worker cannot finish, the lease expires, and the cost
falls on whoever lied.

This is where the problem parts company with a mining pool, which it resembles
from outside. A mining pool takes partial solutions, called shares, which occur
far more often than a result. Shares are useless as a result and useful as
evidence of work. They exist to settle **payment**. Nothing here is apportioned
by claimed rate, so there is nothing to prove.

### Why the lease is a minute and not a second

The cost of a short lease is not the exchange with the master. It is the work
of starting a lease: the program chooses the device, constructs the engines,
starts the threads and builds the filter index, and it does all of that again
for every lease.

Sixty seconds is the default. At that length the requests are a negligible part
of the time, and an eviction loses half a minute of work on average. Anything
below fifteen seconds is not worth doing.

## 4. Where the limit is

Lease traffic constrains nothing. A worker sends about four requests a minute,
so even a large fleet stays far below what the master serves.

**Finds are what bite.** Each one is a key derivation, an address and three
files under one lock. The store, and not the search, is therefore the ceiling
of a fleet.

The program works the expected find rate out from the filter set at startup and
compares it with what the store can take. The two ceilings differ:
`STORE_PER_SECOND` for a directory for each key, and `DATABASE_PER_SECOND` for
`--db`, which is much higher. A filter too short for the chosen store is
refused with the arithmetic shown, and not accepted and then quietly lost.

## 5. Metrics: each side about itself

The master knows the shape of the fleet: leases out and returned, finds
accepted, positions refused. A worker knows its own hardware: its measured
rate, blocks finished, readiness, and **how many finds it is holding** because
the master could not take them. That last number is how an outage becomes
visible, and a summary that forwarded everything through the master would lose
it.

The official `prometheus-client` from `github.com/prometheus/client_rust`
builds the format. The program makes a worker's name safe before that name
becomes a label. The library does not escape label values, which we checked
against the library and did not assume, and a quote would end the label early
and take the rest of the exposition with it.

## 6. The probes answer different questions

**Liveness does not depend on reaching the master.** A worker whose master has
gone is still doing useful work: it finishes the range that it holds and keeps
the finds that it makes. To restart it throws both away, and it would happen to
every worker at once. One component's failure would become the whole fleet's
lost work.

**Readiness does depend on reaching the master.** A pod with nowhere to get
work is not a worker, and a rollout must stop on such pods.

With the master stopped, `/health` answers 200 and "searching", `/ready`
answers 503 and "no work from the master", and the worker carries on.

## 7. Exit codes let an orchestrator tell the two cases apart

An interrupted run must not exit 0. Under a Job that wants one completion, a
zero from an evicted pod reads as success, and the orchestrator tears down the
whole fleet without a key found. A run that reached its goal exits 0, and an
interrupt exits with a code of its own.

## 8. What is not here, and why

- **The master is not fault-tolerant.** To lose it stops the handout, and it
  loses neither finds nor completed work: workers finish what they hold and
  keep what they find. A replicated master is a separate question.
- **There is no fleet-wide limit on the number of keys.** An exact count would
  cost the coordination that this design avoids, and spare keys are free.
- **Finds are not encrypted.** The master keeps them in the ordinary tor
  layout.
- **The Helm chart has not run on a cluster.** It is written and checked
  without one, because no machine here has a cluster.
