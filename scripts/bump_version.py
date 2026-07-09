#!/usr/bin/env python3
"""Bump the ctx crate version in Cargo.toml and Cargo.lock."""

from __future__ import annotations

import argparse
import re
from pathlib import Path


SEMVER_RE = re.compile(
    r"^(0|[1-9]\d*)\."
    r"(0|[1-9]\d*)\."
    r"(0|[1-9]\d*)"
    r"(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$"
)


def main() -> None:
    parser = argparse.ArgumentParser(description="Bump the ctx package version.")
    parser.add_argument("version", help="New semver version, for example 0.2.0")
    args = parser.parse_args()

    if SEMVER_RE.fullmatch(args.version) is None:
        raise SystemExit(f"invalid semver version: {args.version}")

    repo = Path(__file__).resolve().parents[1]
    bump_cargo_toml(repo / "Cargo.toml", args.version)
    bump_cargo_lock(repo / "Cargo.lock", args.version)
    print(f"ctx version bumped to {args.version}")


def bump_cargo_toml(path: Path, version: str) -> None:
    lines = path.read_text(encoding="utf-8").splitlines(keepends=True)
    in_package = False
    changed = False
    for index, line in enumerate(lines):
        stripped = line.strip()
        if stripped == "[package]":
            in_package = True
            continue
        if in_package and stripped.startswith("[") and stripped.endswith("]"):
            break
        if in_package and stripped.startswith("version = "):
            lines[index] = re.sub(
                r'version = "[^"]+"',
                f'version = "{version}"',
                line,
                count=1,
            )
            changed = True
            break
    if not changed:
        raise SystemExit("Cargo.toml [package] version not found")
    path.write_text("".join(lines), encoding="utf-8")


def bump_cargo_lock(path: Path, version: str) -> None:
    lines = path.read_text(encoding="utf-8").splitlines(keepends=True)
    in_package = False
    package_is_ctx = False
    changed = False
    for index, line in enumerate(lines):
        stripped = line.strip()
        if stripped == "[[package]]":
            in_package = True
            package_is_ctx = False
            continue
        if in_package and stripped.startswith("name = "):
            package_is_ctx = stripped == 'name = "ctx"'
            continue
        if in_package and package_is_ctx and stripped.startswith("version = "):
            lines[index] = re.sub(
                r'version = "[^"]+"',
                f'version = "{version}"',
                line,
                count=1,
            )
            changed = True
            break
    if not changed:
        raise SystemExit("Cargo.lock package ctx version not found")
    path.write_text("".join(lines), encoding="utf-8")


if __name__ == "__main__":
    main()
