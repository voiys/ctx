#!/usr/bin/env python3
"""Stress-test ctx docs crawling/indexing with append-only diagnostics.

The harness is intentionally dependency-free. It borrows the useful bits from
autoresearch-style loops: repeatable inputs, JSONL observations, isolated state,
and markdown summaries that survive context loss.
"""

from __future__ import annotations

import argparse
import datetime as dt
import html.parser
import json
import os
import pathlib
import random
import signal
import shutil
import sqlite3
import statistics
import subprocess
import sys
import time
import urllib.parse
import urllib.request
from collections import Counter
from typing import Any


USER_AGENT = "ctx-docs-stress/0.1 (+https://github.com/voiys/ctx; local benchmark)"
CONTEXT7 = "https://context7.com"
DEFAULT_QUERIES = [
    "react",
    "next.js",
    "typescript",
    "tailwind",
    "supabase",
    "postgres",
    "rust",
    "python",
    "fastapi",
    "django",
    "go",
    "kubernetes",
    "docker",
    "terraform",
    "aws",
    "cloudflare",
    "vercel",
    "openai",
    "anthropic",
    "langchain",
    "llamaindex",
    "ai sdk",
    "vite",
    "vue",
    "svelte",
    "angular",
    "solid",
    "astro",
    "remix",
    "hono",
    "bun",
    "deno",
    "node",
    "prisma",
    "drizzle",
    "mongodb",
    "redis",
    "stripe",
    "clerk",
    "auth",
    "zod",
    "tanstack",
    "graphql",
    "trpc",
    "expo",
    "react native",
    "electron",
    "tauri",
    "playwright",
    "storybook",
]


class LinkParser(html.parser.HTMLParser):
    def __init__(self) -> None:
        super().__init__()
        self.links: list[str] = []

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        if tag != "a":
            return
        for name, value in attrs:
            if name == "href" and value:
                self.links.append(value)


def utc_stamp() -> str:
    return dt.datetime.now(dt.UTC).strftime("%Y%m%dT%H%M%SZ")


def fetch_text(url: str, timeout: int = 30) -> str:
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return response.read().decode("utf-8", errors="replace")


def fetch_json(url: str, timeout: int = 30) -> dict[str, Any]:
    return json.loads(fetch_text(url, timeout))


def homepage_links() -> list[str]:
    parser = LinkParser()
    parser.feed(fetch_text(CONTEXT7))
    links = []
    for href in parser.links:
        links.append(urllib.parse.urljoin(CONTEXT7, href))
    return sorted(set(links))


def library_target(library_id: str, source: str, rank: int | None = None) -> dict[str, Any]:
    clean = library_id.strip()
    return {
        "library_id": clean,
        "url": f"{CONTEXT7}{clean}",
        "llms_url": f"{CONTEXT7}{clean}/llms.txt",
        "label": "ctxstress-" + clean.strip("/").replace("/", "-").replace(".", "-").lower(),
        "source": source,
        "rank": rank,
    }


def discover_targets(limit: int) -> list[dict[str, Any]]:
    links = homepage_links()
    targets: dict[str, dict[str, Any]] = {}

    if any(urllib.parse.urlparse(link).path == "/rankings" for link in links):
        rankings = fetch_json(f"{CONTEXT7}/api/rankings")
        for item in rankings.get("data", {}).get("libraries", []):
            library_id = item.get("libraryId")
            if isinstance(library_id, str) and library_id.startswith("/"):
                target = library_target(library_id, "context7_rankings", item.get("rank"))
                target["market_share"] = item.get("marketShare")
                targets.setdefault(target["url"], target)

    for query in DEFAULT_QUERIES:
        if len(targets) >= limit:
            break
        encoded = urllib.parse.urlencode({"libraryName": query, "query": f"{query} docs"})
        try:
            payload = fetch_json(f"{CONTEXT7}/api/v2/libs/search?{encoded}")
        except Exception as error:  # noqa: BLE001 - discovery should keep moving.
            sys.stderr.write(f"search failed for {query}: {error}\n")
            continue
        for index, item in enumerate(payload.get("results", [])):
            library_id = item.get("id")
            if not isinstance(library_id, str) or not library_id.startswith("/"):
                continue
            target = library_target(library_id, f"context7_search:{query}", None)
            target.update(
                {
                    "title": item.get("title"),
                    "trust_score": item.get("trustScore"),
                    "stars": item.get("stars"),
                    "benchmark_score": item.get("benchmarkScore"),
                    "search_index": index,
                }
            )
            targets.setdefault(target["url"], target)
            if len(targets) >= limit:
                break

    return list(targets.values())[:limit]


