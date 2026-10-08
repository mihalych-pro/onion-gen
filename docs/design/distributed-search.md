# Distributed search: a master and its workers

**English** | [Русский](distributed-search.ru.md)

This document describes how distributed search works and why it works that
way.

## 1. Why a fleet needs more than many processes

The search divides perfectly. Candidates are independent, so a hundred machines
give a hundred times the rate. Everything around the search is what a fleet
must add.

A hundred processes with no coordinator are a hundred separate runs. Finds
disappear with the pod, and nobody can tell which process works and which has
stopped.

A fleet must therefore do six things: register a node, hand out disjoint work,
deliver the filters, collect finds in one place, report statistics for each
node, and survive the loss of a node.

## 2. The layout

The master holds the queue of ranges, the filter set and the store of finds.
Workers connect to it themselves.

There is one binary, and a subcommand picks the role. The single-machine run
has no subcommand, because that is how the program is ordinarily used:

```text
onion-gen -F test -d ./keys                       # a single-machine run
onion-gen master --listen :8080 --store ./keys    # the orchestrator
onion-gen worker --master http://master:8080      # a worker
```

A worker takes the same search flags, because it searches in the same way and
differs only in where the work comes from. The master has its own set of flags,
and the two sets do not overlap. That is the argument for subcommands: a flag
that means something only beside another flag reports its error later and worse
than a subcommand that simply has no such flag.

The program asks for filters only from the role that needs them. A worker
without filters is not an error, because the set arrives from the master.

One image carries every role. An image for each role would mean several
artefacts with their own versions, and a fleet built from different commits
fails as silence and not as an error: the workers simply get no work.

