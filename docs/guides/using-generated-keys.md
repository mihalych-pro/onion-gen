# Putting a generated key pair to use

**English** | [Русский](using-generated-keys.ru.md)

This document tells you what to do with the `<address>.onion/` directory that
the generator made, so that it becomes a working onion service. It covers
operation only.

## Which of the three files you need

```
<address>.onion/
├── hs_ed25519_secret_key    ← the only mandatory one
├── hs_ed25519_public_key    ← tor will regenerate it
└── hostname                 ← tor will regenerate it
```

`hs_ed25519_secret_key` alone is enough: at startup tor derives the public key
and the address from it and rewrites the other two files. Copying the whole
directory is still more convenient — it shows which address is expected, so a
mismatch is immediately visible.

The opposite is not true. `hs_ed25519_public_key` without the secret key is
useless.

## If the keys are in a database

`--db` keeps finds as rows instead of directories — in SQLite, PostgreSQL or
MySQL, whichever the connection string names. A row holds the same bytes the
files hold, headers included, written as base64 for the same reason an SSH key
is: the key bytes are not text. So a key directory is rebuilt by decoding two
columns, and the recipe is the same whichever database it is:

```bash
address=$(sqlite3 keys.db "select address from onion_keys order by score desc limit 1;")
mkdir -m 700 "$address"
sqlite3 keys.db "select secret_key from onion_keys where address = '$address';" \
  | base64 -d > "$address/hs_ed25519_secret_key"
sqlite3 keys.db "select public_key from onion_keys where address = '$address';" \
  | base64 -d > "$address/hs_ed25519_public_key"
echo "$address" > "$address/hostname"
chmod 600 "$address"/*
```

The table also records which filter matched, the score, when it was found, and
the block and scalar offset the key came from — enough to derive it again from
the run's seed.

