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
task cross:linux      # x86_64-unknown-linux-gnu.2.17
task cross:windows    # x86_64-pc-windows-gnu
task cross:macos      # aarch64-apple-darwin
```

The `.2.17` on the Linux target is the oldest glibc the binary will run on, and
it is part of the target name rather than a flag.

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
Linux, Windows and macOS, and builds all three binaries from one Linux runner.
`.gitlab-ci.yml` does the same on Linux runners only, which is possible because
zig builds every target from there.

Both are written and their YAML is valid. **Neither has been through a real
run**: the repository has no remotes yet. They are not to be trusted until they
have.

One detail that will bite otherwise: the official `cargo-zigbuild` image pins
an older toolchain than the manifest asks for, so every job installs stable
first.

## 7. Versions this was done with

zig 0.16.0, cargo-zigbuild 0.23.4, rustc 1.99.0, go-task from Homebrew.

## 8. Continuous integration: written, not yet run

Both configurations are in the repository and their content is grounded in what
was checked by hand: one runner's platform is enough to produce all three
binaries, and the `cargo-zigbuild` image pins an older toolchain than the
manifest asks for, so every job switches to stable first. Every job that builds
the crate also fetches the kernel's toolchain, because the build script
compiles the GPU kernel to PTX — on a runner with no CUDA, which is the
property most easily lost by accident.

The container build does it once instead, in a stage of its own: the PTX is
architecture-independent, so one compilation serves both image architectures,
and a change to the host code leaves that layer cached. The stage passes the
file to the build script through `ONION_GEN_KERNEL_PTX`, which is why the
compile stage itself needs no nightly.

**None of it has been through a real run.** The repository has no remotes, and
by the owner's decision they come at the end of the project. Until then these
files are a plan rather than a fact, and are marked as such rather than counted
as done.