![The initiative is always the worker's: the master does not know where they are and cannot call them.](diagrams/01-layout.svg)

The arrows point only upward, and that is a property and not a drawing choice.
**A worker has no listening socket at all.** It needs no known address, no
certificate and no firewall rule, and nothing outside can reach it. The master
does not know where its workers are, and it does not need to.

## 3. One cycle

![The path of a find is in pink. The worker reports where it found something, not what.](diagrams/02-cycle.svg)

A worker registers, receives a seed, a range of blocks and the filter set. It
then searches, renews its lease and reports what it finds. The path of a find
is in pink, and section 4 explains it.

## 4. Two numbers on the wire

![The dashed line is what the exchange leaves out. Verification is possible because the master repeats the computation instead of trusting the result.](diagrams/03-wire.svg)

A worker reports **where** it found something, and not **what**. The master
handed out the seed, so it already has it. There is nothing to gain by sending
what the seed derives: the master derives the key itself, with the same
`expanded_secret_at_offset` that the worker would use.

The gain is not in traffic. The gain is that **the master can verify a find**.
It repeats the computation, builds the address and checks it against the
filter. A worker that is broken or substituted cannot put into the store
something that it never found. A finished key sent over the wire would have to
be taken on trust.

The master stores finds in the ordinary tor layout: a directory for each
address, with the same three files that a single run writes. Any tool can
collect them, and there is no new format to learn.

## 5. When a node disappears

An orchestrator evicts a pod at any moment. That is an ordinary event and not a
failure, and the design must survive it with no intervention.

![Nothing unsearched is lost — at the cost of repeating the part the worker covered without reporting.](diagrams/04-lease-expiry.svg)

The master therefore hands out leases and not assignments. A range goes out for
a period, the worker renews it, and an expired lease returns to the queue. A
range assigned for ever would stay unsearched in silence.

The cost is real and worth naming. The part of the range that the worker
covered without reporting will be covered again. That is seconds of work
against a guarantee that nothing unsearched is lost.

## 6. The lease is sized to the worker

Rates across a fleet differ by more than an order of magnitude, from a
processor to a fast device. A fixed lease size is therefore wrong for somebody,
whatever size it is. Too small, and a fast device spends its time asking for
work instead of doing it, which makes the master the limit. Too large, and an
evicted pod means that half a range, on average, is searched again.

A worker reports its own rate, and nobody needs to check it. An inflated rate
yields a range that the worker cannot finish. The lease expires, the range
returns to the queue, and the cost falls on whoever lied. Nobody else is
affected.

This is worth naming, because it is where this problem parts company with one
that looks like it. In a mining pool, miners submit shares, which are partial
solutions that prove work done. Shares exist to settle **payment** and not to
check the solution: contribution must be measured honestly, or an inflated rate
is paid for with other people's money. Nothing here is apportioned by claimed
rate, so there is nothing to prove.

## 7. When the master disappears

![The buffer is not unbounded, but on overflow the system holds rather than discards what it found.](diagrams/05-master-down.svg)

The asymmetry is plain. A master is unreachable for minutes, and a find that
took a week of searching will not come round again. The worker therefore keeps
unsent finds on disk and retries.

It takes no new range meanwhile. To take one by itself risks an overlap with
whoever the master has already given it to. To finish the range already held is
always safe.

The buffer is not unbounded. On overflow the system says so and keeps holding.
It discards nothing that it has found, in any circumstances.

## 8. How the orchestrator controls workers

An orchestrator does not talk to a process. It looks at **how the process
ended**. Control is therefore exit codes.

![Zero must mean only that the run reached its goal. Otherwise an evicted pod reads as a success.](diagrams/06-exit-codes.svg)

Zero means only that the run reached its goal. A run that stopped short of the
goal exits 4, and that includes an interrupt and a run with no goal to reach. A
bad command line exits 2.

The reason is in the diagram. If an interrupt exited zero, a pod evicted during
a node drain would read as a success. Kubernetes would count `completions: 1`
as satisfied and terminate the whole fleet with no key found. There would be no
error anywhere: the Job "completed", and nobody would go looking.

**One question stays open.** A non-zero exit from an interrupt counts against
`backoffLimit`. Whether that is acceptable, or whether a further code is
needed, has to be checked on a real cluster.

## 9. Control by platform

| Platform | Start | Stop on demand |
|---|---|---|
| Kubernetes | `Deployment`, `replicas: N` | `kubectl scale --replicas 0`, or delete it |
| Docker Swarm | `service`, `replicas: N` | `docker service rm` |
| Docker on a VM | `compose up --scale` | `compose down` |
| Bare VM | systemd | `systemctl stop` |

Workers run as **replicas and not as a job**. There is no "stop on success"
column, because a replica does not finish. It runs until somebody stops it, and
the decision that enough has been found belongs to a person who looks at the
master's store.

The probes follow from this. The orchestrator asks a pod whether it is alive
and whether it is ready, and for a worker those two answers differ. Liveness
does not depend on reaching the master, because a master outage would otherwise
restart the whole fleet at once and throw away every current range and every
buffered find. Readiness does depend on reaching the master, because a rollout
would otherwise replace the whole fleet with pods that have nowhere to get
work.

## 10. What this design leaves out

| Left out | Why |
|---|---|
| A replicated master | To lose it stops the handout, and it loses neither finds nor completed work. |
| A fleet-wide `--limit` | An exact count would cost the coordination that this design avoids. Spare keys are free. |
| Two modes of operation | The worker is the same in Kubernetes and on a bare VM. Only the way it learns the master's address differs. |
| Encrypted finds | The master stores keys in the ordinary tor layout. |

## 11. Rejected alternatives

Two other approaches exist, and the reasons against them are here so that
nobody works them out again.

**A consensus protocol such as Raft.** It would guarantee that no two nodes
hold the same range. The price is a quorum of at least three nodes, stable
membership, a leader election on every pod rebuild, persistent storage for each
node and stable network names. In Kubernetes that is a `StatefulSet` with
`PVC`s.

What that price buys is protection from an event that costs seconds of
duplicated work. A block index is a `u64`, and a block holds 64 batches of 2048
candidates, so one seed holds 2^81 candidates. The fastest card available walks
that for about 80 million years. Leases from a master give the same
disjointness with no quorum.

**No coordinator at all.** Each node would derive its own space from a shared
salt and its own identity, and talk to nobody. It works, and it solves one
requirement of the six: the handing out of work. Finds would stay on the pods,
there would be no statistics, somebody would have to copy the filters by hand,
and two identities could coincide in silence.

**An IP address as identity** does not work in that scheme. Addresses are
reused over time, they change on restart, they are not unique between private
networks, and a machine has several of them. With a master the question
disappears, because the master assigns the identity.