def write_jsonl(path: pathlib.Path, rows: list[dict[str, Any]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as handle:
        for row in rows:
            handle.write(json.dumps(row, sort_keys=True) + "\n")


def append_jsonl(path: pathlib.Path, row: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(row, sort_keys=True) + "\n")


def read_jsonl(path: pathlib.Path) -> list[dict[str, Any]]:
    rows = []
    with path.open(encoding="utf-8") as handle:
        for line in handle:
            if line.strip():
                rows.append(json.loads(line))
    return rows


def dir_size(path: pathlib.Path) -> int:
    total = 0
    if not path.exists():
        return total
    for item in path.rglob("*"):
        if item.is_file():
            total += item.stat().st_size
    return total


def db_stats(ctx_home: pathlib.Path, label: str) -> dict[str, Any]:
    db_path = ctx_home / "ctx.db"
    if not db_path.exists():
        return {}
    with sqlite3.connect(db_path) as conn:
        conn.row_factory = sqlite3.Row
        stats: dict[str, Any] = {}
        for table in ["resources", "snapshots", "chunks"]:
            stats[f"{table}_total"] = conn.execute(f"SELECT COUNT(*) FROM {table}").fetchone()[0]
        resource = conn.execute(
            "SELECT id, current FROM resources WHERE label = ?", (label,)
        ).fetchone()
        if resource:
            stats["resource_id"] = resource["id"]
            stats["snapshot_id"] = resource["current"]
            snapshot = conn.execute(
                "SELECT page_count FROM snapshots WHERE resource_id = ? AND snapshot_id = ?",
                (resource["id"], resource["current"]),
            ).fetchone()
            chunks = conn.execute(
                "SELECT COUNT(*), COUNT(embedding) FROM chunks WHERE resource_id = ? AND snapshot_id = ?",
                (resource["id"], resource["current"]),
            ).fetchone()
            stats["page_count"] = snapshot[0] if snapshot else 0
            stats["chunk_count"] = chunks[0] if chunks else 0
            stats["embedding_count"] = chunks[1] if chunks else 0
        return stats


def time_prefix(metrics_path: pathlib.Path) -> list[str]:
    time_bin = pathlib.Path("/usr/bin/time")
    if not time_bin.exists():
        return []
    if sys.platform == "darwin":
        return [str(time_bin), "-l", "-o", str(metrics_path)]
    return [str(time_bin), "-v", "-o", str(metrics_path)]


def parse_time_metrics(path: pathlib.Path) -> dict[str, Any]:
    if not path.exists():
        return {}
    raw = path.read_text(encoding="utf-8", errors="replace")
    metrics: dict[str, Any] = {"time_raw": raw}
    for line in raw.splitlines():
        stripped = line.strip()
        if "maximum resident set size" in stripped:
            value = stripped.split(maxsplit=1)[0]
            if value.isdigit():
                metrics["max_rss_bytes"] = int(value)
        elif "Maximum resident set size" in stripped:
            value = stripped.rsplit(" ", 1)[-1]
            if value.isdigit():
                metrics["max_rss_bytes"] = int(value) * 1024
    return metrics


def classify(exit_code: int, stderr: str, timed_out: bool) -> str:
    lower = stderr.lower()
    if timed_out:
        return "timeout"
    if exit_code == 0:
        return "ok"
    if any(token in lower for token in ["429", "too many requests", "rate limit"]):
        return "rate_limited"
    if any(token in lower for token in ["403", "forbidden", "cloudflare", "captcha", "bot"]):
        return "blocked"
    return "failed"


def run_target(
    target: dict[str, Any],
    index: int,
    args: argparse.Namespace,
    run_dir: pathlib.Path,
    ctx_home: pathlib.Path,
    results_path: pathlib.Path,
) -> dict[str, Any]:
    stdout_path = run_dir / "logs" / f"{index:03d}-{target['label']}.stdout"
    stderr_path = run_dir / "logs" / f"{index:03d}-{target['label']}.stderr"
    time_path = run_dir / "logs" / f"{index:03d}-{target['label']}.time"
    stdout_path.parent.mkdir(parents=True, exist_ok=True)

    command = [
        str(args.ctx_bin),
        "add",
        target["llms_url"] if args.url_mode == "llms" and "llms_url" in target else target["url"],
        "--label",
        target["label"],
        "--max-pages",
        str(args.max_pages),
        "--concurrency",
        str(args.concurrency),
    ]
    env = os.environ.copy()
    env["CTX_HOME"] = str(ctx_home)
    if args.embeddings == "off":
        env["CTX_EMBEDDINGS"] = "off"
    elif "CTX_EMBEDDINGS" in env:
        del env["CTX_EMBEDDINGS"]

    started = time.perf_counter()
    timed_out = False
    with stdout_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
        try:
            process = subprocess.Popen(
                time_prefix(time_path) + command,
                stdout=stdout,
                stderr=stderr,
                env=env,
                start_new_session=True,
            )
            process.wait(timeout=args.timeout_s)
            exit_code = process.returncode
        except subprocess.TimeoutExpired:
            timed_out = True
            exit_code = 124
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
    elapsed = time.perf_counter() - started

    stderr_text = stderr_path.read_text(encoding="utf-8", errors="replace")
    row = {
        "index": index,
        "target": target,
        "status": classify(exit_code, stderr_text, timed_out),
        "exit_code": exit_code,
        "elapsed_s": round(elapsed, 3),
        "stdout_path": str(stdout_path),
        "stderr_path": str(stderr_path),
        "url_mode": args.url_mode,
        "ctx_home_bytes": dir_size(ctx_home),
    }
    row.update(parse_time_metrics(time_path))
    row.update(db_stats(ctx_home, target["label"]))
    append_jsonl(results_path, row)
    return row


def percentile(values: list[float], pct: float) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    index = min(len(ordered) - 1, max(0, round((len(ordered) - 1) * pct)))
    return ordered[index]


def summarize(run_dir: pathlib.Path) -> pathlib.Path:
    rows = read_jsonl(run_dir / "results.jsonl")
    statuses = Counter(row["status"] for row in rows)
    ok_rows = [row for row in rows if row["status"] == "ok"]
    elapsed = [row["elapsed_s"] for row in ok_rows]
    rss = [row.get("max_rss_bytes", 0) / 1024 / 1024 for row in rows if row.get("max_rss_bytes")]
    chunks = [row.get("chunk_count", 0) for row in ok_rows]
    pages = [row.get("page_count", 0) for row in ok_rows]
    ctx_home_bytes = max((row.get("ctx_home_bytes", 0) for row in rows), default=0)
    summary_path = run_dir / "summary.md"
    summary_path.write_text(
        "\n".join(
            [
                "# ctx docs stress run",
                "",
                f"- targets attempted: {len(rows)}",
                f"- statuses: {dict(statuses)}",
                f"- elapsed ok p50/p90/max: {percentile(elapsed, 0.50):.2f}s / {percentile(elapsed, 0.90):.2f}s / {(max(elapsed) if elapsed else 0):.2f}s",
                f"- max RSS observed: {(max(rss) if rss else 0):.1f} MiB",
                f"- ctx home size: {ctx_home_bytes / 1024 / 1024:.1f} MiB",
                f"- pages indexed: {sum(pages)}",
                f"- chunks indexed: {sum(chunks)}",
                f"- chunks ok p50/p90/max: {percentile(chunks, 0.50):.0f} / {percentile(chunks, 0.90):.0f} / {(max(chunks) if chunks else 0):.0f}",
                f"- elapsed mean/stdev: {(statistics.mean(elapsed) if elapsed else 0):.2f}s / {(statistics.pstdev(elapsed) if len(elapsed) > 1 else 0):.2f}s",
                "",
                "## Slowest successful targets",
                "",
                *[
                    f"- {row['elapsed_s']:.2f}s {row['target']['library_id']} ({row.get('chunk_count', 0)} chunks)"
                    for row in sorted(ok_rows, key=lambda row: row["elapsed_s"], reverse=True)[:10]
                ],
                "",
                "## Non-ok targets",
                "",
                *[
                    f"- {row['status']} {row['target']['library_id']} exit={row['exit_code']}"
                    for row in rows
                    if row["status"] != "ok"
                ][:20],
                "",
            ]
        ),
        encoding="utf-8",
    )
    return summary_path


def cmd_discover(args: argparse.Namespace) -> None:
    targets = discover_targets(args.limit)
    write_jsonl(args.out, targets)
    print(f"wrote {len(targets)} targets to {args.out}")


def cmd_run(args: argparse.Namespace) -> None:
    run_dir = args.run_dir or pathlib.Path("bench-results/docs-stress") / utc_stamp()
    run_dir.mkdir(parents=True, exist_ok=True)
    ctx_home = run_dir / "ctx-home"
    ctx_home.mkdir(parents=True, exist_ok=True)
    targets = read_jsonl(args.targets)[: args.limit]
    shutil.copy2(args.targets, run_dir / "targets.jsonl")
    (run_dir / "session.json").write_text(
        json.dumps(
            {
                "created_at": utc_stamp(),
                "ctx_bin": str(args.ctx_bin),
                "limit": args.limit,
                "max_pages": args.max_pages,
                "concurrency": args.concurrency,
                "delay_ms": args.delay_ms,
                "timeout_s": args.timeout_s,
                "embeddings": args.embeddings,
                "url_mode": args.url_mode,
            },
            indent=2,
            sort_keys=True,
        ),
        encoding="utf-8",
    )
    results_path = run_dir / "results.jsonl"
    random.seed(7)
    for index, target in enumerate(targets, start=1):
        row = run_target(target, index, args, run_dir, ctx_home, results_path)
        print(
            f"{index:03d}/{len(targets):03d} {row['status']:12s} "
            f"{row['elapsed_s']:7.2f}s {target['library_id']}"
        )
        if row.get("max_rss_bytes", 0) > args.max_rss_mb * 1024 * 1024:
            print(f"stopping: max RSS exceeded {args.max_rss_mb} MiB", file=sys.stderr)
            break
        if row.get("ctx_home_bytes", 0) > args.max_home_mb * 1024 * 1024:
            print(f"stopping: CTX_HOME exceeded {args.max_home_mb} MiB", file=sys.stderr)
            break
        time.sleep((args.delay_ms + random.randint(0, args.jitter_ms)) / 1000)
    summary_path = summarize(run_dir)
    print(f"summary: {summary_path}")


def cmd_summarize(args: argparse.Namespace) -> None:
    print(summarize(args.run_dir))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(required=True)

    discover = sub.add_parser("discover", help="create Context7-derived stress targets")
    discover.add_argument("--limit", type=int, default=100)
    discover.add_argument("--out", type=pathlib.Path, default=pathlib.Path("bench-results/context7-targets.jsonl"))
    discover.set_defaults(func=cmd_discover)

    run = sub.add_parser("run", help="run ctx add across stress targets")
    run.add_argument("--targets", type=pathlib.Path, required=True)
    run.add_argument("--limit", type=int, default=100)
    run.add_argument("--ctx-bin", type=pathlib.Path, default=pathlib.Path("target/release/ctx"))
    run.add_argument("--run-dir", type=pathlib.Path)
    run.add_argument("--max-pages", type=int, default=8)
    run.add_argument("--concurrency", type=int, default=2)
    run.add_argument("--delay-ms", type=int, default=750)
    run.add_argument("--jitter-ms", type=int, default=250)
    run.add_argument("--timeout-s", type=int, default=180)
    run.add_argument("--embeddings", choices=["on", "off"], default="off")
    run.add_argument("--url-mode", choices=["llms", "base"], default="llms")
    run.add_argument("--max-rss-mb", type=int, default=4096)
    run.add_argument("--max-home-mb", type=int, default=4096)
    run.set_defaults(func=cmd_run)

    summarize_cmd = sub.add_parser("summarize", help="rewrite summary.md for a run")
    summarize_cmd.add_argument("run_dir", type=pathlib.Path)
    summarize_cmd.set_defaults(func=cmd_summarize)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
