# Building and delivery

**English** | [Русский](build-and-delivery.ru.md)

One machine builds for every platform. The other machines receive binaries and
not source.

## 1. What you need

On the build machine:

- Rust through `rustup`, at least the version in `rust-version` of
  `Cargo.toml`.
- `zig` and `cargo-zigbuild` (`brew install zig cargo-zigbuild`). zig supplies
  the linker and the libc for every target, and nothing else is needed.
- `go-task` (`brew install go-task`), to run the tasks by name.
- The targets: `task cross:setup`.

On the target machines, nothing at all. That is the point.

## 2. Why zig, and not the alternatives

`mingw-w64` and zig are not equal choices. zig holds mingw-w64 for its Windows
target and adds Linux with a glibc floor on top. mingw alone covers half the
problem.

`cross` runs through Docker. On macOS that is a virtual machine with slow file
access, which is the opposite of a fast local loop. `cross` stays the better
answer when byte-for-byte agreement with CI matters.

zig links through lld, needs no container, and leaves the cache of cargo
alone.

## 3. Building

```bash
task cross:all        # three platforms
task cross:linux      # x86_64-unknown-linux-gnu.2.28
task cross:windows    # x86_64-pc-windows-gnu
task cross:macos      # aarch64-apple-darwin
```

The `.2.28` on the Linux target is the oldest glibc that the binary will run
on. It is part of the target name and not a flag.

It reads backwards from what one expects. glibc is compatible backwards and not
forwards, so a binary built against a new version will not start on an old one.
The linker writes a version against every symbol that it takes — for example
`memcpy@GLIBC_2.14` and not `memcpy` — and it picks the newest version that the
build host offers. zig supplies its own headers and stubs instead, and they
hold every symbol down to the floor named here.

The lower the number, the wider the reach. To raise it only drops
distributions. 2.28 is RHEL 8 and its rebuilds, which have support until May
2029, and nothing older still gets updates.

**A Linux machine can build every target**, the macOS one included. That is the
unobvious case: zig carries libc stubs for macOS and signs the result ad hoc,
and without that signature Apple Silicon kills the binary. One runner is
therefore enough to produce every artefact, which matters where Windows and
macOS runners are not available.

## 4. Shipping

```bash
task ship:linux
task ship:windows
```

Machine addresses come from the environment or from a local `.env`, which
`.gitignore` keeps out of the repository.

Each task copies the binary into a directory of its own on the remote machine
and runs it there. A task that succeeds therefore means a binary that works on
that machine, and not one that merely arrived.

The directory of its own is not decoration. A bare name such as `onion-gen`
can collide with a source tree of that name, and `scp` then writes the binary
inside it.

## 5. Testing on Windows without Rust on the machine

`cargo zigbuild --release --tests --target x86_64-pc-windows-gnu` produces the
test executables. Copy them over and run them there.

Every test runs that way. Nothing in the suite calls an outside program, so
there is no interpreter to install and no path fixed at build time to go
missing on another machine.

## 6. Continuous integration

`.github/workflows/ci.yml` runs formatting, clippy and the tests natively on
Linux, Windows and macOS. It builds the binaries on one Linux runner, except
the macOS ones: `wgpu` links Apple frameworks that exist in no other SDK, so a
Mac builds those. `.gitlab-ci.yml` does the rest on Linux runners only, which
is possible because zig builds every target from there.

Both pipelines compile the CUDA kernel in a job of its own and pass the PTX
down as an artefact. Every job below sets `ONION_GEN_KERNEL_PTX` and stays on
the stable toolchain. Without that, the build script compiles the kernel again
in each job, which is the same PTX several times over and several chances for
one of them to come out different.

The image is packed from the binaries and not compiled again. Both pipelines
build all six once with zig, and then hand the two Linux ones to
`Dockerfile.dist`, which has no compile stage. `Dockerfile` builds from sources
and is what a local build uses. Either way both architectures come off one
amd64 runner with no emulation: the only stages that run a command are pinned
to `$BUILDPLATFORM`, and the published stage only copies files in.

GitHub uses buildx. GitLab uses rootless BuildKit through
`buildctl-daemonless.sh`, which needs neither a privileged runner nor a
docker-in-docker service. Both sign the published image by digest with cosign,
keyless.

The GitHub workflows have run. **`.gitlab-ci.yml` has not**, because there is
no GitLab remote yet. It is valid YAML and nothing more. Keyless signing there
also needs gitlab.com: Fulcio does not trust a self-managed instance as an
issuer, so the job falls back to a key, or skips, as its comments describe.

One detail will bite otherwise. The official `cargo-zigbuild` image carries zig
0.16.0, Rust 1.93.0 and cargo-zigbuild 0.23.4, which we checked by running it.
GitLab uses that image pinned to `0.23.4` and not to `latest`, because zig and
cargo-zigbuild arrive with the image and `latest` would take both out of our
hands. Its Rust is older than the manifest asks for, so every job installs
stable first.

## 7. Versions this was done with

zig 0.17.0, cargo-zigbuild 0.23.4, rustc 1.99.0, go-task from Homebrew.

## 8. Cutting a release

The version of the crate is the source of truth, and the tag follows it. It is
never the other way round.

```bash
# Bump [package] version in Cargo.toml, then:
task release
```

`task release` reads the version from `Cargo.toml`, updates the lock, commits
only `Cargo.toml` and `Cargo.lock`, tags the commit and pushes both. It refuses
to run when the tag exists already, and when the tree holds tracked changes
elsewhere. It asks before it pushes, and `CONFIRM=yes` skips that question.

The same steps by hand:

```bash
cargo check
git commit -am "chore: release 0.2.0"
git tag v0.2.0
git push && git push --tags
```

The tag starts `.github/workflows/release.yml`. That workflow refuses to go
further when the tag and `Cargo.toml` disagree. Nothing downstream reads
`Cargo.toml`, so without that check a mismatched tag would ship a binary whose
`--version` contradicts the release that carries it.

From there the release holds six binaries, their checksums, and notes built
from the commits since the previous tag. The image is published as `0.2.0`,
`0.2`, `0` and `latest`.

Versions are semantic. Before 1.0 a breaking change raises the minor number,
which is where this project is now.
