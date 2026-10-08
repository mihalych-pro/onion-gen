# Example: an onion service on generated keys (tor + nginx)

**English** | [Русский](README.ru.md)

A working pair of containers: `tor` brings up an onion service on your keys and
`nginx` serves the site. Nothing is published outward — the site is reachable
only through Tor.

## Contents

```
docker-compose.yml           tor + nginx
docker-compose.offline.yml   an override for checking without reaching the Tor network
tor/Dockerfile               the image with tor
tor/entrypoint.sh            prepares directories and starts as the tor user
tor/torrc                    the service configuration
tor/torrc.offline            the same plus DisableNetwork 1
nginx/onion.conf             the server block for the onion address
site/index.html              a demonstration page
keys/                        PUT the keys here (kept out of git)
```

## Running it

### 1. Place the keys

Only one file is needed from the directory the generator produced:

```bash
mkdir -p keys
cp /path/to/abcd1234….onion/hs_ed25519_secret_key keys/
```

The secret key alone is enough: tor derives the public key and `hostname` from
it. The log shows this as
`Found secret key but not hs_ed25519_public_key. Regenerating.`

The `keys/` directory is listed in `.gitignore` — the secret key must not reach
the repository.

### 2. Check that the right key is mounted

Before publishing the service to the network, make sure the address is the one
you expect:

```bash
docker compose -f docker-compose.yml -f docker-compose.offline.yml up -d --build
docker compose exec tor cat /var/lib/tor/hs/hostname
```

The printed address must match the directory name the generator produced. This
mode enables `DisableNetwork 1`: tor reads the key and creates `hostname` but
**does not announce the service** on the Tor network.

```bash
docker compose -f docker-compose.yml -f docker-compose.offline.yml down
```

### 3. Bring it up for real

```bash
docker compose up -d --build
docker compose logs -f tor
```

Wait for `Bootstrapped 100%`. After that the address opens in Tor Browser.

A command-line check, if you have a local Tor client on 9050:

```bash
curl --socks5-hostname 127.0.0.1:9050 \
  "http://$(docker compose exec -T tor cat /var/lib/tor/hs/hostname)/"
```

## How it works

**Keys are mounted, not copied into the image.** A secret that lands in an image
layer spreads to everyone who pulls that image and stays in the registry history
even after "deletion". The volume is mounted read-only (`:ro`), and the
container copies the key into `/var/lib/tor/hs` and sets ownership there — the
source directory is left untouched.

**tor does not run as root.** `entrypoint.sh` prepares the directories and hands
control to the `tor` user through `exec su`.

**The `DataDirectory` is handed to the `tor` user too.** A named volume is
created as root, and without `chown` tor fails with
`Couldn't create private data directory`. That is not a theoretical nitpick —
this very example reproduced the error on its first build.

**No port is published to the host.** The `nginx` service uses `expose` rather
than `ports`: opening a port on the host would make the same content reachable
directly by the server's IP, and the link between the onion address and your
machine would be established trivially.

**A separate access log for onion traffic.** Mixing onion traffic with clearnet
in one log means linking the two services together.

## Replacing the demonstration site

Put your own files in `site/`, or replace `nginx` with your application and
adjust `HiddenServicePort` in `tor/torrc`:

```
HiddenServicePort 80 myapp:3000
```

The application must listen inside the compose network and must not publish a
port to the host.

## What was verified

The setup was checked in this configuration: tor 0.4.9.11 on `alpine:3.20`,
`nginx:1.27-alpine`, started in offline mode. tor printed exactly the address
the generator had produced, and nginx served the page.

An end-to-end check with the descriptor actually published to the Tor network
**was not performed** — that requires announcing the service outward.