The database holds secret keys. A SQLite file is created readable only by its
owner, and everything under [What you must not do](#what-you-must-not-do)
applies to it exactly as it applies to the files. Setting up a server, an
account and its grants: [../deployment/databases.md](../deployment/databases.md).

## Option 1: a permanent service through files

### 1. Place the keys

```bash
sudo cp -r abcd1234….onion /var/lib/tor/myservice
sudo chown -R _tor:_tor /var/lib/tor/myservice   # Debian/Ubuntu: debian-tor
sudo chmod 700 /var/lib/tor/myservice
sudo chmod 600 /var/lib/tor/myservice/hs_ed25519_secret_key
```

The tor user name varies: `debian-tor` on Debian and Ubuntu, `tor` on Fedora and
Arch, `_tor` on OpenBSD and macOS/Homebrew. Check with `ps -o user= -C tor` or
in the service unit file.

The permissions are mandatory. tor **refuses** to start when anyone but the
owner can read the service directory. This is not pedantry: the directory holds
a key that gives full control over the address.

### 2. Declare it in `torrc`

```
HiddenServiceDir /var/lib/tor/myservice
HiddenServicePort 80 127.0.0.1:8080
```

`HiddenServicePort <port_inside_Tor> <where_to_proxy>`. On the left is the port
the service exposes; on the right the local address of your application.
Several `HiddenServicePort` lines may share one directory.

The application must listen on `127.0.0.1` specifically: if it listens on
`0.0.0.0` it is simultaneously reachable directly, which defeats the point of an
onion service.

### 3. Restart and verify

```bash
sudo systemctl reload tor        # or restart
sudo cat /var/lib/tor/myservice/hostname
```

The printed address **must** match the directory name the generator produced. If
it does not, tor picked up a different key: most often the directory was not
empty and tor generated a new key on first start, with your file added
afterwards.

Logs, when things go wrong:

```bash
sudo journalctl -u tor -n 50
```

## Option 2: no files, through the ControlPort

This suits short-lived services, and applications that drive tor themselves.

The `ADD_ONION` command takes the secret key in base64 — **without** the 32-byte
file prefix, i.e. exactly 64 bytes:

```
ADD_ONION ED25519-V3:<base64 of 64 bytes> Port=80,127.0.0.1:8080
```

The generator emits exactly that string in `-y --rawyaml` mode (the
`hs_ed25519_secret_key` field) — the same format, no conversion needed.

> **Argument order.** `--rawyaml` must come **before** `-Y`: the `-Y` flag takes
> the following arguments as a file name and an address, so
> `-Y file.yaml --rawyaml` produces `bad onion argument length`. The correct
> order is `--rawyaml -Y file.yaml`.

To obtain it from an existing file:

```bash
tail -c 64 hs_ed25519_secret_key | base64 -w0
```

`tail -c 64` strips the 32-byte prefix. On macOS `base64` has no `-w0`, but it
does not wrap lines by default anyway.

An example over `nc` (the ControlPort must be enabled in `torrc`):

```bash
printf 'AUTHENTICATE ""\r\nADD_ONION ED25519-V3:%s Port=80,127.0.0.1:8080\r\nQUIT\r\n' \
  "$(tail -c 64 hs_ed25519_secret_key | base64)" | nc 127.0.0.1 9051
```

The reply contains `250-ServiceID=<address without .onion>`, which must match
the directory name.

Such a service lives while the control connection stays open, or until
`DEL_ONION`. tor does **not** write it to disk. Add the `Detach` flag when the
service must survive the close of the connection.

## Option 3: a container

```dockerfile
# Keys are NOT copied into the image - they are mounted at run time.
FROM alpine
RUN apk add --no-cache tor
COPY torrc /etc/tor/torrc
USER tor
CMD ["tor", "-f", "/etc/tor/torrc"]
```

```bash
docker run --rm \
  -v "$PWD/abcd1234….onion:/var/lib/tor/myservice:ro" \
  myimage
```

The directory is mounted rather than baked in: a secret key inside an image
layer spreads to everyone who pulls that image and stays in the registry history
even after "deletion".

In Kubernetes the key is a `Secret` mounted with `defaultMode: 0600`, not a
`ConfigMap` and not an environment variable.

A working tor + nginx example driven by `docker compose` lives in
[`examples/docker-onion-nginx/`](../../examples/docker-onion-nginx/), together
with instructions for running it.

## Serving a site through nginx

In the usual arrangement tor listens on the virtual port of the onion service
and proxies to a local nginx. nginx then serves the site.

### torrc

```
HiddenServiceDir /var/lib/tor/myservice
HiddenServicePort 80 127.0.0.1:8080
```

### nginx

A separate `server` block bound to the port from `HiddenServicePort`, with
`server_name` set to the onion address:

```nginx
server {
    listen 127.0.0.1:8080;
    server_name abcd1234….onion;

    root /var/www/myservice;
    index index.html;

    # The client is already inside Tor: encryption and address authentication
    # are provided by the onion service protocol itself. TLS is unnecessary here
    # and only creates certificate trouble for a .onion name.

    # Telling Tor Browser that an onion version exists is possible from the
    # clearnet domain - but the header goes THERE, not here:
    # add_header Onion-Location https://abcd1234….onion$request_uri;

    location / {
        try_files $uri $uri/ =404;
    }
}
```

Listen strictly on `127.0.0.1:8080` rather than `0.0.0.0:8080`: otherwise the
same content is reachable directly by the server's IP, and the link between the
onion address and your server is established trivially.

### When nginx proxies to an application

```nginx
server {
    listen 127.0.0.1:8080;
    server_name abcd1234….onion;

    location / {
        proxy_pass http://127.0.0.1:3000;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-Proto http;

        # Do NOT forward a real IP: an onion client has none, and $remote_addr
        # here is always 127.0.0.1. The header would only suggest that you
        # know where the request came from.
    }
}
```

### What not to do

- **Do not redirect from onion to a clearnet domain.** That throws the user out
  of Tor and exposes them. A common cause is a shared `server` block with
  `return 301 https://example.com$request_uri` as the default.
- **Do not put a TLS certificate on a .onion for the sake of the padlock.** The
  connection is already authenticated by the address. Ordinary CAs do not issue
  such certificates (bar rare EV ones), and a self-signed one warns the user.
- **Do not enable HSTS on an onion domain** and do not let it be inherited from a
  shared config: the policy binds to the name and breaks access when the scheme
  changes.
- **Do not serve identical `Server`/`ETag`/banners on clearnet and onion** — that
  links the two addresses together.
- **Do not log into a shared access.log alongside clearnet traffic** if the goal
  is to keep the two services unlinked. Use a separate `access_log` for the onion
  block.

### Verification

```bash
sudo nginx -t && sudo systemctl reload nginx
curl -s -H 'Host: abcd1234….onion' http://127.0.0.1:8080/ | head
```

A local `curl` checks that nginx answers on the right virtual host. Checking
from outside requires a Tor client:

```bash
curl --socks5-hostname 127.0.0.1:9050 http://abcd1234….onion/
```

## Verifying without starting a service

You can confirm that a key matches an address without bringing tor up:

```bash
./scripts/verify/verify-onion-address.py <directory>.onion
```

The script computes the address from the public key and additionally checks that
the secret key really produces that public key. The second check is slow and can
be disabled with `--no-secret`; `--all` checks every directory instead of the
first twelve.

## What of this has been verified

| Claim | Status |
|---|---|
| `base64(hs_ed25519_secret_key[32:])` is exactly 64 bytes and matches the `--rawyaml` field | verified |
| `tail -c 64 … \| base64` yields the same string `ADD_ONION` needs | verified |
| The round-trip `-y --rawyaml` → `--rawyaml -Y` restores the key and the address matches | verified |
| `--rawyaml` must precede `-Y` | verified (the error was reproduced) |
| tor derives the address from `hs_ed25519_secret_key` alone | verified |
| tor's address matches the generator's | verified |

To check the last two yourself, put **only** `hs_ed25519_secret_key` in the
service directory and start tor with `DisableNetwork 1`. tor creates
`hs_ed25519_public_key` and `hostname` by itself, and it touches no network:

```
$ ls /tmp/tortest/hs        # before starting
hs_ed25519_secret_key

$ tor -f torrc              # DisableNetwork 1 - the descriptor is not published
[notice] DisableNetwork is set. Tor will not make or accept non-control
         network connections.

$ cat /tmp/tortest/hs/hostname
a6k4fysykhkhkob5uhfrooihjdkgoriodvtydtcdgnnwolzzvxvlnyqd.onion   ← matched
```

This also confirms that only the secret key is mandatory among the three files.

tor sets mode `0600` on **every** file in the directory. That is stricter than
the generator, which leaves the public key and `hostname` readable. tor also
creates an `authorized_clients/` subdirectory for client authorisation. The
changed permissions and the extra directory are tor working normally. They are
not key corruption.

This check publishes nothing. The `DisableNetwork 1` flag stops tor before it
announces the descriptor.

## Onionbalance

Onionbalance accepts an ordinary `hs_ed25519_secret_key` as the frontend key, so
a generated address can serve as the entry point for a set of backends. The key
format is the same and needs no separate preparation.

## What you must not do

- **Do not reuse one key for several services.** The address *is* the public
  key; two services on one address are indistinguishable to a client and link
  your services together.
- **Do not put a secret key in git, a container image or a ConfigMap.** Whoever
  obtains the file owns the address forever; it cannot be revoked, only replaced.
- **Do not generate keys on someone else's machine or through a web service.**
  The machine's owner sees the key.
- **Do not rely on password-derived keys for unrelated services.** Keys from one
  password are related: leaking one helps reconstruct its neighbours.
- **Do not leave an unencrypted backup.** A backup of the service directory is a
  backup of full control over the address.

## Rotating a key

You cannot revoke a compromised address. You can only abandon it. The steps
are: generate a new address, run both at the same time, announce the move, and
switch the old one off after a while.

You will lose the clients who know only the old address. That is why the key is
worth protecting.
