#!/usr/bin/env python3
"""SHIP-12 forbidden-term scanner.

Walks production crate roots declared in `config/forbidden-terms.toml`
and refuses any literal occurrence of a forbidden symbol outside the
per-term allowlist.

Exit codes:
  0  no offending matches (allowlisted matches are silent)
  1  one or more disallowed matches
  2  config or filesystem error

Invocation:
  python3 scripts/ci_forbidden_terms.py
  python3 scripts/ci_forbidden_terms.py --config <path>
  python3 scripts/ci_forbidden_terms.py --json    # machine-readable output
"""

from __future__ import annotations

import argparse
import json
import sys
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_CONFIG = REPO_ROOT / "config" / "forbidden-terms.toml"


@dataclass(frozen=True)
class Term:
    name: str
    description: str
    allowed_paths: frozenset[str]
    carried_forward: str


@dataclass
class ScanResult:
    hits: list[dict[str, object]] = field(default_factory=list)
    files_scanned: int = 0
    terms: int = 0

    @property
    def ok(self) -> bool:
        return not self.hits


def load_config(path: Path) -> tuple[list[Path], frozenset[str], list[Term]]:
    try:
        raw = path.read_bytes()
    except FileNotFoundError:
        print(f"error: config not found: {path}", file=sys.stderr)
        sys.exit(2)
    try:
        cfg = tomllib.loads(raw.decode("utf-8"))
    except (tomllib.TOMLDecodeError, UnicodeDecodeError) as exc:
        print(f"error: config parse failed: {exc}", file=sys.stderr)
        sys.exit(2)

    scan = cfg.get("scan", {})
    roots_raw = scan.get("roots")
    if not isinstance(roots_raw, list) or not roots_raw:
        print("error: [scan].roots must be a non-empty list", file=sys.stderr)
        sys.exit(2)
    roots = [REPO_ROOT / r for r in roots_raw]

    exts_raw = scan.get("extensions")
    if not isinstance(exts_raw, list) or not exts_raw:
        print("error: [scan].extensions must be a non-empty list", file=sys.stderr)
        sys.exit(2)
    exts = frozenset(str(e) for e in exts_raw)

    terms_raw = cfg.get("term")
    if not isinstance(terms_raw, list) or not terms_raw:
        print("error: at least one [[term]] is required", file=sys.stderr)
        sys.exit(2)

    terms: list[Term] = []
    seen: set[str] = set()
    for t in terms_raw:
        name = t.get("name")
        if not isinstance(name, str) or not name:
            print("error: [[term]].name must be a non-empty string", file=sys.stderr)
            sys.exit(2)
        if name in seen:
            print(f"error: duplicate [[term]].name: {name!r}", file=sys.stderr)
            sys.exit(2)
        seen.add(name)
        allowed = t.get("allowed_paths", [])
        if not isinstance(allowed, list):
            print(f"error: term {name!r}: allowed_paths must be a list", file=sys.stderr)
            sys.exit(2)
        terms.append(
            Term(
                name=name,
                description=str(t.get("description", "")),
                allowed_paths=frozenset(str(p).replace("\\", "/") for p in allowed),
                carried_forward=str(t.get("carried_forward", "")),
            )
        )
    return roots, exts, terms


def iter_files(roots: list[Path], extensions: frozenset[str]) -> list[Path]:
    files: list[Path] = []
    for root in roots:
        if not root.exists():
            continue
        for path in sorted(root.rglob("*")):
            if not path.is_file():
                continue
            if path.suffix not in extensions:
                continue
            files.append(path)
    return files


def repo_relative(path: Path) -> str:
    try:
        return path.relative_to(REPO_ROOT).as_posix()
    except ValueError:
        return path.as_posix()


def scan(roots: list[Path], extensions: frozenset[str], terms: list[Term]) -> ScanResult:
    result = ScanResult(terms=len(terms))
    files = iter_files(roots, extensions)
    result.files_scanned = len(files)
    for path in files:
        rel = repo_relative(path)
        abs_posix = path.resolve().as_posix()
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError as exc:
            print(f"error: cannot read {rel}: {exc}", file=sys.stderr)
            sys.exit(2)
        for term in terms:
            if term.name not in text:
                continue
            if rel in term.allowed_paths or abs_posix in term.allowed_paths:
                continue
            for lineno, line in enumerate(text.splitlines(), 1):
                if term.name in line:
                    result.hits.append(
                        {
                            "term": term.name,
                            "file": rel,
                            "line": lineno,
                            "carried_forward": term.carried_forward,
                            "excerpt": line.strip()[:200],
                        }
                    )
    return result


def format_human(result: ScanResult) -> str:
    if result.ok:
        return (
            f"forbidden-term scan: PASS "
            f"({result.files_scanned} files, {result.terms} terms, 0 disallowed hits)\n"
        )
    lines = [f"forbidden-term scan: FAIL ({len(result.hits)} disallowed match(es))\n"]
    for hit in result.hits:
        lines.append(
            f"  - {hit['term']} in {hit['file']}:{hit['line']}\n"
            f"      {hit['excerpt']}\n"
            f"      (allowlist closes at {hit['carried_forward'] or 'unspecified'})\n"
        )
    lines.append(
        "\nResolution: either remove the forbidden symbol or, if the use is "
        "deliberate and lives in a builder/error-message path, add the file "
        "to the term's allowed_paths in config/forbidden-terms.toml.\n"
    )
    return "".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description="SHIP-12 forbidden-term scanner")
    parser.add_argument(
        "--config",
        type=Path,
        default=DEFAULT_CONFIG,
        help="path to forbidden-terms.toml (default: config/forbidden-terms.toml)",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="emit JSON instead of human-readable output",
    )
    args = parser.parse_args()

    roots, exts, terms = load_config(args.config)
    result = scan(roots, exts, terms)

    if args.json:
        payload = {
            "ok": result.ok,
            "files_scanned": result.files_scanned,
            "terms": result.terms,
            "hits": result.hits,
        }
        print(json.dumps(payload, indent=2, sort_keys=True))
    else:
        sys.stdout.write(format_human(result))

    return 0 if result.ok else 1


if __name__ == "__main__":
    sys.exit(main())
