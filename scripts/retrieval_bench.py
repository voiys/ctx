#!/usr/bin/env python3
"""Benchmark ctx retrieval with researched docs facts.

The harness generates a small local docs site from JSONL cases, optionally pads
it with deterministic noise pages, indexes it with ctx, then verifies that the
same questions surface the expected facts in both small and large corpora.
"""

from __future__ import annotations

import argparse
import contextlib
import datetime as dt
import html
import json
import os
import pathlib
import random
import shutil
import socket
import subprocess
import sys
import threading
import time
from collections import defaultdict
from http.server import ThreadingHTTPServer, SimpleHTTPRequestHandler
from statistics import mean
from typing import Any


NOISE_TOPICS = [
    "button color tokens spacing layout typography border radius",
    "deployment shell examples environment variables local development",
    "database migrations schema relations transactions pooling adapters",
    "testing screenshots fixtures assertions retries timeouts reports",
    "authentication sessions cookies oauth redirects middleware guards",
    "forms validation optimistic updates cache hydration pagination",
    "accessibility keyboard focus aria labels modal overlay navigation",
    "configuration plugins transforms bundling assets source maps",
]


def utc_stamp() -> str:
    return dt.datetime.now(dt.UTC).strftime("%Y%m%dT%H%M%SZ")


def load_cases(path: pathlib.Path, limit: int | None) -> list[dict[str, Any]]:
    cases: list[dict[str, Any]] = []
    with path.open(encoding="utf-8") as handle:
        for line in handle:
            if line.strip():
                cases.append(json.loads(line))
    return cases[:limit] if limit else cases


def library_slug(library: str) -> str:
    return library.lower().replace("_", "-").replace(" ", "-")


def html_page(title: str, body: str, links: list[str] | None = None) -> str:
    link_html = "".join(
        f'<li><a href="{html.escape(link)}">{html.escape(link)}</a></li>'
        for link in (links or [])
    )
    escaped = html.escape(body)
    return (
        "<!doctype html><html><body><main>"
        f"<h1>{html.escape(title)}</h1>"
        f"<ul>{link_html}</ul>"
        f"<article><pre>{escaped}</pre></article>"
        "</main></body></html>"
    )


def write_mode_corpus(
    root: pathlib.Path,
    cases: list[dict[str, Any]],
    mode: str,
    large_noise_pages: int,
) -> dict[str, int]:
    grouped: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for case in cases:
        grouped[case["library"]].append(case)

    page_counts = {}
    rng = random.Random(42)
    for library, library_cases in grouped.items():
        slug = library_slug(library)
        library_root = root / slug
        library_root.mkdir(parents=True, exist_ok=True)
        links = ["llms.txt"]

        llms_lines = [
            f"# {library} retrieval benchmark notes",
            "",
            "These notes are paraphrased from official project documentation.",
            "",
        ]
        for case in library_cases:
            doc_path = case["doc_path"]
            links.append(doc_path)
            llms_lines.append(f"- [{case['id']}]({doc_path}): {case['llms_summary']}")
            doc_file = library_root / doc_path
            doc_file.parent.mkdir(parents=True, exist_ok=True)
            doc_file.write_text(
                html_page(
                    case["id"],
                    "\n".join(
                        [
                            f"Official source: {case['source_url']}",
                            "",
                            case["doc_text"],
                        ]
                    ),
                ),
                encoding="utf-8",
            )

        if mode == "large":
            for index in range(large_noise_pages):
                topic = NOISE_TOPICS[index % len(NOISE_TOPICS)]
                noise_path = f"noise/noise-{index:03d}.html"
                links.append(noise_path)
                repeated = " ".join(rng.sample(NOISE_TOPICS, len(NOISE_TOPICS)))
                (library_root / "noise").mkdir(exist_ok=True)
                (library_root / noise_path).write_text(
                    html_page(
                        f"{library} noise {index}",
                        (
                            f"This is deterministic benchmark noise for {library}. "
                            f"It mentions {topic}. {repeated}. "
                            "It intentionally avoids the answer phrases for the benchmark cases."
                        ),
                    ),
                    encoding="utf-8",
                )

        (library_root / "llms.txt").write_text("\n".join(llms_lines), encoding="utf-8")
        (library_root / "index.html").write_text(
            html_page(
                f"{library} docs index",
                f"Index page for {library}. Follow the linked pages for benchmark facts.",
                links,
            ),
            encoding="utf-8",
        )
        page_counts[library] = len(links)
    return page_counts


