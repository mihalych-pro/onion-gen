# Keeping keys in a database

**English** | [Русский](databases.ru.md)

`--db` takes one connection string and decides from it which kind of store to
open. The default, without the flag, is a directory per key.

```bash
onion-gen -F abcd --db keys.db                                  # a SQLite file
onion-gen -F abcd --db sqlite://keys.db                         # the same
onion-gen -F abcd --db postgres://user:pass@host:5432/onion     # PostgreSQL
onion-gen -F abcd --db mysql://user:pass@host:3306/onion        # MySQL
```

A master takes the same flag, and that is where it matters most: every find in
a fleet is written by the one master.

```bash
onion-gen -F abcd master --listen :8080 --db postgres://user:pass@db/onion
```

The password never appears in what the program prints. A startup line and an
error both show the connection string with it replaced by `***`.

## Which to use

| Store | Finds a second | Needs |
|---|---|---|
| directories | 162 | nothing |
| SQLite | 244 000 | nothing |
| PostgreSQL | 32 000 | a server |
| MySQL | 22 700 | a server |

Measured with `cargo bench --bench store-rate`; the servers were containers on
the same machine, so a real network will be slower and the ordering will not
change.

A directory per key is a directory, three files and an `fsync`. One RTX 4060 on
a four-symbol filter finds some 950 keys a second, which that cannot take and
any of the other three can. The program works the expected rate out at startup
and says so.

Use a server when more than one thing needs to read the keys, when the keys
should live somewhere that is already backed up, or when a fleet's master would
otherwise hold the only copy. Use SQLite otherwise: it is a file, it needs no
account, and it is the fastest of the four.

## What the schema is

One table, the same shape in all three:

| Column | Holds |
|---|---|
| `address` | the full `.onion` name, and the primary key |
| `secret_key` | base64 of `hs_ed25519_secret_key`, headers included |
| `public_key` | base64 of `hs_ed25519_public_key` |
| `filter` | the filter that matched, as it was written |
| `score` | what the find scored |
| `found_at` | unix seconds |
| `block`, `scalar_offset` | where in the search it sits |
| `source` | which engine found it |

The keys are base64 because the bytes are not text — the same reason an SSH key
is written that way. A row therefore turns back into the files tor reads with
`base64 -d` and nothing else:

```bash
address=$(psql -tAc "select address from onion_keys order by score desc limit 1")
mkdir -m 700 "$address"
psql -tAc "select secret_key from onion_keys where address = '$address'" \
  | base64 -d > "$address/hs_ed25519_secret_key"
psql -tAc "select public_key from onion_keys where address = '$address'" \
  | base64 -d > "$address/hs_ed25519_public_key"
echo "$address" > "$address/hostname"
chmod 600 "$address"/*
```

The table is called `onion_keys` rather than `keys` because `keys` is a
reserved word in MySQL.

## Migrations

A second table, `onion_gen_migrations`, records which schema steps have run:
`version`, `name`, `applied_at`. Every start checks it and applies whatever is
missing, so the first run against an empty database creates the schema and
every later run finds nothing to do.

Each database has its own list of steps, because the dialects disagree about
types — `DOUBLE PRECISION` against `DOUBLE` against `REAL` — while the versions
are one sequence across all three, so "schema 1" names the same shape wherever
it is.

Nothing is ever dropped or rewritten by a migration. A column that stops being
used is left where it is.

## Setting up PostgreSQL

A role and a database of its own, so that a lost password costs the keys and
nothing else:

```sql
CREATE ROLE onion_gen LOGIN PASSWORD 'something long and random';
CREATE DATABASE onion OWNER onion_gen;
```

That is enough: as the owner, the role can create its tables on first run.

To keep it from owning the database — the tighter arrangement, where an
administrator creates the database and the program only writes to it — run the
program once as a role that can create tables, then narrow it:

```sql
-- as the administrator, after the first run has built the schema
REVOKE CREATE ON SCHEMA public FROM onion_gen;
GRANT SELECT, INSERT ON onion_keys TO onion_gen;
GRANT SELECT ON onion_gen_migrations TO onion_gen;
```

