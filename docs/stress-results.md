# Docs Stress Results

Local stress evidence from 2026-05-24. Raw outputs live under ignored `bench-results/docs-stress/*` directories so the working tree does not accumulate benchmark artifacts.

## Context7 Target Discovery

- `scripts/docs_stress.py discover --limit 100` collected 100 targets.
- Discovery starts from `https://context7.com/`, uses the homepage `/rankings` link, reads `/api/rankings`, then fills the list from `/api/v2/libs/search`.
- Each target keeps both a Context7 library page URL and a Context7 `llms.txt` URL.

## Runs

### `20260524T095137Z`: first 10-target pilot

- mode: direct `llms.txt`, embeddings off
- status: 10 ok
- p50 / p90 / max: 1.82s / 2.50s / 2.61s
- max RSS: 22.0 MiB
- finding: explicit `llms.txt` targets were still probing extra `llms.txt` variants and indexing 4 pages per target.

### `20260524T095236Z`: tuned 10-target pilot

- mode: direct `llms.txt`, embeddings off
- status: 10 ok
- p50 / p90 / max: 0.95s / 1.46s / 1.85s
- max RSS: 20.6 MiB
- finding: avoiding extra `llms.txt` probes roughly halved the cache size and improved p50 latency.

### `20260524T095312Z`: 100-target main run

- mode: direct `llms.txt`, embeddings off
- status: 94 ok, 6 failed
- p50 / p90 / max: 0.95s / 1.37s / 1.70s for successful targets
- max RSS: 22.5 MiB
- cache size: 22.4 MiB
- indexed: 108 pages, 1662 chunks
- finding: no `blocked`, `rate_limited`, CAPTCHA, or broad 403 pattern. The 6 failures were missing `llms.txt` resources, not bot detection.

### `20260524T095834Z`: base URL fallback pilot

- mode: Context7 library page URL, embeddings off
- status: 8 ok, 1 timeout, 1 ok after 73s
- finding: base URL mode exercises fallback behavior, but it is too noisy for broad Context7 stress runs because transient `llms.txt` fetches can hang long enough to dominate the run.

### `20260524T101113Z`: embedding sample

- mode: direct `llms.txt`, embeddings on
- status: 5 ok, 1 timeout
- p50 / p90 / max: 1.32s / 6.03s / 6.03s for successful targets
- max RSS: 965.4 MiB
- indexed: 5 pages, 82 chunks, 82 embeddings
- finding: embedding memory is the resource ceiling. Healthy fetches index quickly after the model is cached, but local embedding runs should keep concurrency low and monitor RSS.

## Decisions

- Use direct `llms.txt` mode for broad Context7 corpus stress.
- Use base URL mode only as a targeted fallback test.
- Treat the first successful `llms.txt` candidate as authoritative. Do not probe broader variants or fetch the seed page after that.
- Keep per-target timeouts configurable. 15s caused false timeouts under normal network variance; 90s wastes time on missing or stalled resources.
- Keep benchmark state isolated under per-run `CTX_HOME`.
- Keep process-group cleanup in the harness so timed-out `/usr/bin/time` wrappers do not leave orphan `ctx` processes.