class QuietHandler(SimpleHTTPRequestHandler):
    def log_message(self, format: str, *args: Any) -> None:  # noqa: A002
        return


@contextlib.contextmanager
def serve_directory(root: pathlib.Path):
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        _host, port = sock.getsockname()
    handler = lambda *args, **kwargs: QuietHandler(*args, directory=str(root), **kwargs)
    server = ThreadingHTTPServer(("127.0.0.1", port), handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{port}"
    finally:
        server.shutdown()
        thread.join(timeout=5)


def run_command(
    command: list[str],
    env: dict[str, str],
    timeout_s: int,
    cwd: pathlib.Path,
) -> tuple[int, float, str, str]:
    started = time.perf_counter()
    try:
        process = subprocess.run(
            command,
            cwd=cwd,
            env=env,
            text=True,
            capture_output=True,
            timeout=timeout_s,
            check=False,
        )
    except subprocess.TimeoutExpired as error:
        stdout = error.stdout or ""
        stderr = error.stderr or ""
        return (
            124,
            time.perf_counter() - started,
            stdout,
            f"{stderr}\ncommand timed out after {timeout_s}s",
        )
    return process.returncode, time.perf_counter() - started, process.stdout, process.stderr


def index_libraries(
    args: argparse.Namespace,
    base_url: str,
    cases: list[dict[str, Any]],
    page_counts: dict[str, int],
    ctx_home: pathlib.Path,
    run_dir: pathlib.Path,
    mode: str,
) -> list[dict[str, Any]]:
    rows = []
    env = os.environ.copy()
    env["CTX_HOME"] = str(ctx_home)
    if args.embeddings == "off":
        env["CTX_EMBEDDINGS"] = "off"
    elif "CTX_EMBEDDINGS" in env:
        del env["CTX_EMBEDDINGS"]

    libraries = sorted({case["library"] for case in cases})
    for library in libraries:
        slug = library_slug(library)
        label = next(case["label"] for case in cases if case["library"] == library)
        url = f"{base_url}/{slug}/"
        max_pages = page_counts[library] + 2
        command = [
            str(args.ctx_bin),
            "add",
            url,
            "--label",
            label,
            "--max-pages",
            str(max_pages),
            "--concurrency",
            str(args.crawl_concurrency),
        ]
        code, elapsed, stdout, stderr = run_command(command, env, args.index_timeout_s, args.cwd)
        row = {
            "phase": "index",
            "mode": mode,
            "library": library,
            "label": label,
            "url": url,
            "exit_code": code,
            "elapsed_s": round(elapsed, 3),
            "ok": code == 0,
            "stderr_tail": stderr[-1200:],
        }
        rows.append(row)
        (run_dir / f"{mode}-{slug}-add.stdout").write_text(stdout, encoding="utf-8")
        (run_dir / f"{mode}-{slug}-add.stderr").write_text(stderr, encoding="utf-8")
        if code != 0:
            raise RuntimeError(f"ctx add failed for {library}: {stderr}")
    return rows


def run_queries(
    args: argparse.Namespace,
    cases: list[dict[str, Any]],
    ctx_home: pathlib.Path,
    run_dir: pathlib.Path,
    mode: str,
) -> list[dict[str, Any]]:
    env = os.environ.copy()
    env["CTX_HOME"] = str(ctx_home)
    if args.embeddings == "off":
        env["CTX_EMBEDDINGS"] = "off"
    elif "CTX_EMBEDDINGS" in env:
        del env["CTX_EMBEDDINGS"]

    rows = []
    require_hybrid = args.require_hybrid == "on" or (
        args.require_hybrid == "auto" and args.embeddings == "on"
    )
    require_llms_prior = args.require_llms_prior == "on"
    for case in cases:
        command = [
            str(args.ctx_bin),
            "query",
            case["question"],
            "--label",
            case["label"],
            "--top-k",
            str(args.top_k),
            "--budget",
            str(args.budget),
            "--debug",
        ]
        code, elapsed, stdout, stderr = run_command(command, env, args.query_timeout_s, args.cwd)
        lower = stdout.lower()
        missing = [term for term in case["must_include"] if term.lower() not in lower]
        hybrid_seen = "rrf_hybrid" in lower
        llms_prior_seen = "source_prior: llms_txt" in lower
        failed_checks = []
        if require_hybrid and not hybrid_seen:
            failed_checks.append("hybrid retrieval was not observed")
        if require_llms_prior and not llms_prior_seen:
            failed_checks.append("llms.txt source prior was not observed")
        row = {
            "phase": "query",
            "mode": mode,
            "id": case["id"],
            "library": case["library"],
            "label": case["label"],
            "question": case["question"],
            "exit_code": code,
            "elapsed_s": round(elapsed, 3),
            "passed": code == 0 and not missing and not failed_checks,
            "missing_terms": missing,
            "failed_checks": failed_checks,
            "must_include": case["must_include"],
            "hybrid_seen": hybrid_seen,
            "llms_prior_seen": llms_prior_seen,
            "stdout_path": str(run_dir / f"{mode}-{case['id']}.stdout"),
            "stderr_tail": stderr[-1200:],
        }
        rows.append(row)
        pathlib.Path(row["stdout_path"]).write_text(stdout, encoding="utf-8")
    return rows


def append_jsonl(path: pathlib.Path, rows: list[dict[str, Any]]) -> None:
    with path.open("a", encoding="utf-8") as handle:
        for row in rows:
            handle.write(json.dumps(row, sort_keys=True) + "\n")


def summarize(run_dir: pathlib.Path, rows: list[dict[str, Any]], args: argparse.Namespace) -> None:
    query_rows = [row for row in rows if row["phase"] == "query"]
    lines = [
        "# ctx retrieval benchmark",
        "",
        f"- embeddings: {args.embeddings}",
        f"- cases: {len({row['id'] for row in query_rows})}",
        f"- modes: {', '.join(sorted({row['mode'] for row in query_rows}))}",
        f"- top_k: {args.top_k}",
        f"- budget: {args.budget}",
        f"- large_noise_pages_per_library: {args.large_noise_pages}",
        f"- require_hybrid: {args.require_hybrid}",
        f"- require_llms_prior: {args.require_llms_prior}",
        "",
    ]
    for mode in sorted({row["mode"] for row in query_rows}):
        mode_rows = [row for row in query_rows if row["mode"] == mode]
        index_rows = [row for row in rows if row["phase"] == "index" and row["mode"] == mode]
        passed = sum(1 for row in mode_rows if row["passed"])
        hybrid = sum(1 for row in mode_rows if row["hybrid_seen"])
        priors = sum(1 for row in mode_rows if row["llms_prior_seen"])
        timings = [row["elapsed_s"] for row in mode_rows]
        index_timings = [row["elapsed_s"] for row in index_rows]
        lines.extend(
            [
                f"## {mode}",
                "",
                f"- index ok: {sum(1 for row in index_rows if row['ok'])}/{len(index_rows)}",
                "- index mean/max: "
                f"{(mean(index_timings) if index_timings else 0):.3f}s / "
                f"{(max(index_timings) if index_timings else 0):.3f}s",
                f"- pass: {passed}/{len(mode_rows)}",
                f"- hybrid seen: {hybrid}/{len(mode_rows)}",
                f"- llms prior seen: {priors}/{len(mode_rows)}",
                "- query mean/max: "
                f"{(mean(timings) if timings else 0):.3f}s / "
                f"{(max(timings) if timings else 0):.3f}s",
                "",
            ]
        )
        failures = [row for row in mode_rows if not row["passed"]]
        if failures:
            lines.append("### Failures")
            lines.append("")
            for row in failures:
                lines.append(
                    f"- {row['id']}: missing {row['missing_terms']}; "
                    f"failed checks {row['failed_checks']}"
                )
            lines.append("")
    (run_dir / "summary.md").write_text("\n".join(lines), encoding="utf-8")


def run_mode(
    args: argparse.Namespace,
    cases: list[dict[str, Any]],
    run_dir: pathlib.Path,
    mode: str,
) -> list[dict[str, Any]]:
    mode_dir = run_dir / mode
    corpus_dir = mode_dir / "corpus"
    ctx_home = mode_dir / "ctx-home"
    logs_dir = mode_dir / "logs"
    corpus_dir.mkdir(parents=True, exist_ok=True)
    ctx_home.mkdir(parents=True, exist_ok=True)
    logs_dir.mkdir(parents=True, exist_ok=True)
    page_counts = write_mode_corpus(corpus_dir, cases, mode, args.large_noise_pages)
    with serve_directory(corpus_dir) as base_url:
        index_rows = index_libraries(
            args, base_url, cases, page_counts, ctx_home, logs_dir, mode
        )
        query_rows = run_queries(args, cases, ctx_home, logs_dir, mode)
    return index_rows + query_rows


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--case-file",
        type=pathlib.Path,
        default=pathlib.Path("benchmarks/retrieval_cases.jsonl"),
    )
    parser.add_argument(
        "--ctx-bin",
        type=pathlib.Path,
        default=pathlib.Path("target/release/ctx"),
    )
    parser.add_argument("--run-dir", type=pathlib.Path)
    parser.add_argument("--mode", choices=["small", "large", "both"], default="both")
    parser.add_argument("--embeddings", choices=["on", "off"], default="on")
    parser.add_argument("--limit", type=int)
    parser.add_argument("--large-noise-pages", type=int, default=64)
    parser.add_argument("--top-k", type=int, default=5)
    parser.add_argument("--budget", type=int, default=20000)
    parser.add_argument("--crawl-concurrency", type=int, default=4)
    parser.add_argument("--index-timeout-s", type=int, default=240)
    parser.add_argument("--query-timeout-s", type=int, default=60)
    parser.add_argument("--require-hybrid", choices=["auto", "on", "off"], default="auto")
    parser.add_argument("--require-llms-prior", choices=["on", "off"], default="on")
    parser.add_argument("--cwd", type=pathlib.Path, default=pathlib.Path.cwd())
    args = parser.parse_args()

    cases = load_cases(args.case_file, args.limit)
    run_dir = args.run_dir or pathlib.Path("bench-results/retrieval") / utc_stamp()
    if run_dir.exists():
        shutil.rmtree(run_dir)
    run_dir.mkdir(parents=True)
    shutil.copy2(args.case_file, run_dir / "cases.jsonl")

    modes = ["small", "large"] if args.mode == "both" else [args.mode]
    rows: list[dict[str, Any]] = []
    started = time.perf_counter()
    for mode in modes:
        rows.extend(run_mode(args, cases, run_dir, mode))
        append_jsonl(run_dir / "results.jsonl", [row for row in rows if row["mode"] == mode])
    summarize(run_dir, rows, args)
    elapsed = time.perf_counter() - started
    print(f"wrote retrieval benchmark to {run_dir}")
    print((run_dir / "summary.md").read_text(encoding="utf-8"))
    failures = [row for row in rows if row["phase"] == "query" and not row["passed"]]
    if failures:
        print(f"retrieval benchmark failed: {len(failures)} failed cases", file=sys.stderr)
        sys.exit(1)
    print(f"completed in {elapsed:.2f}s")


if __name__ == "__main__":
    main()
