# Deploying a fleet

**English** | [Русский](fleet.ru.md)

Four platforms, one binary. A subcommand picks the role; everything else takes
the same flags a single run does.

```
onion-gen -F test -d ./keys                       # a single-machine run
onion-gen master --listen :8080 --store /keys     # the orchestrator
onion-gen worker --master http://master:8080      # a worker
```

## What every platform has to answer

**Where the master is.** A worker goes to it and accepts no incoming
connections, so only the master needs a known address. That turned out not to
be theory: when this was checked across two machines the routing between them
was one-way, and the fleet came together only because the worker is the side
that dials.

**Where the master's store is.** It is the one thing whose loss cannot be
undone — the same address will not turn up again soon. A volume, not the
container's own layer.

**Whether it is directories or a database.** `--store` makes a directory per
key; `--db /keys/fleet.db` keeps rows instead, and the rows hold the same bytes
the files do. Directories take about 160 keys a second, rows 206 000, so on a
short filter the choice decides whether workers queue. One file is also easier
to put on a volume and back up than a million directories.

**The worker's buffer.** Finds the master has not taken yet sit on the worker
until it acknowledges them. In normal running it is empty; the volume is there
so that a restart during an outage does not take them with it.

**Filter length.** A fleet will not take a filter shorter than four symbols.
But four is a floor rather than a sufficiency: one RTX 4060 on a four-symbol
filter produces 954 finds a second, and a directory store takes about 160. The
program works the expected rate out at startup and says it, against whichever
store it was given — a database clears 954 a second with room to spare.

## Docker Compose — verified

```bash
docker build -t onion-gen:latest .
docker compose up -d --scale worker=3
```

The files are [`Dockerfile`](../../Dockerfile) and
[`docker-compose.yml`](../../docker-compose.yml) at the root. The image builds
itself: the first stage compiles, the second holds nothing but the binary on
distroless — no shell, no package manager, no utilities.

For both architectures, or to run the tests inside the image:

```bash
docker buildx build --platform linux/amd64,linux/arm64 .
docker build --target check .
```

The compile always runs on the builder's own architecture and cross-compiles
with zig rather than running under emulation: an emulated build of this crate
takes the better part of an hour and produces the same bytes.

Built images are published to `ghcr.io`: `latest` from main, and the version
from a tag.

Checked on a live machine (x86_64, 8 cores): three workers at 38.3 blocks a
second each, 115 together; scaling to five gave 198; no rejections; the
`/health` probe answers 200 from inside the container network.

**Stopping**: `docker compose down`. **Scaling**: `--scale worker=N`.

Two details found by testing rather than by reading:

*A crashed worker comes back; a stopped one does not.* `restart: unless-stopped`
brings a container back after a genuine failure, but Docker treats `docker kill`
and `docker stop` as the operator's decision and deliberately does not override
them. "The worker crashed" and "the worker was stopped" behave differently, and
that is right.

*Without a restart policy the fleet quietly shrinks.* The first version of
`docker-compose.yml` had none: a killed worker's lease came back as designed,
and there was nobody to take it.

## Kubernetes — the chart is written, **not verified on a cluster**

The chart is in [`charts/onion-gen`](../../charts/onion-gen).

```bash
helm install fleet charts/onion-gen \
  --set filters={abcdef} \
  --set worker.replicas=10 \
  --set master.persistence.size=50Gi
```

What has been checked without a cluster: `helm lint --strict` passes for both
worker kinds, the templates render, the YAML parses, the filter-length guard
fires at install time (`filters={abc}` is refused with an explanation), and the
rendered output confirms what matters — the worker gets neither `--filter` nor
`--limit`, liveness is on `/health`, readiness on `/ready`, and the master is
recreated rather than rolled.

What has **not** been checked: none of it has been deployed. No machine here has
a cluster, and until one does, everything below is a description rather than a
report.

Two values worth setting deliberately:

`master.seed.existingSecret` — without it the master takes a random seed and
says so: such a search cannot be resumed once the master is replaced. The seed
is as sensitive as the keys, so it lives in a `Secret` and arrives through the
environment rather than the command line: a process's arguments are readable by
anything that can list processes on the node.

`worker.kind` — a `Deployment` keeps the buffer of unsent finds in an
`emptyDir`, and replacing a pod during a master outage takes them with it. A
`StatefulSet` gives every worker a volume of its own and a stable name in the
fleet view.

Workers run as a **`Deployment` with replicas, not a `Job`**: a replica does not
finish, it runs until stopped, and deciding that enough has been found is a
person's job, looking at the master's store.

| | |
|---|---|
| master | a one-replica `Deployment` plus a `Service` — the service name is the address workers use |
| store | a `PersistentVolumeClaim` on the master |
| workers | a `Deployment`, `replicas: N`; no volumes beyond the buffer |
| stopping | `kubectl scale --replicas 0`, or delete it |
| metrics | `/metrics` on the master and on every worker, as separate scrape targets |

The probes are the thing that has to be right here:

```yaml
livenessProbe:
  httpGet: { path: /health, port: 9100 }
  periodSeconds: 30
readinessProbe:
  httpGet: { path: /ready, port: 9100 }
  periodSeconds: 10
```

**Liveness does not depend on reaching the master, and that must not be
changed.** A worker whose master has gone is doing useful work: finishing the
range it holds and keeping the finds it makes. If liveness depended on the
master, a master outage would restart the entire fleet at once and throw both
away — one component's failure becoming everyone's lost work. Checked on a live
run: master killed, `/health` answers 200, `/ready` answers 503, the worker
carries on.

One trap: **do not give a worker `-n`**. On reaching the limit it exits zero,
the `Deployment` starts it again, and so on for ever. A limit makes sense for a
single run, not for a replica.

## Docker Swarm — not verified

As Compose, with `docker service` in place of compose:

```bash
docker service create --name master --publish 8080:8080 \
  --mount type=volume,source=keys,target=/keys \
  onion-gen:latest -F abcde master --listen 0.0.0.0:8080 --store /keys
docker service create --name worker --replicas 5 \
  onion-gen:latest --compute cpu worker --master http://master:8080
```

**Stopping**: `docker service rm`. **Scaling**: `docker service scale worker=N`.

## Bare virtual machines — not verified as a service

Two machines were checked directly, without systemd: the master on one, workers
on both, 337.4 blocks a second together against 153.5 for one alone, 114 finds
and not one duplicate.

As a unit it looks like this:

```ini
[Unit]
Description=onion-gen worker
After=network-online.target

[Service]
ExecStart=/usr/local/bin/onion-gen --compute auto -d /var/lib/onion-gen \
          worker --master http://master.example:8080
Restart=always
# An interrupted run exits 4, and for a worker that is an ordinary ending:
# it should be restarted either way.
RestartSec=5
User=onion-gen

[Install]
WantedBy=multi-user.target
```

**Stopping**: `systemctl stop onion-gen-worker`.

## Exit codes

| code | meaning |
|---:|---|
| 0 | the run reached the goal it was given |
| 1 | it could not start |
| 2 | the command line was wrong |
| 3 | a found key did not open its address — a defect, and the run stopped |
| 4 | the run was stopped before reaching its goal |

Zero means **only** a goal reached. That is not a formality: were an
interruption zero, an evicted pod would count as a success, and a `Job` wanting
one completion would tear down the whole fleet without a key found.
