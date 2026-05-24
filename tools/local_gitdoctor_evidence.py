#!/usr/bin/env python3
"""Three-layer evidence runner for the local GitDoctor-style audit.

Layer 1 runs the mapped scanner that mirrors the Dougherty findings.
Layer 2 runs independent mature tools when they are installed locally.
Layer 3 runs adversarial scanner fixtures so we can prove the mapped
rules detect behavior rather than only matching this repository.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import shutil
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
SCAN_PATH = Path(__file__).resolve().with_name("local_gitdoctor_scan.py")


@dataclass(frozen=True)
class ProbeDef:
    probe_id: str
    layer: str
    toolchain: str
    title: str
    command: list[str]
    required_paths: tuple[str, ...] = ()
    availability_command: list[str] | None = None
    availability_module: str | None = None
    timeout_seconds: int = 180


@dataclass
class ProbeResult:
    probe_id: str
    layer: str
    toolchain: str
    title: str
    status: str
    command: list[str]
    exit_code: int | None = None
    output_tail: str = ""
    reason: str = ""

    def to_dict(self) -> dict[str, object]:
        return {
            "probe_id": self.probe_id,
            "layer": self.layer,
            "toolchain": self.toolchain,
            "title": self.title,
            "status": self.status,
            "command": self.command,
            "exit_code": self.exit_code,
            "output_tail": self.output_tail,
            "reason": self.reason,
        }


@dataclass
class EvidenceReport:
    root: str
    mapped_scan: dict[str, object]
    probes: list[ProbeResult] = field(default_factory=list)

    def to_dict(self) -> dict[str, object]:
        return {
            "root": self.root,
            "mapped_scan": self.mapped_scan,
            "probes": [probe.to_dict() for probe in self.probes],
            "summary": summarize(self.probes),
        }


def load_scanner():
    spec = importlib.util.spec_from_file_location("local_gitdoctor_scan", SCAN_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"Unable to load scanner at {SCAN_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def module_available(name: str) -> bool:
    return importlib.util.find_spec(name) is not None


def command_available(command: list[str]) -> bool:
    if not command:
        return False
    if command[0] == sys.executable and len(command) >= 3 and command[1] == "-m":
        return module_available(command[2])
    return shutil.which(command[0]) is not None


def required_paths_exist(root: Path, probe: ProbeDef) -> bool:
    return all((root / item).exists() for item in probe.required_paths)


def availability_ok(root: Path, probe: ProbeDef) -> tuple[bool, str]:
    if not required_paths_exist(root, probe):
        missing = [item for item in probe.required_paths if not (root / item).exists()]
        return False, f"missing required path(s): {', '.join(missing)}"
    if probe.availability_module and not module_available(probe.availability_module):
        return False, f"python module not installed: {probe.availability_module}"
    availability = probe.availability_command or probe.command
    if not command_available(availability):
        return False, f"tool not installed: {availability[0]}"
    if probe.availability_command:
        try:
            check = subprocess.run(
                probe.availability_command,
                cwd=root,
                text=True,
                capture_output=True,
                timeout=30,
            )
        except (OSError, subprocess.TimeoutExpired) as exc:
            return False, f"availability probe failed: {exc}"
        if check.returncode != 0:
            return False, "availability probe returned non-zero"
    return True, ""


def run_probe(root: Path, probe: ProbeDef) -> ProbeResult:
    ok, reason = availability_ok(root, probe)
    if not ok:
        return ProbeResult(
            probe_id=probe.probe_id,
            layer=probe.layer,
            toolchain=probe.toolchain,
            title=probe.title,
            status="SKIPPED",
            command=probe.command,
            reason=reason,
        )
    try:
        completed = subprocess.run(
            probe.command,
            cwd=root,
            text=True,
            encoding="utf-8",
            errors="replace",
            capture_output=True,
            timeout=probe.timeout_seconds,
        )
    except subprocess.TimeoutExpired as exc:
        output = ((exc.stdout or "") + "\n" + (exc.stderr or "")).strip()
        return ProbeResult(
            probe_id=probe.probe_id,
            layer=probe.layer,
            toolchain=probe.toolchain,
            title=probe.title,
            status="FAIL",
            command=probe.command,
            output_tail=tail(output),
            reason=f"timed out after {probe.timeout_seconds}s",
        )
    except OSError as exc:
        return ProbeResult(
            probe_id=probe.probe_id,
            layer=probe.layer,
            toolchain=probe.toolchain,
            title=probe.title,
            status="SKIPPED",
            command=probe.command,
            reason=str(exc),
        )
    output = ((completed.stdout or "") + "\n" + (completed.stderr or "")).strip()
    return ProbeResult(
        probe_id=probe.probe_id,
        layer=probe.layer,
        toolchain=probe.toolchain,
        title=probe.title,
        status="PASS" if completed.returncode == 0 else "FAIL",
        command=probe.command,
        exit_code=completed.returncode,
        output_tail=tail(output),
    )


def tail(text: str, limit: int = 4000) -> str:
    if len(text) <= limit:
        return text
    return text[-limit:]


def build_probes(root: Path) -> list[ProbeDef]:
    probes: list[ProbeDef] = [
        ProbeDef(
            "ADV-001",
            "adversarial-fixture",
            "scanner",
            "Known-bad and known-clean scanner fixtures",
            [sys.executable, "-m", "pytest", "tools/local_gitdoctor_tests", "-q"],
            availability_module="pytest",
            timeout_seconds=120,
        )
    ]
    if (root / "Cargo.toml").exists():
        probes.extend(
            [
                ProbeDef("IND-RS-001", "independent-implementation", "Rust", "cargo check workspace", ["cargo", "check", "--workspace"], timeout_seconds=600),
                ProbeDef("IND-RS-002", "independent-implementation", "Rust", "cargo clippy workspace", ["cargo", "clippy", "--workspace", "--", "-D", "warnings", "-A", "clippy::pedantic"], timeout_seconds=900),
                ProbeDef("IND-RS-003", "independent-implementation", "Rust", "cargo test workspace", ["cargo", "test", "--workspace"], timeout_seconds=900),
                ProbeDef("IND-RS-004", "independent-implementation", "Rust", "cargo audit", ["cargo", "audit"], availability_command=["cargo", "audit", "--version"], timeout_seconds=300),
                ProbeDef("IND-RS-005", "independent-implementation", "Rust", "cargo deny", ["cargo", "deny", "check"], availability_command=["cargo", "deny", "--version"], timeout_seconds=300),
            ]
        )
    if any((root / name).exists() for name in ["pyproject.toml", "requirements.txt", "requirements-lock.txt"]):
        probes.extend(
            [
                ProbeDef("IND-PY-001", "independent-implementation", "Python", "pytest repository tests", [sys.executable, "-m", "pytest", "-q", "--ignore=target", "--ignore=results"], availability_module="pytest", timeout_seconds=900),
                ProbeDef("IND-PY-002", "independent-implementation", "Python", "ruff lint", [sys.executable, "-m", "ruff", "check", "."], availability_module="ruff", timeout_seconds=300),
                # J-10b: bandit's default `txt` formatter writes `→` etc. via
                # the host stdout codec; on Windows that is cp1252 by default
                # and the formatter crashes mid-report with UnicodeEncodeError.
                # `-f json` routes through json.dumps and avoids the codec
                # path entirely. `-c pyproject.toml` activates the
                # `[tool.bandit]` policy block (skipped rule IDs +
                # exclude_dirs); bandit does not auto-discover pyproject.toml
                # without an explicit `-c`. The probe still FAILs on real
                # findings; it just no longer FAILs on the formatter, on
                # asserts-in-tests, or on stdlib-only urllib usage.
                ProbeDef("IND-PY-003", "independent-implementation", "Python", "bandit security scan", [sys.executable, "-m", "bandit", "-r", ".", "-f", "json", "-c", "pyproject.toml"], availability_module="bandit", timeout_seconds=300),
                ProbeDef("IND-PY-004", "independent-implementation", "Python", "pip-audit dependency scan", [sys.executable, "-m", "pip_audit"], availability_module="pip_audit", timeout_seconds=300),
            ]
        )
    if (root / "package.json").exists():
        probes.extend(
            [
                ProbeDef("IND-JS-001", "independent-implementation", "JS/TS", "npm lint script", ["npm", "run", "lint", "--if-present"], timeout_seconds=300),
                ProbeDef("IND-JS-002", "independent-implementation", "JS/TS", "npm typecheck script", ["npm", "run", "typecheck", "--if-present"], timeout_seconds=300),
                ProbeDef("IND-JS-003", "independent-implementation", "JS/TS", "npm audit", ["npm", "audit", "--audit-level=moderate"], timeout_seconds=300),
            ]
        )
    probes.extend(
        [
            ProbeDef("IND-SEC-001", "independent-implementation", "Secrets", "gitleaks secret scan", ["gitleaks", "detect", "--source", ".", "--no-git", "--redact"], availability_command=["gitleaks", "version"], timeout_seconds=300),
            ProbeDef("IND-SEC-002", "independent-implementation", "Secrets", "detect-secrets scan", ["detect-secrets", "scan", "--all-files"], availability_command=["detect-secrets", "--version"], timeout_seconds=300),
        ]
    )
    if any(path.name == "Dockerfile" or path.name.endswith(".Dockerfile") for path in root.rglob("*") if path.is_file()):
        probes.append(ProbeDef("IND-DOC-001", "independent-implementation", "Docker", "hadolint Dockerfile scan", ["hadolint", "Dockerfile"], availability_command=["hadolint", "--version"], timeout_seconds=180))
    probes.extend(
        [
            ProbeDef("IND-CPLX-001", "independent-implementation", "Complexity", "tokei line-count scan", ["tokei", "."], availability_command=["tokei", "--version"], timeout_seconds=180),
            ProbeDef("IND-CPLX-002", "independent-implementation", "Complexity", "scc complexity scan", ["scc", "."], availability_command=["scc", "--version"], timeout_seconds=180),
            ProbeDef("IND-CPLX-003", "independent-implementation", "Complexity", "radon complexity scan", [sys.executable, "-m", "radon", "cc", "."], availability_module="radon", timeout_seconds=180),
        ]
    )
    return probes


def summarize(probes: list[ProbeResult]) -> dict[str, dict[str, int]]:
    summary: dict[str, dict[str, int]] = {}
    for probe in probes:
        bucket = summary.setdefault(probe.layer, {"PASS": 0, "FAIL": 0, "SKIPPED": 0, "TOTAL": 0})
        bucket[probe.status] += 1
        bucket["TOTAL"] += 1
    return summary


def render_markdown(report: EvidenceReport) -> str:
    mapped = report.mapped_scan
    lines = [
        "# Local GitDoctor Evidence Package",
        "",
        f"Root: `{report.root}`",
        "",
        "## Layer 1: Mapped Checks",
        "",
        f"Overall score: **{mapped['overall_score']}/100**",
        f"Checks: {mapped['total_checks']} total, {mapped['passed']} passed, {mapped['failed']} failed",
        "",
        "This layer intentionally mirrors the Dougherty/GitDoctor finding families.",
        "",
        "## Layer 2: Independent Implementations",
        "",
        "These probes use mature local tools when installed. `SKIPPED` means the tool was not available or the project surface was absent; it is not a pass.",
        "",
        "| Probe | Toolchain | Status | Title |",
        "|---|---|---:|---|",
    ]
    for probe in report.probes:
        if probe.layer != "independent-implementation":
            continue
        lines.append(f"| {probe.probe_id} | {probe.toolchain} | {probe.status} | {probe.title} |")
    lines.extend(
        [
            "",
            "## Layer 3: Adversarial Fixtures",
            "",
            "| Probe | Status | Title |",
            "|---|---:|---|",
        ]
    )
    for probe in report.probes:
        if probe.layer == "adversarial-fixture":
            lines.append(f"| {probe.probe_id} | {probe.status} | {probe.title} |")
    lines.extend(["", "## Probe Details", ""])
    for probe in report.probes:
        lines.append(f"### {probe.probe_id} {probe.title}")
        lines.append("")
        lines.append(f"Layer: `{probe.layer}`")
        lines.append(f"Toolchain: `{probe.toolchain}`")
        lines.append(f"Status: `{probe.status}`")
        lines.append(f"Command: `{' '.join(probe.command)}`")
        if probe.exit_code is not None:
            lines.append(f"Exit code: `{probe.exit_code}`")
        if probe.reason:
            lines.append(f"Reason: {probe.reason}")
        if probe.output_tail:
            lines.append("")
            lines.append("```text")
            lines.append(probe.output_tail)
            lines.append("```")
        lines.append("")
    return "\n".join(lines).rstrip() + "\n"


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Build the three-layer local GitDoctor evidence package.")
    parser.add_argument("--root", type=Path, default=REPO_ROOT, help="Repository root to audit.")
    parser.add_argument("--output", type=Path, default=REPO_ROOT / "docs" / "LOCAL-GITDOCTOR-EVIDENCE.md", help="Markdown evidence output path.")
    parser.add_argument("--json-output", type=Path, default=REPO_ROOT / "docs" / "LOCAL-GITDOCTOR-EVIDENCE.json", help="JSON evidence output path.")
    parser.add_argument("--skip-independent", action="store_true", help="Run only mapped checks and adversarial fixtures.")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv or sys.argv[1:])
    root = args.root.resolve()
    scanner = load_scanner()
    mapped = scanner.run_scan(root).to_dict()
    probes = [probe for probe in build_probes(root) if not args.skip_independent or probe.layer != "independent-implementation"]
    results = [run_probe(root, probe) for probe in probes]
    report = EvidenceReport(root=str(root), mapped_scan=mapped, probes=results)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(render_markdown(report), encoding="utf-8")
    args.json_output.parent.mkdir(parents=True, exist_ok=True)
    args.json_output.write_text(json.dumps(report.to_dict(), indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return 1 if any(probe.status == "FAIL" for probe in results) else 0


if __name__ == "__main__":
    raise SystemExit(main())
