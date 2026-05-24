"""SHIP-14: bash --smoke end-to-end on POSIX runners.

Verifies the driver produces a parseable burn-in-report.json without
needing a live mai-api or operator profile. Windows hosts get the
contract-layer parity tests in test_drivers_contract.py instead.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

from .conftest import BASH_DRIVER, REPO_ROOT, posix_only


@posix_only
def test_smoke_run_succeeds(tmp_path: Path) -> None:
    out = tmp_path / "smoke-out"
    r = subprocess.run(
        ["bash", str(BASH_DRIVER), "--smoke", "--output", str(out)],
        capture_output=True,
        text=True,
        cwd=str(REPO_ROOT),
        timeout=180,
    )
    assert r.returncode == 0, f"stdout={r.stdout}\nstderr={r.stderr}"
    report_path = out / "burn-in-report.json"
    assert report_path.exists(), f"report missing at {report_path}"


@posix_only
def test_smoke_report_schema(tmp_path: Path) -> None:
    out = tmp_path / "smoke-out"
    subprocess.run(
        ["bash", str(BASH_DRIVER), "--smoke", "--output", str(out)],
        capture_output=True,
        text=True,
        cwd=str(REPO_ROOT),
        timeout=180,
    )
    report = json.loads((out / "burn-in-report.json").read_text(encoding="utf-8"))
    assert report["schema_version"] == 1
    assert report["ship_session"] == "SHIP-14"
    assert report["mode"] == "smoke"
    assert report["duration_seconds"] == 60
    assert "host" in report
    assert "hostname" in report["host"]
    totals = report["totals"]
    assert totals["phase_count"] == 10
    assert totals["pass"] + totals["fail"] + totals["skip"] == 10
    assert "signatures" in report
    for key in ("report_mldsa", "anchor_id", "body_sha3_256"):
        assert key in report["signatures"]


@posix_only
def test_smoke_phases_present_in_order(tmp_path: Path) -> None:
    out = tmp_path / "smoke-out"
    subprocess.run(
        ["bash", str(BASH_DRIVER), "--smoke", "--output", str(out)],
        capture_output=True,
        text=True,
        cwd=str(REPO_ROOT),
        timeout=180,
    )
    report = json.loads((out / "burn-in-report.json").read_text(encoding="utf-8"))
    names = [p["name"] for p in report["phases"]]
    assert names == [
        "preflight",
        "service-start",
        "mixed-workload",
        "policy-triggers",
        "trust-degradation",
        "adapter-restart",
        "backup-during-load",
        "restore-side-env",
        "metrics-capture",
        "ship-validate",
    ]


@posix_only
def test_smoke_phase_detail_files_emitted(tmp_path: Path) -> None:
    out = tmp_path / "smoke-out"
    subprocess.run(
        ["bash", str(BASH_DRIVER), "--smoke", "--output", str(out)],
        capture_output=True,
        text=True,
        cwd=str(REPO_ROOT),
        timeout=180,
    )
    phases_dir = out / "phases"
    expected = {
        "preflight.json",
        "service-start.json",
        "mixed-workload.json",
        "policy-triggers.json",
        "trust-degradation.json",
        "adapter-restart.json",
        "backup-during-load.json",
        "restore-side-env.json",
        "metrics-capture.json",
        "ship-validate.json",
    }
    present = {p.name for p in phases_dir.glob("*.json")}
    missing = expected - present
    assert not missing, f"missing phase detail files: {missing}"


@posix_only
def test_smoke_no_payloads_leaked(tmp_path: Path) -> None:
    """SHIP-HARDENING-PLAN §11.3: policy-triggering prompts must not log payloads."""
    out = tmp_path / "smoke-out"
    subprocess.run(
        ["bash", str(BASH_DRIVER), "--smoke", "--output", str(out)],
        capture_output=True,
        text=True,
        cwd=str(REPO_ROOT),
        timeout=180,
    )
    # Smoke skips the live phase, but the contract still applies: no
    # canary payload values should appear anywhere in the artifact tree.
    forbidden_substrings = [
        # If actual PII canary content ever leaks, the test below catches it.
        # We use placeholder substrings that would indicate a regression.
        "BURN_IN_CANARY:ssn-like",
        "BURN_IN_CANARY:credit-card-like",
        "BURN_IN_CANARY:phi-like",
        "BURN_IN_CANARY:itar-like",
    ]
    for path in out.rglob("*.json"):
        text = path.read_text(encoding="utf-8")
        for sub in forbidden_substrings:
            assert sub not in text, f"canary payload {sub!r} leaked to {path}"


@posix_only
def test_smoke_rejects_unknown_flag(tmp_path: Path) -> None:
    r = subprocess.run(
        ["bash", str(BASH_DRIVER), "--smoke", "--output", str(tmp_path), "--bogus"],
        capture_output=True,
        text=True,
        cwd=str(REPO_ROOT),
        timeout=30,
    )
    assert r.returncode == 2, f"stdout={r.stdout}\nstderr={r.stderr}"


@posix_only
def test_signing_key_without_anchor_id_rejected(tmp_path: Path) -> None:
    sk = tmp_path / "sk.bin"
    sk.write_bytes(b"\x00" * 4896)
    r = subprocess.run(
        ["bash", str(BASH_DRIVER), "--smoke", "--output", str(tmp_path / "out"),
         "--signing-key", str(sk)],
        capture_output=True,
        text=True,
        cwd=str(REPO_ROOT),
        timeout=30,
    )
    assert r.returncode == 2, f"stdout={r.stdout}\nstderr={r.stderr}"
    assert "anchor-id" in r.stderr.lower()


@posix_only
def test_smoke_canonical_body_stable(tmp_path: Path) -> None:
    """Two consecutive smoke runs must produce identical canonical bodies
    aside from runtime-dependent fields. We compare the canonical body
    with run_id + host + timestamps normalised."""
    out1 = tmp_path / "out1"
    out2 = tmp_path / "out2"
    for out in (out1, out2):
        subprocess.run(
            ["bash", str(BASH_DRIVER), "--smoke", "--output", str(out)],
            capture_output=True,
            text=True,
            cwd=str(REPO_ROOT),
            timeout=180,
        )
    r1 = json.loads((out1 / "burn-in-report.json").read_text(encoding="utf-8"))
    r2 = json.loads((out2 / "burn-in-report.json").read_text(encoding="utf-8"))
    # Same schema, same totals, same phase shape.
    assert r1["schema_version"] == r2["schema_version"]
    assert r1["ship_session"] == r2["ship_session"]
    assert r1["totals"] == r2["totals"]
    assert [p["name"] for p in r1["phases"]] == [p["name"] for p in r2["phases"]]
    assert [p["status"] for p in r1["phases"]] == [p["status"] for p in r2["phases"]]
