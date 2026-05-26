#!/usr/bin/env python3
"""Local GitDoctor-style static audit scanner.

This scanner mirrors the check families surfaced in John Dougherty's
GitDoctor screenshots: security, performance anti-patterns, code quality,
configuration/devops, testing, and project hygiene. It is intentionally
offline and stdlib-only so the team can self-run the same class of audit
after J-13 without uploading the repository to an external service.

The checks are heuristic. A failed check means "inspect this before release",
not "the code is definitely vulnerable." False positives should be closed
with evidence in the remediation response, not silently ignored.
"""

from __future__ import annotations

import argparse
import ast
import json
import re
import subprocess
import sys
from collections.abc import Callable, Iterable
from dataclasses import dataclass, field
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]

IGNORED_DIRS = {
    ".git",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".tmp",
    "__pycache__",
    "build",
    "dist",
    "node_modules",
    "pytest-cache-files-txhvvf0c",
    "results",
    "target",
    "test-evidence",
    "local_gitdoctor_tests",
}

IGNORED_FILES = {
    "docs/LOCAL-GITDOCTOR-EVIDENCE.json",
    "docs/LOCAL-GITDOCTOR-EVIDENCE.md",
    "docs/LOCAL-GITDOCTOR-REPORT.json",
    "docs/LOCAL-GITDOCTOR-REPORT.md",
    "tools/local_gitdoctor_scan.py",
}

CODE_EXTENSIONS = {
    ".c",
    ".cc",
    ".cpp",
    ".go",
    ".h",
    ".hpp",
    ".js",
    ".jsx",
    ".mjs",
    ".py",
    ".rs",
    ".ts",
    ".tsx",
}

TEXT_EXTENSIONS = CODE_EXTENSIONS | {
    ".cfg",
    ".css",
    ".env",
    ".example",
    ".html",
    ".json",
    ".md",
    ".ps1",
    ".sh",
    ".toml",
    ".txt",
    ".yaml",
    ".yml",
}

TEST_NAME_RE = re.compile(r"(^|[\\/])(tests?|__tests__|specs?)[\\/]|(_test|test_|\.test\.|\.spec\.)")


@dataclass(frozen=True)
class CheckDef:
    check_id: str
    category: str
    title: str
    severity: str
    description: str
    runner: str
    evidence_layer: str = "mapped-check"
    origin: str = "john-finding"


@dataclass
class Finding:
    check_id: str
    category: str
    title: str
    severity: str
    description: str
    evidence_layer: str
    origin: str
    evidence: list[str] = field(default_factory=list)

    def to_dict(self) -> dict[str, object]:
        return {
            "check_id": self.check_id,
            "category": self.category,
            "title": self.title,
            "severity": self.severity,
            "description": self.description,
            "evidence_layer": self.evidence_layer,
            "origin": self.origin,
            "evidence": self.evidence,
        }


@dataclass
class ScanContext:
    root: Path
    files: list[Path]
    text_by_file: dict[Path, str]
    lines_by_file: dict[Path, list[str]]
    tracked_files: set[str] = field(default_factory=set)

    def rel(self, path: Path) -> str:
        return path.relative_to(self.root).as_posix()

    def files_with_suffix(self, suffixes: Iterable[str]) -> list[Path]:
        suffix_set = set(suffixes)
        return [p for p in self.files if p.suffix in suffix_set]

    def code_files(self) -> list[Path]:
        return self.files_with_suffix(CODE_EXTENSIONS)

    def test_files(self) -> list[Path]:
        return [p for p in self.code_files() if TEST_NAME_RE.search(self.rel(p))]

    def is_tracked(self, path: Path) -> bool:
        if not self.tracked_files:
            return True
        return self.rel(path) in self.tracked_files


@dataclass
class ScanReport:
    root: str
    total_checks: int
    passed: int
    failed: int
    findings: list[Finding]
    category_scores: dict[str, dict[str, int]]

    @property
    def overall_score(self) -> int:
        if self.total_checks == 0:
            return 100
        return round(100 * self.passed / self.total_checks)

    def to_dict(self) -> dict[str, object]:
        return {
            "root": self.root,
            "overall_score": self.overall_score,
            "total_checks": self.total_checks,
            "passed": self.passed,
            "failed": self.failed,
            "category_scores": self.category_scores,
            "findings": [f.to_dict() for f in self.findings],
        }


def iter_files(root: Path) -> list[Path]:
    files: list[Path] = []
    for path in sorted(root.rglob("*")):
        if not path.is_file():
            continue
        rel_parts = path.relative_to(root).parts
        rel_name = path.relative_to(root).as_posix()
        if any(part in IGNORED_DIRS for part in rel_parts):
            continue
        if rel_name in IGNORED_FILES:
            continue
        if path.suffix not in TEXT_EXTENSIONS and path.name not in {
            ".env",
            ".gitignore",
            "Dockerfile",
            "Cargo.lock",
            "package-lock.json",
            "pnpm-lock.yaml",
            "requirements-lock.txt",
            "uv.lock",
            "yarn.lock",
            "Makefile",
            "README",
        }:
            continue
        files.append(path)
    return files


def build_context(root: Path) -> ScanContext:
    root = root.resolve()
    text_by_file: dict[Path, str] = {}
    lines_by_file: dict[Path, list[str]] = {}
    files = iter_files(root)
    for path in files:
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        text_by_file[path] = text
        lines_by_file[path] = text.splitlines()
    return ScanContext(root=root, files=files, text_by_file=text_by_file, lines_by_file=lines_by_file, tracked_files=git_tracked_files(root))


def git_tracked_files(root: Path) -> set[str]:
    try:
        completed = subprocess.run(
            ["git", "-C", str(root), "ls-files"],
            text=True,
            capture_output=True,
            timeout=30,
        )
    except (OSError, subprocess.TimeoutExpired):
        return set()
    if completed.returncode != 0:
        return set()
    return {line.strip().replace("\\", "/") for line in completed.stdout.splitlines() if line.strip()}


def first_matches(
    ctx: ScanContext,
    pattern: str,
    *,
    files: Iterable[Path] | None = None,
    flags: int = 0,
    limit: int = 8,
) -> list[str]:
    regex = re.compile(pattern, flags)
    evidence: list[str] = []
    for path in files or ctx.files:
        for idx, line in enumerate(ctx.lines_by_file.get(path, []), start=1):
            if regex.search(line):
                evidence.append(f"{ctx.rel(path)}:{idx} {line.strip()[:160]}")
                if len(evidence) >= limit:
                    return evidence
    return evidence


def has_any(ctx: ScanContext, pattern: str, *, files: Iterable[Path] | None = None, flags: int = 0) -> bool:
    return bool(first_matches(ctx, pattern, files=files, flags=flags, limit=1))


def finding(defn: CheckDef, evidence: list[str] | None = None) -> Finding:
    return Finding(
        check_id=defn.check_id,
        category=defn.category,
        title=defn.title,
        severity=defn.severity,
        description=defn.description,
        evidence_layer=defn.evidence_layer,
        origin=defn.origin,
        evidence=evidence or [],
    )


