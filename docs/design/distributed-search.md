# Distributed search: a master and its workers

**English** | [Русский](distributed-search.ru.md)

How distributed search works and why it works that way.

## 1. Why

The search divides perfectly: candidates are independent, and a hundred
machines should give a hundredfold rate. What is missing is everything around
it. Starting a hundred pods today means a hundred uncoordinated runs, finds
that vanish with the pod, and no way to tell which of the hundred is working
and which has stalled.

What a fleet has to do: register a node, hand out disjoint work, deliver the
filters, collect finds in one place, report per-node statistics, and survive
the loss of a node.

## 2. The layout

The master holds the queue of ranges, the filter set and the store of finds.
Workers connect to it themselves.

There is one binary, and a subcommand picks the role — except for the
single-machine run, which gets no subcommand because it is what the program is
ordinarily used for:

```
onion-gen -F test -d ./keys                       # a single-machine run, as today
onion-gen master --listen :8080 --store ./keys    # the orchestrator
onion-gen worker --master http://master:8080      # a worker
```

A worker takes the same search flags — it searches identically and differs only
in where the work comes from. The master has its own set, which does not
overlap, and that is the argument for subcommands: a flag that only means
something alongside another flag reports its error later and worse than a
subcommand that simply has no such flag.

Filters are asked of whoever needs them: a worker without any is not an error,
since the set arrives from the master.

An image per role would mean several artifacts with their own versions, and a
fleet built from different commits fails as silence rather than as an error —
the workers simply get no work.

