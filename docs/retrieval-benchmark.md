# Retrieval Benchmark

`scripts/retrieval_bench.py` is the repeatable retrieval benchmark for `ctx query`.

The benchmark is built from researched, paraphrased facts in `benchmarks/retrieval_cases.jsonl`. Each case records:

- the library label
- the official source URL used for research
- a question an agent might ask
- answer terms that must surface in retrieved context
- short docs text used to generate a local fixture page

The runner creates a local HTTP docs site and indexes it with `ctx add`. It can run the same cases against two corpus sizes:

- `small`: only the researched docs pages plus `llms.txt`
- `large`: the same pages plus deterministic noise pages under each library

Default command:

```sh
make bench-retrieval
```

Useful faster command:

```sh
cargo build --release
python3 scripts/retrieval_bench.py --mode both --embeddings off
```

Each run writes under `bench-results/retrieval/<timestamp>`:

- `cases.jsonl`: the exact case set
- `results.jsonl`: index and query observations
- `summary.md`: pass rate, hybrid evidence, timing, and failures
- per-case stdout logs for debugging ranking details

The harness intentionally uses a local HTTP server instead of live docs during scoring. Live docs are used for research, but benchmark scoring must stay stable so retrieval regressions are attributable to `ctx`, not third-party site changes.

Reference run on 2026-05-24:

- command: `make bench-retrieval`
- run id: `20260524T110537Z`
- cases: 22
- modes: `small`, `large`
- large noise: 64 extra pages per library
- result: `22/22` passed in both modes
- hybrid evidence: `22/22` queries reported `rrf_hybrid` in both modes
- `llms.txt` prior evidence: `22/22` queries reported `source_prior: llms_txt` in both modes
- query mean/max: small `0.054s / 0.056s`, large `0.057s / 0.061s`
