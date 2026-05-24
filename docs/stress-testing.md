# Docs Stress Testing

`scripts/docs_stress.py` is the crawler/indexer stress harness. It is inspired by autoresearch-style runs: fixed targets, isolated state, append-only JSONL observations, and a markdown summary that can be compared across attempts.

The harness defaults are intentionally polite:

- isolated `CTX_HOME` under `bench-results/docs-stress/<timestamp>/ctx-home`
- one `ctx add` process at a time
- low in-process crawl concurrency
- delay plus jitter between targets
- per-target timeout
- max RSS and cache-size stop guards

Typical flow:

```sh
cargo build --release
python3 scripts/docs_stress.py discover --limit 100 --out bench-results/context7-targets.jsonl
python3 scripts/docs_stress.py run \
  --targets bench-results/context7-targets.jsonl \
  --limit 100 \
  --ctx-bin target/release/ctx \
  --max-pages 8 \
  --concurrency 2 \
  --embeddings off
```

Use `--embeddings on` for end-to-end embedding stress. Keep `--limit` low for the first pass because embedding large `llms.txt` corpora can dominate CPU and memory.

Each run writes:

- `session.json`: benchmark configuration
- `targets.jsonl`: exact target list
- `results.jsonl`: append-only per-target observations
- `logs/*.stdout`, `logs/*.stderr`, `logs/*.time`: raw command diagnostics
- `summary.md`: aggregate timing, status, RSS, disk, page, and chunk metrics

Context7 target discovery starts from `https://context7.com/`, follows the homepage rankings link to `/api/rankings`, then fills the list with public `/api/v2/libs/search` results. Targets are indexed through their Context7 `llms.txt` URLs when available.
