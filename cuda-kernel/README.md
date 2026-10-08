# cuda-kernel

Source of the PTX the CUDA path loads at run time.

It is a separate crate for one reason: it targets `nvptx64-nvidia-cuda`, and the
`ptx-kernel` ABI and the inline assembly it uses are still nightly-only. Keeping
it apart means the main crate stays on stable.

Nothing here has to be built by hand. The main crate's `build.rs` compiles this
one on every build and hands the PTX over through `OUT_DIR`, so the kernel in a
binary is always the kernel these sources describe. The compiled form is not
kept in the repository: a kernel edited without it being refreshed would produce
wrong addresses and no error at all.

`rust-toolchain.toml` names the compiler, and `rustup` fetches it the first time
anything builds. That is the one requirement a clean checkout adds. The version
is pinned rather than tracking nightly, because a compiler changing underneath
would change what runs on the card with nothing being said.

A build that has the PTX already can hand it over instead, through
`ONION_GEN_KERNEL_PTX=<path>`; the build script then copies that file and does
not reach for nightly. The container build uses it so that the kernel occupies
a layer of its own and is not rebuilt when only the host code changed. Setting
it by hand means taking responsibility for the file being current.

To compile it alone and look at the result:

```bash
task kernel:ptx
```

That also checks the two properties a change here can silently lose: the
widening multiply has to survive into the PTX, and nothing may spill. A stack
frame in `chain_steps` or `chain_reduce` means the point is living in off-chip
memory, which cost a third of the throughput the last time it happened. CI runs
the same checks.