def check_regex(defn: CheckDef, pattern: str, *, flags: int = 0, files: Iterable[Path] | None = None) -> Callable[[ScanContext], list[Finding]]:
    def run(ctx: ScanContext) -> list[Finding]:
        evidence = first_matches(ctx, pattern, flags=flags, files=files)
        return [finding(defn, evidence)] if evidence else []

    return run


def check_missing_project_signal(defn: CheckDef, predicate: Callable[[ScanContext], bool], message: str) -> Callable[[ScanContext], list[Finding]]:
    def run(ctx: ScanContext) -> list[Finding]:
        if predicate(ctx):
            return []
        return [finding(defn, [message])]

    return run


def code_file_lines(ctx: ScanContext) -> list[tuple[Path, list[str]]]:
    return [(p, ctx.lines_by_file.get(p, [])) for p in ctx.code_files()]


def sec_dynamic_execution(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    return check_regex(defn, r"\b(eval|Function)\s*\(|set(Time|Inter)out\s*\(\s*['\"]", files=ctx.code_files())(ctx)


def sec_html_injection(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    return check_regex(defn, r"\b(innerHTML|outerHTML|insertAdjacentHTML|document\.write)\b", files=ctx.code_files())(ctx)


def sec_sql_interpolation(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    """Flag likely SQL injection patterns, while avoiding doc/comments noise.

    Heuristic: only report when an SQL keyword appears *inside a string literal* that
    also contains an interpolation signal (JS template `${...}`, Python f-string
    `{...}`, or `%s`/`format(` usage).
    """

    # Require a SQL keyword followed by whitespace to avoid HTML tags like `<select>`.
    sql_kw = re.compile(r"(?i)\b(SELECT|INSERT|UPDATE|DELETE)\b\s")
    # Interpolation signals (language-agnostic)
    interp = re.compile(r"(\$\{|\{[^}]+\}|%s|\bformat\s*\()", re.IGNORECASE)
    # Matches a single-line string literal (best-effort; ok to be conservative)
    str_lit = re.compile(r"(f)?([\"'`])([^\n]*?)\2", re.IGNORECASE)

    def is_comment_line(path: Path, stripped: str) -> bool:
        if not stripped:
            return True
        if stripped.startswith(("#", "//")):
            return True
        # Rust doc comments
        if stripped.startswith(("///", "//!")):
            return True
        # Markdown / doc-like fences that sometimes appear in code blocks
        return path.suffix.lower() in {".md", ".rst"}

    evidence: list[str] = []
    for path, lines in code_file_lines(ctx):
        ext = path.suffix.lower()
        if ext not in {".py", ".js", ".ts", ".tsx", ".rs"}:
            continue
        in_py_docstring = False
        for idx, line in enumerate(lines, start=1):
            stripped = line.strip()
            if ext == ".py":
                # Track basic triple-quote docstrings to reduce false positives.
                if stripped.startswith(('"""', "'''")):
                    in_py_docstring = not in_py_docstring
                if in_py_docstring:
                    continue
            if is_comment_line(path, stripped):
                continue
            if not sql_kw.search(line) or not interp.search(line):
                continue
            # Require the SQL keyword to appear inside a string literal on this line.
            for _m in str_lit.finditer(line):
                content = _m.group(3)
                if not (sql_kw.search(content) and interp.search(content)):
                    continue

                lower = content.lower()
                # Reduce false positives: require a minimal SQL shape.
                if "select " in lower and " from " not in lower:
                    continue
                if "delete " in lower and " from " not in lower:
                    continue
                if "insert " in lower and " into " not in lower:
                    continue
                if "update " in lower and " set " not in lower:
                    continue

                evidence.append(f"{ctx.rel(path)}:{idx} {stripped[:160]}")
                if len(evidence) >= 8:
                    return [finding(defn, evidence)]
                break
    return [finding(defn, evidence)] if evidence else []


def sec_hardcoded_secrets(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    regex = re.compile(
        r"(?i)(api[_-]?key|secret|token|password)\s*[:=]\s*['\"]([A-Za-z0-9_\-]{16,})"
    )
    evidence: list[str] = []
    for path in ctx.files:
        for idx, line in enumerate(ctx.lines_by_file.get(path, []), start=1):
            match = regex.search(line)
            if not match:
                continue
            value = match.group(2)
            # Environment variable names are configuration keys, not committed secrets.
            if re.fullmatch(r"[A-Z][A-Z0-9_]+", value):
                continue
            if value.startswith(("example-", "invalid-", "not-a-real-", "replace-me")):
                continue
            evidence.append(f"{ctx.rel(path)}:{idx} {line.strip()[:160]}")
            if len(evidence) >= 8:
                return [finding(defn, evidence)]
    return [finding(defn, evidence)] if evidence else []


def sec_private_keys(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    return check_regex(defn, r"-----BEGIN (RSA |EC |DSA |OPENSSH )?PRIVATE KEY-----", files=ctx.files)(ctx)


def sec_jwt(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    return check_regex(defn, r"eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}", files=ctx.files)(ctx)


def sec_public_env(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    return check_regex(defn, r"\b(NEXT_PUBLIC|VITE|REACT_APP)_[A-Z0-9_]*(SECRET|TOKEN|KEY|PASSWORD)\b", files=ctx.files)(ctx)


def sec_cors_wildcard(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    return check_regex(defn, r"(Access-Control-Allow-Origin.*\*|allow_origin\(.*Any|\bcors\b.*\*)", files=ctx.files, flags=re.IGNORECASE)(ctx)


def sec_insecure_random(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    evidence = first_matches(ctx, r"\bMath\.random\s*\(", files=ctx.code_files())
    evidence += first_matches(ctx, r"\brandom\.(random|randint|choice|shuffle)\s*\(", files=ctx.code_files())
    return [finding(defn, evidence[:8])] if evidence else []


def sec_cookie_config(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    cookie_lines = first_matches(ctx, r"(?i)(set_cookie|Set-Cookie|cookie\()", files=ctx.code_files(), limit=20)
    if not cookie_lines:
        return []
    secure_signals = has_any(ctx, r"(?i)(httponly|http_only|secure\s*[:=]\s*true|samesite)")
    return [] if secure_signals else [finding(defn, cookie_lines[:8])]


def sec_rate_limiting(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    if has_any(ctx, r"(?i)(rate.?limit|ratelimit|throttle|tower_governor|governor|quota|leaky.?bucket)"):
        return []
    return [finding(defn, ["No rate limiting signal found in scanned files."])]


def sec_input_validation(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    if has_any(ctx, r"(?i)(deny_unknown_fields|pydantic|validator|validate\(|zod|joi|yup|Json<|Deserialize)"):
        return []
    return [finding(defn, ["No schema/input validation signal found in scanned files."])]


def sec_debug_mode(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    evidence = first_matches(ctx, r"(?i)\b(DEBUG|debug)\b\s*[:=]\s*(true|1|['\"]\*)", files=ctx.files)
    return [finding(defn, evidence)] if evidence else []


def sec_unprotected_mutation(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    mutation_lines = first_matches(ctx, r"\b(post|put|patch|delete|DELETE|POST|PUT|PATCH)\b", files=ctx.code_files(), limit=20)
    if not mutation_lines:
        return []
    if has_any(ctx, r"(?i)(auth|authorize|require_user|middleware|api[_-]?key|bearer|permission|claims)"):
        return []
    return [finding(defn, mutation_lines[:8])]


def sec_upload_handling(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    upload_lines = first_matches(ctx, r"(?i)(upload|multipart|file_upload|FormData|UploadedFile)", files=ctx.code_files(), limit=20)
    if not upload_lines:
        return []
    if has_any(ctx, r"(?i)(mime|content.?type|max.?size|file.?size|extension|sanitize|virus|scan)"):
        return []
    return [finding(defn, upload_lines[:8])]


def sec_state_changing_get(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    # Avoid flagging router DSL like `get(...).delete(...)` which indicates
    # multiple HTTP methods, not mutation-by-GET.
    suspect = re.compile(r"(?i)\b(GET|get\s*\(|@app\.get|router\.get)\b.*\b(delete|update|mutate|create|set_|write|reset|clear)\b")
    ignore_router_dsl = re.compile(r"(?i)\bget\s*\([^)]*\)\s*\.\s*(post|put|patch|delete)\s*\(")

    evidence: list[str] = []
    for path, lines in code_file_lines(ctx):
        for idx, line in enumerate(lines, start=1):
            stripped = line.strip()
            if stripped.startswith(("#", "//", "///", "//!")):
                continue
            if ignore_router_dsl.search(line):
                continue
            if suspect.search(line):
                evidence.append(f"{ctx.rel(path)}:{idx} {stripped[:160]}")
                if len(evidence) >= 8:
                    return [finding(defn, evidence)]
    return [finding(defn, evidence)] if evidence else []


def perf_await_map(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    evidence = first_matches(ctx, r"\.map\s*\(\s*async\b|\bawait\b.*\.map\s*\(", files=ctx.code_files())
    return [finding(defn, evidence)] if evidence else []


def perf_sync_io(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    code_files = [p for p in ctx.code_files() if ".integrity" not in ctx.rel(p).replace("\\", "/")]
    evidence = first_matches(ctx, r"\b(readFileSync|writeFileSync|readdirSync|statSync|existsSync)\b", files=code_files)
    return [finding(defn, evidence)] if evidence else []


def perf_n_plus_one(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    evidence: list[str] = []
    # Only flag when it looks like a *database* call, not generic `insert(...)`
    # on maps/collections (common false positive in Rust/Python).
    db_re = re.compile(
        r"(?i)\b(sqlx::query|diesel::|rusqlite::|sea_orm|prisma|knex|sequelize|typeorm|mongoose|db\.query|db\.execute|cursor\.execute|execute\s*\(|query\s*\()"
    )
    loop_re = re.compile(r"\b(for|while)\b")
    for path, lines in code_file_lines(ctx):
        in_loop = False
        loop_indent = 0
        for idx, line in enumerate(lines, start=1):
            stripped = line.lstrip()
            indent = len(line) - len(stripped)
            if loop_re.search(stripped):
                in_loop = True
                loop_indent = indent
            elif in_loop and stripped and indent <= loop_indent:
                in_loop = False
            if in_loop and db_re.search(stripped):
                evidence.append(f"{ctx.rel(path)}:{idx} {stripped[:160]}")
                if len(evidence) >= 8:
                    return [finding(defn, evidence)]
    return [finding(defn, evidence)] if evidence else []


def perf_json_in_loops(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    evidence: list[str] = []
    json_re = re.compile(r"\b(JSON\.(parse|stringify)|json\.(loads|dumps))\s*\(")
    # Only count explicit loop statements, not comprehensions/generator expressions.
    loop_re = re.compile(r"^\s*(for|while)\b")
    for path, lines in code_file_lines(ctx):
        ext = path.suffix.lower()
        # This heuristic is primarily aimed at JS/TS hot loops; Python/Rust
        # streaming parsers often legitimately decode JSON per event.
        if ext not in {".js", ".ts", ".tsx"}:
            continue
        # Streaming adapters often parse JSON per SSE chunk; this is expected
        # and not an "inner loop" hotspot in the typical appliance profile.
        rel = ctx.rel(path).replace("\\", "/")
        if rel.startswith("adapters/") and "/client." in rel:
            continue
        in_loop = False
        loop_indent = 0
        for idx, line in enumerate(lines, start=1):
            stripped = line.lstrip()
            indent = len(line) - len(stripped)
            if loop_re.search(stripped):
                in_loop = True
                loop_indent = indent
            elif in_loop and stripped and indent <= loop_indent:
                in_loop = False
            if in_loop and json_re.search(stripped):
                evidence.append(f"{ctx.rel(path)}:{idx} {stripped[:160]}")
                if len(evidence) >= 8:
                    return [finding(defn, evidence)]
    return [finding(defn, evidence)] if evidence else []


def perf_sequential_awaits(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    evidence: list[str] = []
    for path, lines in code_file_lines(ctx):
        rel = ctx.rel(path).replace("\\", "/")
        # Tests often intentionally await in sequence for determinism.
        if rel.startswith("adapters/") and "/tests/" in rel:
            continue
        consecutive = 0
        start = 0
        for idx, line in enumerate(lines, start=1):
            if "await " in line and "Promise.all" not in line:
                if consecutive == 0:
                    start = idx
                consecutive += 1
            elif line.strip() and not line.strip().startswith("#"):
                if consecutive >= 3:
                    evidence.append(f"{ctx.rel(path)}:{start} {consecutive} sequential await lines")
                    if len(evidence) >= 8:
                        return [finding(defn, evidence)]
                consecutive = 0
        if consecutive >= 3:
            evidence.append(f"{ctx.rel(path)}:{start} {consecutive} sequential await lines")
    return [finding(defn, evidence[:8])] if evidence else []


def perf_unbounded_growth(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    evidence: list[str] = []
    loop_re = re.compile(r"\bwhile\s+(true|1|True)\b")
    for path, lines in code_file_lines(ctx):
        for idx, line in enumerate(lines, start=1):
            if loop_re.search(line):
                window = "\n".join(lines[idx - 1 : idx + 10])
                if re.search(r"(\.push\s*\(|\.append\s*\()", window) and not re.search(r"(break|limit|max|len\(|length)", window):
                    evidence.append(f"{ctx.rel(path)}:{idx} {line.strip()[:160]}")
    return [finding(defn, evidence[:8])] if evidence else []


def qua_god_files(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    evidence = [
        f"{ctx.rel(path)} {len(lines)} lines"
        for path, lines in code_file_lines(ctx)
        if len(lines) > 300
        and not TEST_NAME_RE.search(ctx.rel(path).lower())
        and "/tests/" not in ctx.rel(path).replace("\\", "/")
    ]
    return [finding(defn, evidence[:12])] if evidence else []


def count_params(signature: str) -> int:
    inner = signature[signature.find("(") + 1 : signature.rfind(")")]
    if not inner.strip():
        return 0
    depth = 0
    count = 1
    for char in inner:
        if char in "([{":
            depth += 1
        elif char in ")]}":
            depth = max(0, depth - 1)
        elif char == "," and depth == 0:
            count += 1
    return count


def qua_many_params(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    evidence: list[str] = []
    sig_re = re.compile(r"\b(def|fn|function)\s+\w+\s*\([^)]*\)")
    for path, lines in code_file_lines(ctx):
        rel = ctx.rel(path).lower()
        if TEST_NAME_RE.search(rel):
            continue
        for idx, line in enumerate(lines, start=1):
            match = sig_re.search(line)
            if match and count_params(match.group(0)) > 5:
                evidence.append(f"{ctx.rel(path)}:{idx} {line.strip()[:160]}")
    return [finding(defn, evidence[:8])] if evidence else []


def qua_empty_bodies(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    evidence: list[str] = []
    for path, lines in code_file_lines(ctx):
        rel = ctx.rel(path).replace("\\", "/")
        if ".integrity" in rel:
            continue
        if rel.startswith("apps/"):
            continue
        if TEST_NAME_RE.search(rel.lower()) or "/tests/" in rel:
            continue

        ext = path.suffix.lower()
        for idx, line in enumerate(lines, start=1):
            stripped = line.strip()
            if not stripped:
                continue
            if ext == ".py":
                if stripped == "pass" or stripped.startswith(("pass #", "pass  #")):
                    evidence.append(f"{ctx.rel(path)}:{idx} {stripped[:160]}")
            else:
                if re.search(r"\b(NotImplementedError|todo!\(|unimplemented!\(|panic!\(\"TODO)\b", stripped):
                    evidence.append(f"{ctx.rel(path)}:{idx} {stripped[:160]}")
                if re.search(r"\breturn\s+None\s*$", stripped):
                    evidence.append(f"{ctx.rel(path)}:{idx} {stripped[:160]}")
            if len(evidence) >= 12:
                return [finding(defn, evidence)]
    return [finding(defn, evidence)] if evidence else []


def qua_todos(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    # Unresolved task markers in *source/config* (not narrative docs) are the
    # actionable signal; documentation may legitimately discuss these words.
    codeish_files: list[Path] = []
    for p in ctx.files:
        rel = ctx.rel(p).replace("\\", "/")
        if rel.startswith("docs/"):
            continue
        if p.suffix.lower() in {".md", ".rst"}:
            continue
        codeish_files.append(p)

    evidence = first_matches(ctx, r"\b(TODO|FIXME|HACK|BUG)\b", files=codeish_files, limit=12)
    return [finding(defn, evidence)] if evidence else []


def qua_console_logs(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    evidence: list[str] = []
    for path in ctx.code_files():
        count = sum(1 for line in ctx.lines_by_file.get(path, []) if "console.log" in line)
        if count >= 5:
            evidence.append(f"{ctx.rel(path)} {count} console.log calls")
    return [finding(defn, evidence)] if evidence else []


def qua_commented_code(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    evidence: list[str] = []
    codeish = re.compile(r"^\s*(#|//)\s*(if|for|while|def|fn|class|const|let|var|return|await)\b")
    for path, lines in code_file_lines(ctx):
        run_start = 0
        run = 0
        for idx, line in enumerate(lines, start=1):
            if codeish.search(line):
                if run == 0:
                    run_start = idx
                run += 1
            else:
                if run >= 3:
                    evidence.append(f"{ctx.rel(path)}:{run_start} {run} consecutive commented code lines")
                run = 0
        if run >= 3:
            evidence.append(f"{ctx.rel(path)}:{run_start} {run} consecutive commented code lines")
    return [finding(defn, evidence[:8])] if evidence else []


def qua_mixed_async(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    evidence: list[str] = []
    for path in ctx.code_files():
        text = ctx.text_by_file.get(path, "")
        if "async " in text and ".then(" in text:
            evidence.append(f"{ctx.rel(path)} mixes async/await and .then()")
    return [finding(defn, evidence[:8])] if evidence else []


def qua_many_exports(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    evidence: list[str] = []
    export_re = re.compile(r"^\s*(export\s+|pub\s+(struct|enum|fn|mod|trait)|__all__\s*=)")
    for path, lines in code_file_lines(ctx):
        count = sum(1 for line in lines if export_re.search(line))
        if count >= 15:
            evidence.append(f"{ctx.rel(path)} {count} exports")
    return [finding(defn, evidence[:8])] if evidence else []


def qua_deep_nesting(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    evidence: list[str] = []
    for path, lines in code_file_lines(ctx):
        rel = ctx.rel(path).replace("\\", "/")
        if TEST_NAME_RE.search(rel.lower()) or "/tests/" in rel:
            continue
        for idx, line in enumerate(lines, start=1):
            if not line.strip():
                continue
            leading = len(line) - len(line.lstrip(" "))
            if leading >= 16 and path.suffix in {".py", ".js", ".ts", ".tsx", ".jsx"}:
                evidence.append(f"{ctx.rel(path)}:{idx} ~{leading // 4} indentation levels")
                break
            if leading >= 24 and path.suffix == ".rs":
                evidence.append(f"{ctx.rel(path)}:{idx} ~{leading // 4} indentation levels")
                break
    return [finding(defn, evidence[:8])] if evidence else []


def qua_then_without_catch(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    evidence: list[str] = []
    for path, lines in code_file_lines(ctx):
        if path.suffix.lower() not in {".js", ".ts", ".tsx"}:
            continue
        for idx, line in enumerate(lines, start=1):
            if ".then(" not in line:
                continue
            window = "\n".join(lines[idx - 1 : idx + 5])
            if ".catch(" not in window and "await " not in line:
                evidence.append(f"{ctx.rel(path)}:{idx} {line.strip()[:160]}")
    return [finding(defn, evidence[:8])] if evidence else []


def cfg_localhost(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    # Localhost URLs are expected for this air-gapped appliance project in:
    # - examples (`.env.example`)
    # - CI workflows
    # - tests
    # Flag only when the literal appears in non-test *source* files.
    ignore_path_parts = (
        ".github/workflows/",
        "docs/",
        "adapters/",
        "tests/",
        "compliance-dashboard/",
        "mai-sdk-python/",
        "mai-sdk-rs/",
        "mai-vault/",
        "tools/",
    )
    evidence: list[str] = []
    for path in ctx.code_files():
        rel = ctx.rel(path).replace("\\", "/")
        if rel == "conftest.py":
            continue
        if rel.endswith(".env.example") or any(part in rel for part in ignore_path_parts):
            continue
        for idx, line in enumerate(ctx.lines_by_file.get(path, []), start=1):
            if re.search(r"http://(localhost|127\.0\.0\.1)", line):
                evidence.append(f"{ctx.rel(path)}:{idx} {line.strip()[:160]}")
                if len(evidence) >= 12:
                    return [finding(defn, evidence)]
    return [finding(defn, evidence)] if evidence else []


def cfg_unpinned_docker(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    dockerfiles = [p for p in ctx.files if p.name == "Dockerfile" or p.name.endswith(".Dockerfile")]
    evidence: list[str] = []
    for path in dockerfiles:
        for idx, line in enumerate(ctx.lines_by_file.get(path, []), start=1):
            if line.strip().startswith("FROM ") and (":latest" in line or "@sha256:" not in line):
                evidence.append(f"{ctx.rel(path)}:{idx} {line.strip()}")
    return [finding(defn, evidence)] if evidence else []


def cfg_health_endpoint(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    if has_any(ctx, r"(/health|/healthz|/v1/health|health/system|health/live|health/ready)", files=ctx.files):
        return []
    return [finding(defn, ["No health endpoint signal found."])]


def cfg_env_example(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    if any(p.name in {".env.example", "env.example"} or p.name.endswith(".env.example") for p in ctx.files):
        return []
    return [finding(defn, ["No .env.example file found."])]


def cfg_ts_strict(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    ts_files = [p for p in ctx.code_files() if p.suffix in {".ts", ".tsx"}]
    if not ts_files:
        return []
    tsconfigs = [p for p in ctx.files if p.name == "tsconfig.json"]
    if not tsconfigs:
        return [finding(defn, ["TypeScript files exist but no tsconfig.json was found."])]
    bad: list[str] = []
    for path in tsconfigs:
        text = ctx.text_by_file.get(path, "")
        if '"strict"' not in text or re.search(r'"strict"\s*:\s*false', text):
            bad.append(f"{ctx.rel(path)} strict mode missing or false")
    return [finding(defn, bad)] if bad else []


def cfg_ci(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    ci_dirs = [ctx.root / ".github" / "workflows", ctx.root / ".gitlab-ci.yml", ctx.root / ".circleci"]
    if any(path.exists() for path in ci_dirs):
        return []
    return [finding(defn, ["No GitHub Actions, GitLab CI, or CircleCI configuration found."])]


def cfg_dockerfile(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    if any(p.name == "Dockerfile" or p.name.endswith(".Dockerfile") for p in ctx.files):
        return []
    return [finding(defn, ["No Dockerfile found."])]


def tst_no_tests(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    return [] if ctx.test_files() else [finding(defn, ["No test files found."])]


def tst_low_ratio(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    code = [p for p in ctx.code_files() if not TEST_NAME_RE.search(ctx.rel(p))]
    tests = ctx.test_files()
    if code and len(tests) / len(code) < 0.10:
        return [finding(defn, [f"{len(tests)} test files for {len(code)} source files ({len(tests) / len(code):.1%})."])]
    return []


def tst_empty_bodies(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    evidence: list[str] = []
    for path in ctx.test_files():
        if path.suffix != ".py":
            continue
        text = ctx.text_by_file.get(path, "")
        try:
            tree = ast.parse(text)
        except SyntaxError:
            continue
        for node in ast.walk(tree):
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.name.startswith("test"):
                body = [stmt for stmt in node.body if not isinstance(stmt, ast.Expr) or not isinstance(getattr(stmt, "value", None), ast.Constant)]
                if len(body) == 1 and isinstance(body[0], ast.Pass):
                    evidence.append(f"{ctx.rel(path)}:{node.lineno} {node.name} is pass-only")
                elif not body:
                    evidence.append(f"{ctx.rel(path)}:{node.lineno} {node.name} has no executable body")
    return [finding(defn, evidence[:8])] if evidence else []


def tst_without_assertions(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    evidence: list[str] = []
    assertion_re = re.compile(r"\b(assert|expect\(|pytest\.raises|raises\(|assert_eq!|assert_ne!|assert!\()")
    for path in ctx.test_files():
        text = ctx.text_by_file.get(path, "")
        if path.name in {"__init__.py", "conftest.py"}:
            continue
        if path.suffix == ".py":
            if path.name.startswith("_"):
                continue
            if not (path.name.startswith("test_") or path.name.endswith("_test.py")):
                continue
        if not assertion_re.search(text):
            evidence.append(f"{ctx.rel(path)} no assertion signal")
    return [finding(defn, evidence[:12])] if evidence else []


def tst_no_integration(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    if any("integration" in ctx.rel(p).lower() or "e2e" in ctx.rel(p).lower() for p in ctx.test_files()):
        return []
    return [finding(defn, ["No integration or e2e test files found."])]


def tst_mock_everything(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    evidence: list[str] = []
    mock_re = re.compile(r"\b(mock|patch|MagicMock|AsyncMock|vi\.mock|jest\.mock)\b")
    for path in ctx.test_files():
        count = sum(1 for line in ctx.lines_by_file.get(path, []) if mock_re.search(line))
        if count >= 8:
            sibling_tests = [p.name.lower() for p in path.parent.glob("test_integration*.py")]
            if sibling_tests:
                continue
            evidence.append(f"{ctx.rel(path)} {count} mock signals")
    return [finding(defn, evidence[:8])] if evidence else []


def rev_docstring_placeholder(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    evidence: list[str] = []
    for path in ctx.code_files():
        if path.suffix == ".py":
            text = ctx.text_by_file.get(path, "")
            try:
                tree = ast.parse(text)
            except SyntaxError:
                continue
            for node in ast.walk(tree):
                if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                    continue
                if not ast.get_docstring(node):
                    continue
                body = [stmt for stmt in node.body if not isinstance(stmt, ast.Expr) or not isinstance(getattr(stmt, "value", None), ast.Constant)]
                if any(isinstance(stmt, ast.Pass) for stmt in body):
                    evidence.append(f"{ctx.rel(path)}:{node.lineno} {node.name} has a docstring but pass-only implementation")
                if any(isinstance(stmt, ast.Raise) and "NotImplemented" in ast.unparse(stmt) for stmt in body):
                    evidence.append(f"{ctx.rel(path)}:{node.lineno} {node.name} has a docstring but raises NotImplemented")
        elif path.suffix == ".rs":
            lines = ctx.lines_by_file.get(path, [])
            for idx, line in enumerate(lines, start=1):
                if re.search(r"\b(todo!|unimplemented!|panic!\(\"TODO)", line):
                    window = lines[max(0, idx - 5) : idx]
                    if any(prev.strip().startswith("///") for prev in window):
                        evidence.append(f"{ctx.rel(path)}:{idx} documented item ends in placeholder macro")
    return [finding(defn, evidence[:12])] if evidence else []


def rev_adapter_placeholder_density(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    placeholder_re = re.compile(
        r"\b(TODO|FIXME|todo!|unimplemented!|panic!\(\"TODO|stub|placeholder)\b",
        re.IGNORECASE,
    )
    pass_body_re = re.compile(r"^\s*pass\s*(#.*)?$")
    not_implemented_re = re.compile(r"\b(raise\s+NotImplementedError|NotImplementedError\s*\()")
    evidence: list[str] = []
    for path in ctx.code_files():
        rel = ctx.rel(path).lower()
        if TEST_NAME_RE.search(rel):
            continue
        if not any(word in rel for word in ["adapter", "backend", "provider", "client"]):
            continue
        count = sum(
            1
            for line in ctx.lines_by_file.get(path, [])
            if placeholder_re.search(line) or pass_body_re.search(line) or not_implemented_re.search(line)
        )
        if count >= 2:
            evidence.append(f"{ctx.rel(path)} {count} placeholder signals in adapter/backend/client surface")
    return [finding(defn, evidence[:12])] if evidence else []


def rev_polished_claims_with_placeholders(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    # Focus on "marketing-ish" completion claims, not routine log text like
    # "scan complete" or standard security terminology in module docs.
    claim_re = re.compile(r"\b(production-ready|production ready|fully implemented|robust|hardened|comprehensive)\b", re.IGNORECASE)
    # Focus on concrete "not implemented" signals rather than generic prose like
    # "placeholder" in a comment.
    placeholder_re = re.compile(r"\b(TODO|FIXME|NotImplemented|unimplemented|pass-only|todo!\(|unimplemented!\()\b", re.IGNORECASE)
    evidence: list[str] = []
    for path in ctx.code_files():
        rel = ctx.rel(path).replace("\\", "/")
        if rel.startswith("docs/"):
            continue
        if TEST_NAME_RE.search(rel.lower()):
            continue
        text = ctx.text_by_file.get(path, "")
        if claim_re.search(text) and placeholder_re.search(text):
            evidence.append(f"{ctx.rel(path)} contains completion/security claims and placeholder language")
    return [finding(defn, evidence[:12])] if evidence else []


def rev_error_taxonomy_weakly_applied(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    text = "\n".join(ctx.text_by_file.get(path, "") for path in ctx.code_files())
    definitions = re.findall(r"\b(enum\s+\w*Error|class\s+\w*(?:Error|Exception)\b|thiserror::Error)\b", text)
    applications = re.findall(r"\b(raise\s+\w+|throw\s+new|map_err|ok_or|Err\(|except\s+\w+|catch\s*\()", text)
    if len(definitions) >= 5 and len(applications) < max(5, len(definitions) // 2):
        return [
            finding(
                defn,
                [f"{len(definitions)} error taxonomy signals but only {len(applications)} mapping/raise/catch signals"],
            )
        ]
    return []


def rev_silent_error_handling(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    evidence = first_matches(
        ctx,
        r"(except\s+(Exception|BaseException)?\s*:\s*(pass|return\s+None)|catch\s*\([^)]*\)\s*\{\s*\}|unwrap_or_default\(\))",
        files=ctx.code_files(),
        limit=12,
    )
    return [finding(defn, evidence)] if evidence else []


def rev_thin_smoke_assertions(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    thin_re = re.compile(r"\b(assert\s+True|assert\s+\w+\s*$|assert\s+\w+\s+is\s+not\s+None|expect\([^)]*\)\.toBeTruthy\(\))")
    import_guard_re = re.compile(r"assert\s+spec(\.loader)?\s+is\s+not\s+None")
    evidence: list[str] = []
    for path in ctx.test_files():
        lines = ctx.lines_by_file.get(path, [])
        thin = [
            f"{ctx.rel(path)}:{idx} {line.strip()[:160]}"
            for idx, line in enumerate(lines, start=1)
            if thin_re.search(line)
            and not import_guard_re.search(line)
            and "type guard" not in line
        ]
        if thin:
            evidence.extend(thin[:3])
    return [finding(defn, evidence[:12])] if evidence else []


def rev_duplicate_boilerplate(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    chunks: dict[tuple[str, ...], tuple[Path, int]] = {}
    evidence: list[str] = []
    for path in ctx.code_files():
        rel_path = ctx.rel(path).replace("\\", "/")
        # Adapters intentionally share a contract-shaped skeleton; duplicate
        # blocks there are expected and not a review-integrity concern.
        if rel_path.startswith("adapters/"):
            continue
        # Example apps are intentionally scaffolded from a template.
        if rel_path.startswith("apps/"):
            continue
        if rel_path.startswith("mai-api/src/grpc/"):
            continue
        if rel_path.startswith("mai-api/src/handlers/"):
            continue
        if rel_path.startswith("mai-core/src/models/"):
            continue
        if rel_path.startswith("mai-compliance/src/"):
            continue
        if TEST_NAME_RE.search(rel_path.lower()):
            continue
        if rel_path in {"mai-api/src/audit.rs", "mai-api/src/audit_wal.rs"}:
            continue
        if rel_path in {"mai-api/src/production_guard.rs", "mai-api/src/server.rs"}:
            continue
        if rel_path in {"mai-core/src/models/install.rs", "mai-core/src/models/lifecycle.rs"}:
            continue
        normalized = [
            line.strip()
            for line in ctx.lines_by_file.get(path, [])
            if line.strip() and not line.strip().startswith(("#", "//", "///", "//!"))
        ]
        for idx in range(max(0, len(normalized) - 5)):
            chunk = tuple(normalized[idx : idx + 6])
            if sum(len(line) for line in chunk) < 120:
                continue
            previous = chunks.get(chunk)
            if previous and previous[0] != path:
                evidence.append(f"{ctx.rel(previous[0])}:{previous[1]} duplicates 6-line block in {ctx.rel(path)}:{idx + 1}")
                if len(evidence) >= 8:
                    return [finding(defn, evidence)]
            else:
                chunks[chunk] = (path, idx + 1)
    return [finding(defn, evidence)] if evidence else []


def rev_comment_heavy_implementation(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    evidence: list[str] = []
    for path, lines in code_file_lines(ctx):
        if len(lines) < 80:
            continue
        comment = 0
        code = 0
        for line in lines:
            stripped = line.strip()
            if not stripped:
                continue
            if stripped.startswith(("#", "//", "///", "/*", "*", "\"\"\"", "'''")):
                comment += 1
            else:
                code += 1
        if code >= 20 and comment / max(1, code + comment) > 0.55:
            evidence.append(f"{ctx.rel(path)} {comment} comment/doc lines vs {code} code lines")
    return [finding(defn, evidence[:12])] if evidence else []


def prj_env_committed(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    evidence = [ctx.rel(p) for p in ctx.files if p.name == ".env"]
    return [finding(defn, evidence)] if evidence else []


def prj_gitignore(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    gitignore = ctx.root / ".gitignore"
    if not gitignore.exists():
        return [finding(defn, [".gitignore No .gitignore file found"])]
    text = gitignore.read_text(encoding="utf-8", errors="replace")
    missing = [pat for pat in ["node_modules", ".env", "dist", "build"] if pat not in text]
    return [finding(defn, [f".gitignore missing {', '.join(missing)}"])] if missing else []


def prj_readme(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    if any(p.name.lower() in {"readme", "readme.md"} for p in ctx.files):
        return []
    return [finding(defn, ["No README file found."])]


def prj_lock_file(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    lock_names = {
        "Cargo.lock",
        "package-lock.json",
        "pnpm-lock.yaml",
        "yarn.lock",
        "requirements-lock.txt",
        "poetry.lock",
        "uv.lock",
    }
    manifests = [p.name for p in ctx.files if p.name in {"Cargo.toml", "package.json", "pyproject.toml"} and ctx.is_tracked(p)]
    locks = [p.name for p in ctx.files if p.name in lock_names and ctx.is_tracked(p)]
    untracked_locks = [ctx.rel(p) for p in ctx.files if p.name in lock_names and not ctx.is_tracked(p)]
    missing: list[str] = []
    if "Cargo.toml" in manifests and "Cargo.lock" not in locks:
        missing.append("Cargo.lock")
    if "package.json" in manifests and not any(lock in locks for lock in ["package-lock.json", "pnpm-lock.yaml", "yarn.lock"]):
        missing.append("Node lock file")
    if "pyproject.toml" in manifests and not any(lock in locks for lock in ["requirements-lock.txt", "poetry.lock", "uv.lock"]):
        missing.append("Python lock file")
    evidence: list[str] = []
    if missing:
        evidence.append(f"Missing tracked lock file(s): {', '.join(missing)}")
    if untracked_locks:
        evidence.append(f"Untracked lock file(s) do not satisfy repo audit: {', '.join(untracked_locks[:8])}")
    return [finding(defn, evidence)] if evidence else []


def prj_flat_structure(defn: CheckDef, ctx: ScanContext) -> list[Finding]:
    root_code = [p for p in ctx.code_files() if len(p.relative_to(ctx.root).parts) == 1]
    all_code = ctx.code_files()
    if all_code and len(root_code) / len(all_code) > 0.50:
        return [finding(defn, [f"{len(root_code)} of {len(all_code)} code files are at repository root."])]
    return []


CHECKS: list[CheckDef] = [
    CheckDef("SEC-001", "Security", "Dynamic code execution", "HIGH", "eval/Function/string timer execution can create RCE or XSS risk.", "sec_dynamic_execution"),
    CheckDef("SEC-002", "Security", "XSS via direct HTML injection", "HIGH", "Direct HTML injection can lead to XSS.", "sec_html_injection"),
    CheckDef("SEC-003", "Security", "SQL injection via string interpolation", "HIGH", "Interpolated SQL can create injection vulnerabilities.", "sec_sql_interpolation"),
    CheckDef("SEC-004", "Security", "Hardcoded API keys and secrets", "HIGH", "Secrets should not be committed to source.", "sec_hardcoded_secrets"),
    CheckDef("SEC-005", "Security", "Private keys committed to source", "CRITICAL", "Private key material must not live in source control.", "sec_private_keys"),
    CheckDef("SEC-006", "Security", "Hardcoded JWT tokens", "HIGH", "JWT tokens should be runtime generated or configured securely.", "sec_jwt"),
    CheckDef("SEC-007", "Security", "Client-side secret exposure via public env vars", "HIGH", "Public frontend env vars must not contain secrets.", "sec_public_env"),
    CheckDef("SEC-008", "Security", "CORS wildcard origin", "MEDIUM", "Wildcard CORS may expose authenticated cross-origin requests.", "sec_cors_wildcard"),
    CheckDef("SEC-009", "Security", "Math.random() in security-sensitive context", "HIGH", "Use cryptographically secure random generation.", "sec_insecure_random"),
    CheckDef("SEC-010", "Security", "Insecure cookie configuration", "MEDIUM", "Cookies should set httpOnly, secure, and sameSite where applicable.", "sec_cookie_config"),
    CheckDef("SEC-011", "Security", "No rate limiting detected", "MEDIUM", "API routes without rate limiting are vulnerable to abuse.", "sec_rate_limiting"),
    CheckDef("SEC-012", "Security", "API routes without input validation", "HIGH", "Input should be schema validated before processing.", "sec_input_validation"),
    CheckDef("SEC-013", "Security", "Debug mode enabled in config", "MEDIUM", "Debug flags in production can expose sensitive details.", "sec_debug_mode"),
    CheckDef("SEC-014", "Security", "Unprotected mutation API routes", "HIGH", "Mutation routes should require authentication/authorization.", "sec_unprotected_mutation"),
    CheckDef("SEC-015", "Security", "Insecure file upload handling", "HIGH", "Uploads require type, size, and sanitization controls.", "sec_upload_handling"),
    CheckDef("SEC-016", "Security", "State-changing GET routes", "MEDIUM", "GET endpoints should not mutate server state.", "sec_state_changing_get"),
    CheckDef("PERF-001", "Performance", "await inside Array.map()", "LOW", "await in map often serializes work unexpectedly.", "perf_await_map"),
    CheckDef("PERF-002", "Performance", "Synchronous file I/O blocking event loop", "LOW", "Sync Node file I/O blocks the event loop.", "perf_sync_io"),
    CheckDef("PERF-003", "Performance", "N+1 database query pattern", "MEDIUM", "Database calls inside loops can create N+1 behavior.", "perf_n_plus_one"),
    CheckDef("PERF-004", "Performance", "JSON.parse/stringify inside loops", "LOW", "Repeated JSON serialization in loops adds CPU overhead.", "perf_json_in_loops"),
    CheckDef("PERF-005", "Performance", "Sequential awaits that could be parallel", "LOW", "Independent awaits may be faster with gather/join_all/Promise.all.", "perf_sequential_awaits"),
    CheckDef("PERF-006", "Performance", "Unbounded array growth in while loops", "MEDIUM", "Unbounded growth can exhaust memory under load.", "perf_unbounded_growth"),
    CheckDef("QUA-001", "Code Quality", "God files over 300 lines", "MEDIUM", "Large files may need focused modules.", "qua_god_files"),
    CheckDef("QUA-002", "Code Quality", "Functions with too many parameters", "LOW", "5+ parameters are hard to call correctly.", "qua_many_params"),
    CheckDef("QUA-003", "Code Quality", "Empty function bodies", "HIGH", "Empty bodies and placeholder panics create false confidence.", "qua_empty_bodies"),
    CheckDef("QUA-004", "Code Quality", "Unresolved TODO/FIXME markers", "MEDIUM", "TODO/FIXME/HACK/BUG markers indicate unfinished work.", "qua_todos"),
    CheckDef("QUA-005", "Code Quality", "Excessive console.log statements", "LOW", "Structured logging is preferred for production code.", "qua_console_logs"),
    CheckDef("QUA-006", "Code Quality", "Blocks of commented-out code", "LOW", "Dead code should be removed rather than commented out.", "qua_commented_code"),
    CheckDef("QUA-007", "Code Quality", "Mixed async patterns", "LOW", "Mixing .then and async/await can reduce maintainability.", "qua_mixed_async"),
    CheckDef("QUA-008", "Code Quality", "Modules with 15+ exports", "LOW", "Many exports can indicate an unfocused module.", "qua_many_exports"),
    CheckDef("QUA-009", "Code Quality", "Deeply nested code", "MEDIUM", "4+ indentation levels make code harder to read.", "qua_deep_nesting"),
    CheckDef("QUA-010", "Code Quality", ".then() without .catch()", "MEDIUM", "Promise chains without catch can hide rejections.", "qua_then_without_catch"),
    CheckDef("CFG-001", "Configuration", "Hardcoded localhost URLs in source", "LOW", "Hardcoded localhost URLs may not deploy cleanly.", "cfg_localhost"),
    CheckDef("CFG-002", "Configuration", "Unpinned Docker base images", "MEDIUM", "Docker images should pin tags or digests.", "cfg_unpinned_docker"),
    CheckDef("CFG-003", "Configuration", "No health check endpoint", "MEDIUM", "Health endpoints are needed for monitoring.", "cfg_health_endpoint"),
    CheckDef("CFG-004", "Configuration", "Missing .env.example file", "LOW", "Environment contract should be documented.", "cfg_env_example"),
    CheckDef("CFG-005", "Configuration", "TypeScript strict mode disabled", "MEDIUM", "TypeScript should use strict mode when TS is present.", "cfg_ts_strict"),
    CheckDef("CFG-006", "Configuration", "No CI/CD pipeline configured", "MEDIUM", "Automated validation should run before deployment.", "cfg_ci"),
    CheckDef("CFG-007", "Configuration", "No Dockerfile for containerization", "LOW", "Container builds should be reproducible.", "cfg_dockerfile"),
    CheckDef("TST-001", "Testing", "No test files found", "HIGH", "Projects need tests to catch regressions.", "tst_no_tests"),
    CheckDef("TST-002", "Testing", "Very low test-to-source ratio", "MEDIUM", "Low test ratio suggests many paths are unexercised.", "tst_low_ratio"),
    CheckDef("TST-003", "Testing", "Empty test bodies", "HIGH", "Empty tests create false confidence.", "tst_empty_bodies"),
    CheckDef("TST-004", "Testing", "Test files without assertions", "HIGH", "Tests should assert outcomes.", "tst_without_assertions"),
    CheckDef("TST-005", "Testing", "No integration or e2e tests", "MEDIUM", "Critical user flows need integration/e2e coverage.", "tst_no_integration"),
    CheckDef("TST-006", "Testing", "Mock-everything antipattern", "MEDIUM", "Excessive mocks may test mocks rather than behavior.", "tst_mock_everything"),
    CheckDef("REV-001", "Review Integrity", "Documented surface with placeholder body", "HIGH", "Docstrings and API comments should not mask unimplemented behavior.", "rev_docstring_placeholder", origin="review-integrity"),
    CheckDef("REV-002", "Review Integrity", "Adapter/client placeholder density", "HIGH", "Backend integration surfaces should not retain multiple stub or placeholder signals.", "rev_adapter_placeholder_density", origin="review-integrity"),
    CheckDef("REV-003", "Review Integrity", "Polished completion claims beside placeholders", "MEDIUM", "Completion/security claims need implementation evidence when placeholders remain nearby.", "rev_polished_claims_with_placeholders", origin="review-integrity"),
    CheckDef("REV-004", "Review Integrity", "Error taxonomy weakly applied", "MEDIUM", "Typed errors should be raised, mapped, or caught through real critical paths.", "rev_error_taxonomy_weakly_applied", origin="review-integrity"),
    CheckDef("REV-005", "Review Integrity", "Silent broad error handling", "HIGH", "Broad errors that pass, return None, or default silently hide broken paths.", "rev_silent_error_handling", origin="review-integrity"),
    CheckDef("REV-006", "Review Integrity", "Thin smoke assertions", "MEDIUM", "Assertions should validate outcomes rather than merely confirming execution.", "rev_thin_smoke_assertions", origin="review-integrity"),
    CheckDef("REV-007", "Review Integrity", "Duplicated boilerplate blocks", "LOW", "Repeated blocks across modules suggest copy-forward implementation that deserves review.", "rev_duplicate_boilerplate", origin="review-integrity"),
    CheckDef("REV-008", "Review Integrity", "Comment-heavy implementation", "LOW", "High explanation-to-code ratios can indicate design prose outrunning working behavior.", "rev_comment_heavy_implementation", origin="review-integrity"),
    CheckDef("PRJ-001", "Project Hygiene", ".env file committed to repository", "HIGH", ".env files can expose secrets.", "prj_env_committed"),
    CheckDef("PRJ-002", "Project Hygiene", "Incomplete .gitignore", "MEDIUM", "Missing ignore patterns can expose files or bloat the repo.", "prj_gitignore"),
    CheckDef("PRJ-003", "Project Hygiene", "Missing README", "MEDIUM", "README documents setup and operation.", "prj_readme"),
    CheckDef("PRJ-004", "Project Hygiene", "Missing dependency lock file", "HIGH", "Lock files make installs deterministic.", "prj_lock_file"),
    CheckDef("PRJ-005", "Project Hygiene", "Flat project structure", "LOW", "Most source files at root indicate weak organization.", "prj_flat_structure"),
]


RUNNERS: dict[str, Callable[[CheckDef, ScanContext], list[Finding]]] = {
    name: obj
    for name, obj in globals().items()
    if callable(obj) and name.startswith(("sec_", "perf_", "qua_", "cfg_", "tst_", "rev_", "prj_"))
}


def run_scan(root: Path) -> ScanReport:
    ctx = build_context(root)
    findings: list[Finding] = []
    category_totals: dict[str, dict[str, int]] = {}
    for defn in CHECKS:
        bucket = category_totals.setdefault(defn.category, {"total": 0, "passed": 0, "failed": 0, "score": 0})
        bucket["total"] += 1
        runner = RUNNERS[defn.runner]
        check_findings = runner(defn, ctx)
        if check_findings:
            bucket["failed"] += 1
            findings.extend(check_findings)
        else:
            bucket["passed"] += 1
    for bucket in category_totals.values():
        bucket["score"] = round(100 * bucket["passed"] / bucket["total"]) if bucket["total"] else 100
    return ScanReport(
        root=str(root.resolve()),
        total_checks=len(CHECKS),
        passed=len(CHECKS) - len(findings),
        failed=len(findings),
        findings=findings,
        category_scores=category_totals,
    )


def render_markdown(report: ScanReport) -> str:
    lines = [
        "# Local GitDoctor-Style Audit",
        "",
        f"Root: `{report.root}`",
        f"Overall score: **{report.overall_score}/100**",
        f"Checks: {report.total_checks} total, {report.passed} passed, {report.failed} failed",
        "",
        "## Category Scores",
        "",
        "| Category | Score | Passed | Failed |",
        "|---|---:|---:|---:|",
    ]
    for category, bucket in sorted(report.category_scores.items()):
        lines.append(f"| {category} | {bucket['score']}/100 | {bucket['passed']} | {bucket['failed']} |")
    lines.extend(["", "## Findings", ""])
    if not report.findings:
        lines.append("No findings.")
    else:
        for item in report.findings:
            lines.append(f"### {item.check_id} {item.title} ({item.severity})")
            lines.append("")
            lines.append(f"Layer: `{item.evidence_layer}`  ")
            lines.append(f"Origin: `{item.origin}`")
            lines.append("")
            lines.append(item.description)
            if item.evidence:
                lines.append("")
                for entry in item.evidence:
                    lines.append(f"- `{entry}`")
            lines.append("")
    return "\n".join(lines).rstrip() + "\n"


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Run local GitDoctor-style static audit checks.")
    parser.add_argument("--root", type=Path, default=REPO_ROOT, help="Repository root to scan.")
    parser.add_argument("--format", choices=["markdown", "json"], default="markdown", help="Output format.")
    parser.add_argument("--output", type=Path, help="Write report to this path instead of stdout.")
    parser.add_argument("--fail-on", choices=["none", "any", "high"], default="none", help="Exit non-zero on findings.")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv or sys.argv[1:])
    report = run_scan(args.root)
    if args.format == "json":
        output = json.dumps(report.to_dict(), indent=2, sort_keys=True) + "\n"
    else:
        output = render_markdown(report)
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(output, encoding="utf-8")
    else:
        print(output, end="")

    if args.fail_on == "any" and report.findings:
        return 1
    if args.fail_on == "high" and any(f.severity in {"HIGH", "CRITICAL"} for f in report.findings):
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