![The initiative is always the worker's: the master does not know where they are and cannot call them.](diagrams/01-layout.svg)

The arrows point only upward, and that is a property rather than a drawing
choice: **a worker has no listening sockets at all**. It needs no known
address, no certificate and no firewall rule, and it cannot be reached from
outside. The master does not know where its workers are and cannot call them —
nor does it need to.

## 3. One cycle

![The path of a find is in pink. The worker reports where it found something, not what.](diagrams/02-cycle.svg)

A worker registers, receives a seed, a range of blocks and the filter set, then
searches, renews its lease and reports what it finds. The path of a find is
marked in pink, and it works differently from what one might expect.

## 4. Two numbers on the wire

![The dashed line is what the exchange leaves out. Verification is possible because the master repeats the computation instead of trusting the result.](diagrams/03-wire.svg)

A worker reports **where** it found something, not **what**. The master hands
out the seeds, so it already has them, and there is nothing to be gained by
shipping what is derived from them: the master derives the key itself with the
same `expanded_secret_at_offset` the worker would have used.

The gain is not in traffic. The gain is that **verification becomes possible**:
the master repeats the computation, builds the address and checks it against
the filter. A worker that is broken or substituted cannot put into the store
something it never found — which, had a finished key been sent, would have had
to be taken on trust.

The master stores finds in the ordinary tor layout: a directory per address
with the same three files a single run writes. They can be collected with any
tool, with no new format to learn.

## 5. When a node disappears

A pod is evicted at any moment. That is an ordinary event rather than a
failure, and the design has to survive it without intervention.

![Nothing unsearched is lost — at the cost of repeating the part the worker covered without reporting.](diagrams/04-lease-expiry.svg)

Hence leases rather than assignments: a range is handed out for a period, the
worker renews it, and an expired lease returns to the queue. A range assigned
forever would have stayed unsearched in silence.

The cost is real and worth naming: the part of the range the worker covered
without reporting will be covered again. That is seconds of work against a
guarantee that nothing unsearched is lost.

## 6. The lease is sized to the worker

The spread in rates is wide and measured:

| Machine | Rate | Blocks in a 60 s lease |
|---|---:|---:|
| RTX 4060, Vulkan | 960 M/s | 439,774 |
| Apple M1 Pro, Metal | 150 M/s | 68,527 |
| i9-9900K, processor | 67 M/s | 30,670 |

A factor of 14.3 between the ends, so a fixed size is wrong for someone
whatever it is. Too small and the card spends its time asking for work instead
of doing it, making the master the bottleneck; too large and an evicted pod
means half a range, on average, gets searched again.

A worker reports its own rate, and there is no need to check it. An inflated
rate yields a range the worker cannot finish: the lease expires, the range goes
back to the queue, and the cost falls on whoever lied. Nobody else is affected.

This is worth naming, because it is where our problem parts company with one
that looks like it. In a mining pool, miners submit shares — partial solutions
proving work done. They exist not to check the solution but to settle
**payment**: contribution has to be measured honestly, or an inflated rate is
paid for with other people's money. Nothing here is apportioned by claimed
rate, so there is nothing to prove.

## 7. When the master disappears

![The buffer is not unbounded, but on overflow the system holds rather than discards what it found.](diagrams/05-master-down.svg)

The asymmetry is plain: a master is unreachable for minutes, while a find that
took a week of searching will not come round again. So the worker keeps
unsent finds on disk and retries.

It does not take a new range meanwhile. Taking one unbidden risks overlapping
with whoever the master has already given it to; finishing the range already
held is always safe.

The buffer is not unbounded. On overflow the system says so and keeps holding:
it will not discard something it has found, under any circumstances.

## 8. How the orchestrator controls workers

An orchestrator does not talk to a process. It looks at **how the process
ended**. Control is therefore exit codes, and there a defect turned up.

Measured on the live binary:

```
exit after SIGTERM, nothing found: 0
exit after finding what was asked: 0
```

![Zero must mean only that the run reached its goal. Otherwise an evicted pod reads as a success.](diagrams/06-exit-codes.svg)

A pod evicted during a node drain exits zero. Kubernetes counts a success,
`completions: 1` is satisfied, and the entire fleet is terminated without a
single key found. There is no error anywhere: the job "completed", and nobody
will go looking.

Zero must mean only that the run reached its goal. This has nothing to do with
distribution as such, but it has to be fixed here: without it the orchestrator
cannot control workers under any architecture.

**Open question.** If interruption starts exiting non-zero, an evicted pod
counts against `backoffLimit`. Whether that is acceptable or a further code is
needed gets checked on a real cluster.

## 9. Control by platform

| Platform | Start | Stop on demand |
|---|---|---|
| Kubernetes | `Deployment`, `replicas: N` | `kubectl scale --replicas 0`, or delete it |
| Docker Swarm | `service`, `replicas: N` | `docker service rm` |
| Docker on a VM | `compose up --scale` | `compose down` |
| Bare VM | systemd | `systemctl stop` |

Workers run as **replicas rather than as a job**. There is no "stop on success"
column because a replica does not finish: it runs until stopped, and deciding
that enough has been found is a person's job, looking at the master's store.

The probes follow from this. The orchestrator keeps asking a pod whether it is
alive and whether it is ready, and for a worker those two answers differ:
liveness does not depend on reaching the master, or a master outage would
restart the whole fleet at once and throw away every current range and every
buffered find. Readiness does depend on it, or a rollout would replace the whole
fleet with pods that have nowhere to get work.

## 10. What this design leaves out

| Left out | Why |
|---|---|
| A replicated master | Losing it stops the handout but loses neither finds nor completed work. Solving it before anyone complains is guesswork. |
| A fleet-wide `--limit` | An exact count would cost the coordination being avoided. Spare keys are free. |
| Two modes of operation | The worker is the same in Kubernetes and on a bare VM. Only how it learns the master's address differs. |
| Encrypting finds | Not asked for. The master stores keys in the ordinary tor layout. |

## 11. Rejected alternatives

Two other approaches were considered before settling on a master, and the
reasons are recorded so they need not be worked out again.

**A consensus protocol (Raft).** It would guarantee that no two nodes hold the
same range. The price is a quorum of at least three nodes, stable membership,
leader election on every pod rebuild, persistent storage per node and stable
network names — that is, a `StatefulSet` with `PVC`s. What that buys is
protection from an event costing seconds of duplicated work: a block index is a
`u64`, a block holds 64 batches of 2048 candidates, so one seed holds 2^81
candidates and the fastest card available walks it for 80 million years. Leases
from a master give the same disjointness without a quorum.

**No coordinator at all.** Each node would derive its own space from a shared
salt and its own identity and talk to nobody. It works, but it solves only the
handing out of work — one requirement of six. Finds would stay on the pods,
there would be no statistics, filters would have to be copied by hand, and
identities could silently coincide.

**An IP address as identity** (in that scheme) does not do: addresses are
reused over time, change on restart, are not unique between private networks,
and a machine has several. With a master the question disappears — it assigns
the identity.
