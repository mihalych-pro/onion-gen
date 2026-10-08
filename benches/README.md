# Benchmarks

The measurement harnesses behind the numbers quoted in [docs/](../docs/). Each
one measures a single thing in isolation — none of them is sample code.

```bash
cargo bench --bench index-width
```

They are plain `fn main()` programs rather than `#[bench]` functions, declared
in `Cargo.toml` with `harness = false`.

| Benchmark | Measures |
|---|---|
| `field-bench` | field multiplication: scalar against NEON and AVX2 |
| `gpu-chain` | the device kernel on its own |
| `hit-rate` | how often a filter hits, against theory |
| `index-width` | throughput against bitmap index width |
| `master-load` | how many requests a master answers |
| `match-bench` | matching with the index against a plain scan |
| `regex-cost` | what a regular expression costs over a literal |
| `score-calibration` | the table mapping a score to a measured share |
| `store-rate` | writing finds down: key directories against database rows |
| `substring-scaling` | substring search against the filter count |
