#!/usr/bin/env python3
"""Independent verification of Onion Service v3 keys.

Two checks:
  1. the address is derived from hs_ed25519_public_key following the description
     in docs/analysis/onion-v3-protocol.md and compared against the hostname file;
  2. the scalar from hs_ed25519_secret_key, multiplied by the base point, yields
     the public key stored in hs_ed25519_public_key.

Everything is computed here in plain Python: no mkp224o code and no external
crypto libraries. That is the whole point - if the description in the document
is wrong, or the implementation is, the check fails.
"""
import base64
import hashlib
import pathlib
import sys

PK_PREFIX = b"== ed25519v1-public: type0 ==\x00\x00\x00"
SK_PREFIX = b"== ed25519v1-secret: type0 ==\x00\x00\x00"
CHECKSUM_SALT = b".onion checksum"
VERSION = 3

# --- ed25519 arithmetic, a reference implementation per RFC 8032 ------------
P = 2**255 - 19
D = (-121665 * pow(121666, P - 2, P)) % P
BX = 15112221349535400772501151409588531511454012693041857206046113283949847762202
BY = 46316835694926478169428394003475163141307993866256225615783033603165251855960


def _point_add(p1, p2):
    """Addition in extended coordinates (X:Y:Z:T)."""
    x1, y1, z1, t1 = p1
    x2, y2, z2, t2 = p2
    a = (y1 - x1) * (y2 - x2) % P
    b = (y1 + x1) * (y2 + x2) % P
    c = 2 * t1 * t2 * D % P
    dd = 2 * z1 * z2 % P
    e, f, g, h = b - a, dd - c, dd + c, b + a
    return (e * f % P, g * h % P, f * g % P, e * h % P)


def _scalar_mult_base(scalar: int):
    """[scalar]B by double-and-add."""
    q = (0, 1, 1, 0)  # the identity element
    p = (BX, BY, 1, BX * BY % P)
    while scalar > 0:
        if scalar & 1:
            q = _point_add(q, p)
        p = _point_add(p, p)
        scalar >>= 1
    return q


def _compress(point) -> bytes:
    """Packs a point: y with the sign bit of x in the top position."""
    x, y, z, _ = point
    zinv = pow(z, P - 2, P)
    x, y = x * zinv % P, y * zinv % P
    return ((y | ((x & 1) << 255))).to_bytes(32, "little")


def public_from_secret(scalar_bytes: bytes) -> bytes:
    """Public key from an ALREADY expanded and clamped scalar (32 bytes, LE).

    SHA-512 is not applied again here: the file holds a ready scalar rather than
    a seed (see onion-v3-protocol.md, section 3).
    """
    return _compress(_scalar_mult_base(int.from_bytes(scalar_bytes, "little")))


def onion_address(pubkey: bytes) -> str:
    if len(pubkey) != 32:
        raise ValueError(f"the public key must be 32 bytes, got {len(pubkey)}")
    digest = hashlib.sha3_256(CHECKSUM_SALT + pubkey + bytes([VERSION])).digest()
    checksum = digest[:2]
    blob = pubkey + checksum + bytes([VERSION])           # 35 bytes
    return base64.b32encode(blob).decode("ascii").lower() + ".onion"


def read_pubkey(path: pathlib.Path) -> bytes:
    raw = path.read_bytes()
    if len(raw) != 64:
        raise ValueError(f"{path}: expected 64 bytes, got {len(raw)}")
    if raw[:32] != PK_PREFIX:
        raise ValueError(f"{path}: unexpected prefix {raw[:32]!r}")
    return raw[32:]


def read_scalar(path: pathlib.Path) -> bytes:
    raw = path.read_bytes()
    if len(raw) != 96:
        raise ValueError(f"{path}: expected 96 bytes, got {len(raw)}")
    if raw[:32] != SK_PREFIX:
        raise ValueError(f"{path}: unexpected prefix {raw[:32]!r}")
    return raw[32:64]


def check_dir(d: pathlib.Path, check_secret: bool) -> tuple[bool, str]:
    pub = d / "hs_ed25519_public_key"
    host = d / "hostname"
    for f in (pub, host):
        if not f.is_file():
            return False, f"missing file {f.name}"

    expected = host.read_text().strip()
    pubkey = read_pubkey(pub)
    actual = onion_address(pubkey)
    if actual != expected:
        return False, f"address mismatch:\n    hostname: {expected}\n    computed: {actual}"
    if d.name != expected:
        return False, f"directory name {d.name!r} != hostname {expected!r}"

    if check_secret:
        sec = d / "hs_ed25519_secret_key"
        if not sec.is_file():
            return False, "missing file hs_ed25519_secret_key"
        derived = public_from_secret(read_scalar(sec))
        if derived != pubkey:
            return False, (f"the secret does not yield the public key:\n"
                           f"    in file:  {pubkey.hex()}\n"
                           f"    computed: {derived.hex()}")
    return True, expected


def main(argv: list[str]) -> int:
    args = [a for a in argv[1:] if not a.startswith("--")]
    # The secret key check is slow (scalar multiplication in plain Python), so
    # only a subset of directories is taken by default.
    check_secret = "--no-secret" not in argv
    limit = 0 if "--all" in argv else 12

    if len(args) != 1:
        print(f"usage: {argv[0]} [--no-secret] [--all] "
              f"<directory holding *.onion subdirectories | a single *.onion directory>",
              file=sys.stderr)
        return 2

    root = pathlib.Path(args[0])
    if (root / "hs_ed25519_public_key").is_file():
        dirs = [root]
    else:
        dirs = sorted(p for p in root.iterdir() if p.is_dir())

    if not dirs:
        print("no key directories found", file=sys.stderr)
        return 1

    total = len(dirs)
    if limit and len(dirs) > limit:
        dirs = dirs[:limit]
        print(f"checking {limit} directories out of {total} "
              f"(--all to check them all)")

    ok = bad = 0
    for d in dirs:
        good, msg = check_dir(d, check_secret)
        if good:
            ok += 1
            print(f"  OK   {msg}")
        else:
            bad += 1
            print(f"  FAIL {d.name}: {msg}")

    print(f"\nchecked: {ok + bad}, matched: {ok}, failed: {bad}")
    return 0 if bad == 0 else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
