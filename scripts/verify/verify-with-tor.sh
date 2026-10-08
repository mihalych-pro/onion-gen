#!/usr/bin/env bash
#
# Confirms that tor derives the same address from a found secret key.
#
# The network stays off. `DisableNetwork 1` means tor builds no circuits and
# publishes no descriptor: bringing the service up is a check that the key is
# well formed, not an act of publication. Nothing here reaches the live network.
#
# Usage: verify-with-tor.sh <key-directory> [<key-directory> ...]
#
# A key directory is one produced by onion-gen, named after its address.

set -euo pipefail

if [ $# -lt 1 ]; then
    echo "usage: $0 <key-directory> [...]" >&2
    exit 2
fi

command -v tor >/dev/null || { echo "tor is not installed" >&2; exit 2; }

checked=0
failed=0

for key_dir in "$@"; do
    expected="$(basename "${key_dir%/}")"
    expected="${expected%.onion}"

    if [ ! -f "$key_dir/hs_ed25519_secret_key" ]; then
        echo "FAIL $expected: no hs_ed25519_secret_key" >&2
        failed=$((failed + 1))
        continue
    fi

    work="$(mktemp -d)"
    svc="$work/service"
    mkdir -p "$svc" "$work/data"
    chmod 700 "$svc" "$work/data"
    cp "$key_dir/hs_ed25519_secret_key" "$svc/"
    chmod 600 "$svc/hs_ed25519_secret_key"

    cat > "$work/torrc" <<TORRC
DisableNetwork 1
SocksPort 0
RunAsDaemon 0
DataDirectory $work/data
HiddenServiceDir $svc
HiddenServicePort 80 127.0.0.1:8080
Log err file $work/tor.log
TORRC

    tor -f "$work/torrc" >/dev/null 2>&1 &
    tor_pid=$!

    # tor writes the hostname as it loads the service, well before it would
    # reach out to anything. Ten seconds is generous for that.
    for _ in $(seq 1 100); do
        [ -s "$svc/hostname" ] && break
        kill -0 "$tor_pid" 2>/dev/null || break
        sleep 0.1
    done

    kill "$tor_pid" 2>/dev/null || true
    wait "$tor_pid" 2>/dev/null || true

    if [ ! -s "$svc/hostname" ]; then
        echo "FAIL $expected: tor produced no hostname" >&2
        sed -n '1,5p' "$work/tor.log" >&2 || true
        failed=$((failed + 1))
    else
        actual="$(tr -d '[:space:]' < "$svc/hostname")"
        actual="${actual%.onion}"
        if [ "$actual" = "$expected" ]; then
            echo "ok   $expected"
        else
            echo "FAIL $expected: tor says $actual" >&2
            failed=$((failed + 1))
        fi
    fi

    checked=$((checked + 1))
    rm -rf "$work"
done

echo "tor checked: $checked, failed: $failed"
[ "$failed" -eq 0 ]
