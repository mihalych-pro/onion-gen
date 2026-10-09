#!/usr/bin/env bash
# Runs the test suite on the windows machine named by ONION_GEN_WINDOWS_HOST.
#
# Linking the tests for windows says that no symbol is missing. It does not say
# that the program behaves there, and the two are different questions: a prose
# line in a `--json` stream links perfectly well.
#
# The tests find the program from their own path, so the layout on the remote
# has to match a cargo target directory: harnesses in `deps`, the program one
# level above them.
set -euo pipefail

target="${1:-x86_64-pc-windows-gnu}"
host="${ONION_GEN_WINDOWS_HOST:?set ONION_GEN_WINDOWS_HOST=user@host}"
# Relative to the remote home directory: a path with a user name in it is a
# detail of one machine, and this file is committed.
remote='ogtests'

cargo zigbuild --release --tests --target "$target" >&2

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/deps"
cp "target/${target%%.*}/release/onion-gen.exe" "$stage/"

# Which executables are harnesses is cargo's answer to give: the names in
# `deps` carry a hash, and the program sits among them under a similar one.
cargo zigbuild --release --tests --target "$target" --message-format=json 2>/dev/null \
  | STAGE="$stage" python3 -c '
import sys, json, shutil, os
stage = os.environ["STAGE"]
for line in sys.stdin:
    try:
        m = json.loads(line)
    except ValueError:
        continue
    if m.get("reason") != "compiler-artifact" or not m.get("profile", {}).get("test"):
        continue
    exe = m.get("executable")
    if exe:
        name = m["target"]["name"]
        shutil.copy(exe, stage + "/deps/" + name + ".exe")
'

ssh "$host" "rmdir /s /q $remote 2>nul & mkdir $remote\\deps"
scp -q "$stage"/onion-gen.exe "$host:$remote/"
scp -q "$stage"/deps/*.exe "$host:$remote/deps/"

ssh "$host" "cd $remote\\deps && for %f in (*.exe) do @(echo === %f & %f)"