The program needs `SELECT` as well as `INSERT`: it asks whether an address is
already stored before it takes a find, so that a repeat is reported rather than
counted twice. It never updates or deletes.

Note what the narrower arrangement costs: the program can no longer apply a
migration, so a version that adds one has to be let through by an administrator
first. The looser arrangement is the one to pick unless there is a reason.

Connection string:

```
postgres://onion_gen:something%20long@db.internal:5432/onion
```

A password with characters that mean something in a URL — `@`, `/`, `:`, `?`,
`#` — has to be percent-encoded. A password that is only letters and digits
avoids the question.

TLS is used when the server asks for it; the client carries its own roots and
needs no system OpenSSL. To require it, add `?sslmode=require`.

## Setting up MySQL

```sql
CREATE DATABASE onion CHARACTER SET utf8mb4 COLLATE utf8mb4_bin;
CREATE USER 'onion_gen'@'%' IDENTIFIED BY 'something long and random';
GRANT SELECT, INSERT, CREATE, INDEX ON onion.* TO 'onion_gen'@'%';
```

`CREATE` and `INDEX` are for the first run, which builds the schema. They can
be revoked afterwards, with the same caveat as above:

```sql
REVOKE CREATE, INDEX ON onion.* FROM 'onion_gen'@'%';
```

The collation matters. An address is base32 and two addresses differing only in
case are two different addresses; under the default case-insensitive collation
the primary key would treat them as one. The schema sets `utf8mb4_bin` on the
table itself, so a database created with another collation still behaves — but
setting it on the database as well keeps anything added later consistent.

Connection string:

```
mysql://onion_gen:something@db.internal:3306/onion
```

## Setting up SQLite

Nothing to set up. The file is created if it is not there, together with any
missing parent directory, and is narrowed to its owner the moment it exists —
before anything is written to it.

It is opened in WAL mode with `synchronous=FULL`, so two more files appear
beside it, `-wal` and `-shm`. All three belong together: copying only the
database file while the program is running copies an incomplete one.

## What is written, and when

Finds are batched: a thousand of them, or one second, whichever comes first.
This is what makes a database worth the trouble — a thousand rows share one
transaction and one flush.

It is also the one thing to know about it. A find is printed when it is found
and stored within a second, so a process killed inside that second loses what
it was holding. Every ordinary exit flushes first, and so does reaching
`--limit`.

An address that is already stored is not written twice and is not an error. The
program asks before it takes a find, which is what keeps the find count honest
when a master hands the same range out after a lease expires.

## Metrics

A master publishes what its store is doing, labelled by kind:

| Metric | Means |
|---|---|
| `onion_gen_store_connections` | connections held open; a file counts as one |
| `onion_gen_store_connections_idle` | of those, how many are not in use |
| `onion_gen_store_rows_total` | keys written; a repeat is not counted |
| `onion_gen_store_bytes_total` | key material written, 160 bytes a key |
| `onion_gen_store_batches_total` | batches committed |
| `onion_gen_store_failures_total` | batches the store refused |

`onion_gen_store_failures_total` above zero is the one worth alerting on: a
fleet whose master cannot write is a fleet losing keys it has already paid for.
Connections dropping to zero says the server went away.

The label is a fixed word — `directory`, `sqlite`, `postgres`, `mysql` — and
never anything out of a connection string, so a host name cannot leak into
whatever scrapes the metrics.

## Security

The database holds secret keys. Whoever can read `secret_key` owns those
addresses permanently.

- Give the account its own database, not one shared with anything else.
- Do not commit a connection string. Pass it through `ONION_GEN_DB` or a
  Kubernetes Secret rather than on a command line, where anything that can list
  processes on the node can read it.
- Back the database up the way you would back up the key directories, and
  guard the backups the same way.
- A SQLite file is created `0600`. Keep it that way, and remember the `-wal`
  file holds the same material.
