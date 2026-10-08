# Building and delivery

**English** | [Русский](build-and-delivery.ru.md)

One machine builds for all three platforms; the other machines receive
binaries, not source.

## 1. What you need

On the build machine:

- Rust via `rustup`, at least the version in `rust-version` of `Cargo.toml`.
- `zig` and `cargo-zigbuild` (`brew install zig cargo-zigbuild`). zig supplies
  the linker and the libc for every target; nothing else is needed.
- `go-task` (`brew install go-task`) to run the tasks by name.
- The targets: `task cross:setup`.

Nothing at all on the target machines. That is the point.

## 2. Why zig and not the alternatives

`mingw-w64` and zig are not equal choices: zig contains mingw-w64 for its
Windows target and adds Linux with a glibc floor on top. mingw alone would
cover half the problem.

`cross` runs through Docker, which was not running on the build machine, and on
macOS that is a virtual machine with slow file access — the opposite of a fast
local loop. It remains the better answer when byte-for-byte agreement with CI
matters.

zig links through lld, needs no container, and leaves cargo's caching alone.

## 3. Building

```bash
task cross:all        # all three platforms
task cross:linux      # x86_64-unknown-linux-gnu.2.28
task cross:windows    # x86_64-pc-windows-gnu
task cross:macos      # aarch64-apple-darwin
```

The `.2.28` on the Linux target is the oldest glibc the binary will run on, and
it is part of the target name rather than a flag.

It reads backwards from what one expects: glibc is compatible backwards and not
forwards, so a binary built against a new one will not start on an old one. The
linker writes a version against every symbol it takes — `memcpy@GLIBC_2.14` and
not `memcpy` — and picks the newest the build host offers. zig supplies its own
headers and stubs instead, which holds every symbol down to the floor named
here. So the lower the number, the wider the reach, and raising it only drops
distributions. 2.28 is RHEL 8 and its rebuilds, supported to May 2029; nothing
older is still getting updates.

All three were built on `darwin/arm64` and run on their own machines: the Linux
one on Debian 13, the Windows one on Windows 11 outside WSL, the macOS one back
on Apple Silicon. Each detected AVX2 or NEON by itself and found an address.

**Linux can build all three as well**, including the macOS target, which is the
unobvious one: zig carries libc stubs for macOS and signs the result ad hoc,
without which Apple Silicon would kill the binary. That is why one runner is
enough to produce every artifact, which matters where Windows and macOS runners
are not available.

## 4. Shipping

```bash
task ship:linux
task ship:windows
```

Machine addresses come from the environment or a local `.env`, which
`.gitignore` keeps out of the repository. Each task copies the binary into a
directory of its own on the remote and runs it there, so a successful task
means a binary that actually works on that machine, not one that merely copied.

The directory matters: shipping to a bare `onion-gen` collided with a source
tree of that name and `scp` wrote the binary inside it.

## 5. Testing on Windows without installing Rust

`cargo zigbuild --release --tests --target x86_64-pc-windows-gnu` produces the
test executables; copy them over and run them. 109 tests pass natively that
way.

One test cannot run: `end_to_end` invokes the Python verifier through a path
baked in at build time, which does not exist on another machine. It now skips
with a message rather than failing, and the message is printed rather than
swallowed — a test that quietly checks nothing is worse than one that fails.

## 6. Continuous integration

`.github/workflows/ci.yml` runs formatting, clippy and the tests natively on
Linux, Windows and macOS, and builds the binaries from one Linux runner — except
the macOS ones, which are built on a Mac because `wgpu` links Apple frameworks
that exist in no other SDK. `.gitlab-ci.yml` does the rest on Linux runners
only, which is possible because zig builds every target from there.

Both compile the CUDA kernel in a job of its own and pass the PTX down as an
artefact. Every job below it sets `ONION_GEN_KERNEL_PTX` and so stays on the
stable toolchain; without that the build script compiles the kernel again in
each job, which is the same PTX several times over and several chances for one
of them to come out different.

The image is packed from the binaries rather than compiled again: both
pipelines build all six once, with zig, and then hand the two linux ones to
`Dockerfile.dist`, which has no compile stage. `Dockerfile` still builds from
sources and is what a local build uses. Either way both architectures come off
one amd64 runner with no emulation — the only stages that run a command are
pinned to `$BUILDPLATFORM`, and the published stage just copies files in.

GitHub uses buildx; GitLab uses rootless BuildKit through
`buildctl-daemonless.sh`, which wants neither a privileged runner nor a
docker-in-docker service. Both sign the published image by digest with cosign,
keyless.

The GitHub workflows have run. **`.gitlab-ci.yml` has not**: there is no GitLab
remote yet, so it is valid YAML and nothing more. Note also that keyless signing
there needs gitlab.com — Fulcio does not trust a self-managed instance as an
issuer, and the job falls back to a key, or skips, as its comments describe.

One detail that will bite otherwise: the official `cargo-zigbuild` image carries
zig 0.16.0, Rust 1.93.0 and cargo-zigbuild 0.23.4 — checked by running it, not
read off a page. GitLab uses that image pinned to `0.23.4` rather than `latest`,
because zig and cargo-zigbuild arrive with it and `latest` would take both out
of our hands. Its Rust is older than the manifest asks for, so every job
installs stable first.

## 7. Versions this was done with

zig 0.17.0, cargo-zigbuild 0.23.4, rustc 1.99.0, go-task from Homebrew.

## 8. Cutting a release

The crate's version is the source of truth and the tag follows it, never the
other way round:

```bash
# 1. Bump [package] version in Cargo.toml, then let the lock catch up.
cargo check
# 2. Commit it on its own.
git commit -am "chore: release 0.2.0"
# 3. Tag it with the same number and a leading v.
git tag v0.2.0
git push && git push --tags
```

The tag starts `.github/workflows/release.yml`, which refuses to go further if
the two disagree: nothing downstream reads `Cargo.toml`, so without that check a
mismatched tag would ship a binary whose `--version` contradicts the release it
came in. From there the release carries six binaries, their checksums and notes
built from the commits since the previous tag, and the image is published as
`0.2.0`, `0.2`, `0` and `latest`.

Versions are semantic. Before 1.0 a breaking change bumps the minor, which is
where this is now.
