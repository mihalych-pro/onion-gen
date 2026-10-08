#!/bin/sh
# Prepares tor's directories and starts it as an unprivileged user.
set -eu

KEYS_RO=${KEYS_RO:-/keys-ro}
HS_DIR=${HS_DIR:-/var/lib/tor/hs}
DATA_DIR=${DATA_DIR:-/var/lib/tor/data}
TORRC=${TORRC:-/etc/tor/torrc}

if [ ! -f "$KEYS_RO/hs_ed25519_secret_key" ]; then
	echo "ERROR: $KEYS_RO/hs_ed25519_secret_key not found" >&2
	echo "       mount the key directory at $KEYS_RO" >&2
	exit 1
fi

# Copy out of the mounted volume: the volume is read-only, while tor requires
# ownership of the service directory and mode 0700 on it.
rm -rf "$HS_DIR"
mkdir -p "$HS_DIR" "$DATA_DIR"
cp "$KEYS_RO/hs_ed25519_secret_key" "$HS_DIR/"

# The DataDirectory must belong to the tor user too: a named volume is created
# as root, and without chown tor refuses to start.
chown -R tor:tor "$HS_DIR" "$DATA_DIR"
chmod 700 "$HS_DIR" "$DATA_DIR"
chmod 600 "$HS_DIR/hs_ed25519_secret_key"

exec su tor -s /bin/sh -c "exec tor -f '$TORRC'"
