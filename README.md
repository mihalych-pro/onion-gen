# onion-gen

**English** | [Русский](README.ru.md)

Generates Tor Onion Service v3 addresses that match text you choose — a prefix,
a substring, a suffix, a pattern or a regular expression — and writes the keys
in the layout tor expects.

Written in Rust. Searches on the processor, on a graphics card, or across many
machines.

## Install

The quickest way is a released binary. These links always point at the latest
release:

| Platform | |
|---|---|
| Linux, Intel/AMD | [onion-gen-linux-amd64](https://github.com/mihalych-pro/onion-gen/releases/latest/download/onion-gen-linux-amd64) |
| Linux, ARM | [onion-gen-linux-arm64](https://github.com/mihalych-pro/onion-gen/releases/latest/download/onion-gen-linux-arm64) |
| macOS, Apple silicon | [onion-gen-darwin-arm64](https://github.com/mihalych-pro/onion-gen/releases/latest/download/onion-gen-darwin-arm64) |
| macOS, Intel | [onion-gen-darwin-amd64](https://github.com/mihalych-pro/onion-gen/releases/latest/download/onion-gen-darwin-amd64) |
| Windows, Intel/AMD | [onion-gen-windows-amd64.exe](https://github.com/mihalych-pro/onion-gen/releases/latest/download/onion-gen-windows-amd64.exe) |
| Windows, ARM | [onion-gen-windows-arm64.exe](https://github.com/mihalych-pro/onion-gen/releases/latest/download/onion-gen-windows-arm64.exe) |

Check what you downloaded. This program writes keys that you cannot replace,
so it is worth the two extra lines. Each binary has a `.sha256` of its own, so
you need nothing but the file you took:

```bash
base=https://github.com/mihalych-pro/onion-gen/releases/latest/download
curl -LO $base/onion-gen-linux-amd64
curl -LO $base/onion-gen-linux-amd64.sha256
sha256sum -c onion-gen-linux-amd64.sha256     # macOS: shasum -a 256 -c
chmod +x onion-gen-linux-amd64 && ./onion-gen-linux-amd64 --version
```

Every release also carries [SHA256SUMS](https://github.com/mihalych-pro/onion-gen/releases/latest/download/SHA256SUMS), which lists all six at
once. Use it when you took all six; for one file the `.sha256` beside it is
easier, because checking one name against the full list needs
`--ignore-missing`, which macOS does not have.

### Homebrew, on macOS and Linux

```bash
brew install mihalych-pro/tap/onion-gen
```

### A container

```bash
docker run --rm -v "$PWD/keys:/home/nonroot/keys" ghcr.io/mihalych-pro/onion-gen:latest -F test -n 1
```

`latest` follows the default branch; a release is also tagged by its version,
so `:0.1.0` stays where it is.

Every published image is signed, and the signature says which workflow built it:

```bash
cosign verify ghcr.io/mihalych-pro/onion-gen:latest \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  --certificate-identity-regexp '^https://github\.com/mihalych-pro/onion-gen/\.github/workflows/'
```

## Build

Rust 1.99 or newer, installed through `rustup`:

```bash
cargo build --release
cargo test --release
```

The build compiles the CUDA kernel on the way, which needs a second, older
toolchain; `rustup` fetches it the first time and nothing else is required —
no CUDA toolkit, and the program itself stays on stable. See
[cuda-kernel/](cuda-kernel/README.md).

Binaries are published for linux, macOS and Windows on amd64 and arm64, and a
container image is built from them. There are two Dockerfiles: `Dockerfile`
compiles from sources and is what a local build should use, and
`Dockerfile.dist` packs binaries that already exist, which is what CI does so
the same sources are not compiled twice. Both produce the same image.

## Use

```bash
# an address starting with "test", stop after one hit
onion-gen -d ./keys -n 1 -F test

# a dictionary of names from a file, statistics every 5 seconds
onion-gen -d ./keys -f words.txt -s 5
```

Each find is written to `./keys/<address>.onion/` as `hs_ed25519_secret_key`,
`hs_ed25519_public_key` and `hostname` — copy that directory to your tor
`HiddenServiceDir`. A worked example with nginx is in
[examples/docker-onion-nginx/](examples/docker-onion-nginx/).

### What it measures about your machine

The batch size and a device's launch shape are settled by running them, because
where they stop paying is a property of the cache and the card rather than
something to hard-code. That costs about a second, once: the answer is written
to `tuning.yaml` in the platform's cache directory, keyed by the device, the
thread count and this program's version, and reused.

```bash
onion-gen -F abcd --retune       # measure again, ignoring what is stored
```

Pass `--retune` after a driver update, or to check what is stored against the
machine as it is now. Deleting the file has the same effect. Nothing about the
cache can stop a search: an unreadable or stale entry simply means the
measurement happens.

### Keeping finds in a database

```bash
onion-gen -F abcd --db keys.db                                # a SQLite file
onion-gen -F abcd --db postgres://user:pass@host:5432/onion   # PostgreSQL
onion-gen -F abcd --db mysql://user:pass@host:3306/onion      # MySQL
```

One flag, one connection string, three kinds of database. A bare path means
SQLite, which is created if it is not there and readable only by its owner. One
row per address, holding the same bytes the three files hold, so `base64 -d` on
a row produces exactly what tor reads.

`--db` replaces the key directories rather than adding to them, so it cannot be
given together with `-d`: a run keeps its finds in one place, and which place
is the question the flag answers.

Worth it when finds come fast. A directory and three files takes about 6 ms, so
some 160 keys a second; a thousand rows share one transaction, and the same
machine takes 244 000 into SQLite, 32 000 into PostgreSQL and 22 700 into
MySQL. A short filter on a card finds faster than directories can be made, and
so does a fleet.

Finds are written in batches whichever store it is — a thousand of them, or
once a second, whichever comes first. A find is therefore stored within a second
of being printed, and a process killed inside that second loses what it was
holding.

Setting up an account, the schema and the migrations:
[docs/deployment/databases.md](docs/deployment/databases.md).

### Before you start, and after you finish

```bash
onion-gen -F abcdef --evaluate          # survey this machine, then search
onion-gen guess -F abcdef               # how long, without searching
onion-gen verify ./keys                 # check what was found
```

`--evaluate` measures this machine on the engine that will do the searching and
prints what each prefix length from 4 to 12 symbols costs, as a median and as a
nine-in-ten wait. `guess` answers the same question for one filter set and
exits; it says so plainly when the set holds a regular expression, whose odds
do not follow from its text.

`verify` checks every key under a directory: the file shape, the clamping, that
the secret key produces its public key, and that all of it agrees with the
address. The last check is made by `tor-hscrypto`, the onion-service code the
Arti project ships — an implementation that is not ours, because our own
arithmetic agreeing with itself proves only that it is consistent. Nothing
contacts the network. It exits non-zero if any key fails.

### What you can search for

| Form | Written as | Means |
| --- | --- | --- |
| prefix | `test` or `prefix:test` | the address starts with it |
| substring | `contains:test` | it appears anywhere |
| suffix | `suffix:testad` | the address ends with it |
| free position | `te?t` | any symbol there |
| character class | `t[eo]st`, `t[a-f]st` | one of those symbols there |
| regular expression | `regex:^my(shop\|store)` | the expression matches |

The alphabet is `a`–`z` and `2`–`7`. Filters come from `-F` (repeatable) or
`-f file` (one per line), and the two add up.

Two limits are worth knowing before planning a search. The version byte fixes
the last symbol of every v3 address at `d` and allows only `a`, `i`, `q` or `y`
before it, so most suffixes can never occur — those are rejected when the filter
is read. And an expression with no obligatory literal, such as
`regex:^[bcdfghjklmnp]{6}`, costs about fourteen times a prefix, because nothing
can reject a candidate cheaply.

A thousand names cost about what one name costs, for every form. That matters
more than it sounds: a thousand-name dictionary finds a hit a thousand times
sooner, which beats any speed-up of the search itself.

### Long searches

```bash
# name the seed, and the run becomes repeatable
onion-gen -d ./keys --seed $SEED -F test

# continue where it stopped; the number is printed at the end of a run
onion-gen -d ./keys --seed $SEED --from-block 1742 -F test
```

`--state FILE` keeps both values for you instead. The seed is a secret — every
key of the run derives from it — so pass it as `ONION_GEN_SEED` rather than on
the command line, which other users can read.

### Processor or card

```bash
onion-gen -F test                  # both, the default
onion-gen --compute gpu -F test    # the card only
onion-gen --compute cpu -F test    # the processor only
```

A card reached more than one way is used the fastest way: CUDA on NVIDIA,
Metal on Apple, Vulkan elsewhere, with OpenCL behind all of them. `--compute`
overrides it. Only a driver is needed, not a vendor toolkit — the same binary
works with a card and without one.

### Many machines

```bash
onion-gen -F abcdef master --listen :8080 --store ./keys     # orchestrator
onion-gen worker --master http://master:8080                 # worker, any number
```

The master takes `--db` as well, and a fleet is where it earns its keep: every
find in the fleet is written by the one master. It also publishes what the
store is doing — connections, rows, bytes, failed batches.

The master hands each worker a seed and a range of blocks, and collects the
finds. Workers dial out; the master never calls them. A worker reports where it
found something rather than what, so the master derives the key itself and
checks it. If a worker disappears its range goes back in the queue, and finds it
has not handed over yet wait on it.

Both sides serve Prometheus metrics about themselves. Deployment notes are in
[docs/deployment/fleet.md](docs/deployment/fleet.md) and a Helm chart in
[charts/onion-gen](charts/onion-gen).

## Speed

Against [`mkp224o`](https://github.com/cathugger/mkp224o) and the two other
current generators, [`prefix32`](https://github.com/0xROOTPLS/Prefix32) and
[`onionloom`](https://github.com/chrisch88dev/onionloom). Every figure is from
a real search on a filter long enough that nothing is found, three runs taken
in rotation so machine drift lands on all four equally, median reported.
Released binaries wherever the project publishes one.

**Processors** — millions of candidates tested per second

| | Apple M1 Pro<br>macOS 27, 8 threads | Intel i9-9900K<br>Windows 11, 16 threads | Xeon E5-2683 v4<br>Debian 13, 8 threads |
|---|---:|---:|---:|
| **onion-gen** | **88.6 M/s** | **150.4 M/s** | **66.0 M/s** |
| prefix32, released binary | not published | 120.5 M/s | 59.5 M/s |
| prefix32, `-fast` binary | not published | 156.2 M/s | does not start |
| prefix32, built from source | 85.5 M/s | — | 67.0 M/s |
| onionloom | 55.1 M/s | 92.5 M/s | 43.9 M/s |
| mkp224o | 32.0 M/s | 39.1 M/s | 21.2 M/s |

How many times this one is faster:

| against | M1 Pro | i9-9900K | Xeon |
|---|---:|---:|---:|
| prefix32, as released | — | 1.25x | 1.11x |
| prefix32, its fastest build | 1.04x | 0.96x | 0.99x |
| onionloom | 1.61x | 1.63x | 1.50x |
| mkp224o | 2.77x | 3.86x | 3.11x |

`prefix32` publishes three Linux binaries; two of them need `libOpenCL.so.1`
and exit at once on a machine without it, which is why the Xeon column shows
the third. It publishes none for macOS, so that column is built from source,
with the `target-cpu=native` its own repository sets.

**Graphics devices** — millions of candidates tested per second

| | Apple M1 Pro | NVIDIA RTX 4060 |
|---|---:|---:|
| **onion-gen**, the default | **255.5 M/s** (Metal) | **1861.9 M/s** (CUDA) |
| onion-gen, Vulkan | no driver on macOS | 1791.0 M/s |
| onion-gen, OpenCL | 249.8 M/s | 1758.9 M/s |
| prefix32 | not supported | 1830 M/s |
| onionloom | 4.35 M/s | 147.9 M/s |
| mkp224o | not supported | not supported |

How many times this one is faster:

| against | M1 Pro | RTX 4060 |
|---|---:|---:|
| prefix32 | reaches no device here | 1.02x |
| onionloom | 58.7x | 12.6x |
| mkp224o | no device path | no device path |

`prefix32` is built on an OpenCL 2.0 entry point that Apple's OpenCL 1.2 does
not export, and a third-party loader finds no platform on macOS at all.
`onionloom` measures the M1 Pro's device below its own processor and falls back.

`mkp224o` is the `./configure && make` build where no binary is published. Each
program picks its own launch shape and batch size by measuring the machine it
is on, so these are the rates each one settles at rather than a tuning contest.

Where the speed comes from: [docs/implementation/speed.md](docs/implementation/speed.md).

## What this does that the others do not

Measured against [`prefix32`](https://github.com/0xROOTPLS/Prefix32) and
[`onionloom`](https://github.com/chrisch88dev/onionloom), the two other current
generators, on the same machines.

**The graphics device needs no separate build.** One binary carries every device path and
opens the driver at run time, so the same file works with a card and without
one, and no vendor SDK is needed to build it. `prefix32` puts its device path behind a
cargo feature that wants OpenCL headers at build time, and that build does not
link at all on macOS on ARM. `onionloom` builds its GPU in by default but
through a feature too.

**The graphics device is actually worth using.** On the same M1 Pro this
reaches 256 M candidates a second through Metal, and 1862 M on an RTX 4060
through CUDA — 12.6x what `onionloom` gets from the same device. On the M1 Pro
`onionloom` measures it at 4.35 M/s and falls back to the processor, and
`prefix32` does not support it at all.

**Six matching forms, not one.** Prefix, substring, suffix, free positions,
character classes and regular expressions. `onionloom` takes prefixes only;
`prefix32` takes prefixes with single-position `?d` and `?l` wildcards.

**A dictionary costs what one name costs.** Going from 1 to 1000 names: 66.5 to
68.5 M/s here, 50.7 to 36.0 M/s for `onionloom`. `prefix32` is flat as well.

**More than one machine.** A master hands out seeds and block ranges and
collects the finds; neither of the others has a distributed mode. Both sides
also serve Prometheus metrics and liveness and readiness probes, and there is a
Helm chart.

**A run can be resumed and repeated.** Naming the seed makes a run reproducible
and `--from-block` continues it; `onionloom` has a checkpoint file, `prefix32`
has neither.

**Finds can go into a database.** `--db` takes a connection string for SQLite,
PostgreSQL or MySQL and writes rows instead of a directory per key: 22 700 a
second at worst against 160, which is what a card on a short filter actually
produces. All three of the others write directories and nothing else.

**An image that need not run as root.** Published on every tag for linux/amd64
and linux/arm64 with version tags, on a distroless base — a libc and nothing
else, no shell and no package manager — with the process running as the
unprivileged 65532. `prefix32` and `onionloom` have no Dockerfile at all.
`mkp224o` does publish one, but for amd64 only, under the single tag `master`,
and from `scratch`, which has no user accounts, so it runs as root.

## Security

The secret key is the address. Whoever holds the file owns the address
permanently — it cannot be revoked, only abandoned. Do not commit key
directories or a `--db` database, do not bake them into images, and do not
generate keys on a machine you do not control. More in
[docs/guides/using-generated-keys.md](docs/guides/using-generated-keys.md).

The container image is built on distroless: a libc and the binary, with no shell
and no package manager, running as the unprivileged user 65532. It cannot be
fully static — the device drivers are opened at run time — which is why the libc
is there. It is signed through Sigstore with no key held anywhere: the signature
is issued against a short-lived GitHub token and names the workflow and the
commit that produced the image, which is what `cosign verify` above checks.

## Documentation

- [docs/implementation/](docs/implementation/search-engine.md) — how the
  search engine, the matching and the device paths work, and what each costs.
- [docs/guides/](docs/guides/using-generated-keys.md) — putting a found key to
  use with tor.
- [docs/design/distributed-search.md](docs/design/distributed-search.md) — how
  the master and workers fit together, with diagrams.
- [docs/deployment/](docs/deployment/databases.md) — running a fleet, and
  setting up a database to keep its keys in.

## Licence

Apache License 2.0 — see [LICENSE](LICENSE).
