"""SHIP-14: sh/ps1 contract parity — required flags, phase functions,
report fields, and shared markers (SHIP-14, schema_version=1)."""

from __future__ import annotations

import re

import pytest

from .conftest import BASH_DRIVER, PS1_DRIVER

# ─── flags ──────────────────────────────────────────────────────────────

REQUIRED_BASH_FLAGS = [
    "--smoke",
    "--duration",
    "--profile",
    "--output",
    "--signing-key",
    "--anchor-id",
    "--api-binary",
    "--admin-binary",
    "--validator-binary",
    "--api-url",
    "--target",
    "--no-load",
    "--sample-interval",
    "--concurrency",
]

REQUIRED_PS1_PARAMS = [
    "$Smoke",
    "$DurationSeconds",
    "$Profile",
    "$Output",
    "$SigningKey",
    "$AnchorId",
    "$ApiBinary",
    "$AdminBinary",
    "$ValidatorBinary",
    "$ApiUrl",
    "$Target",
    "$NoLoad",
    "$SampleInterval",
    "$Concurrency",
]


@pytest.mark.parametrize("flag", REQUIRED_BASH_FLAGS)
def test_bash_documents_each_flag(flag: str) -> None:
    text = BASH_DRIVER.read_text(encoding="utf-8")
    assert flag in text, f"bash driver missing flag {flag}"


@pytest.mark.parametrize("param", REQUIRED_PS1_PARAMS)
def test_ps1_declares_each_param(param: str) -> None:
    text = PS1_DRIVER.read_text(encoding="utf-8")
    assert param in text, f"ps1 driver missing param {param}"


# ─── phases ─────────────────────────────────────────────────────────────

REQUIRED_PHASES = [
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


@pytest.mark.parametrize("phase", REQUIRED_PHASES)
def test_bash_invokes_each_phase(phase: str) -> None:
    text = BASH_DRIVER.read_text(encoding="utf-8")
    func = "phase_" + phase.replace("-", "_")
    assert func in text, f"bash driver missing phase fn {func}"


@pytest.mark.parametrize("phase", REQUIRED_PHASES)
def test_ps1_invokes_each_phase(phase: str) -> None:
    text = PS1_DRIVER.read_text(encoding="utf-8")
    parts = phase.split("-")
    func = "Phase-" + "".join(p[0].upper() + p[1:] for p in parts)
    assert func in text, f"ps1 driver missing phase fn {func}"


def test_bash_orchestrates_phases_in_order() -> None:
    """The bottom of the script calls each phase fn exactly once in order."""
    text = BASH_DRIVER.read_text(encoding="utf-8")
    expected = [
        "phase_preflight",
        "phase_service_start",
        "phase_mixed_workload",
        "phase_policy_triggers",
        "phase_trust_degradation",
        "phase_adapter_restart",
        "phase_backup_during_load",
        "phase_restore_side_env",
        "phase_metrics_capture",
        "phase_ship_validate",
    ]
    positions = []
    # search only the orchestration tail (last 60 lines or so).
    tail = "\n".join(text.splitlines()[-60:])
    for fn in expected:
        m = re.search(rf"^{fn}$", tail, re.MULTILINE)
        assert m is not None, f"orchestration tail missing {fn}"
        positions.append(m.start())
    assert positions == sorted(positions), "phases must be invoked in declared order"


def test_ps1_orchestrates_phases_in_order() -> None:
    text = PS1_DRIVER.read_text(encoding="utf-8")
    expected = [
        "Phase-Preflight",
        "Phase-ServiceStart",
        "Phase-MixedWorkload",
        "Phase-PolicyTriggers",
        "Phase-TrustDegradation",
        "Phase-AdapterRestart",
        "Phase-BackupDuringLoad",
        "Phase-RestoreSideEnv",
        "Phase-MetricsCapture",
        "Phase-ShipValidate",
    ]
    positions = []
    tail = "\n".join(text.splitlines()[-60:])
    for fn in expected:
        m = re.search(rf"^{fn}$", tail, re.MULTILINE)
        assert m is not None, f"orchestration tail missing {fn}"
        positions.append(m.start())
    assert positions == sorted(positions), "phases must be invoked in declared order"


# ─── report shape parity ────────────────────────────────────────────────

REQUIRED_REPORT_FIELDS = [
    "schema_version",
    "ship_session",
    "run_id",
    "mode",
    "duration_seconds",
    "host",
    "totals",
    "phases",
    "signatures",
]


@pytest.mark.parametrize("field", REQUIRED_REPORT_FIELDS)
def test_both_drivers_emit_each_report_field(field: str) -> None:
    sh = BASH_DRIVER.read_text(encoding="utf-8")
    ps = PS1_DRIVER.read_text(encoding="utf-8")
    assert field in sh, f"bash driver missing report field {field}"
    assert field in ps, f"ps1 driver missing report field {field}"


def test_both_drivers_use_same_ship_session_marker() -> None:
    assert '"SHIP-14"' in BASH_DRIVER.read_text(encoding="utf-8")
    assert '"SHIP-14"' in PS1_DRIVER.read_text(encoding="utf-8")


def test_both_drivers_emit_schema_version_1() -> None:
    sh = BASH_DRIVER.read_text(encoding="utf-8")
    ps = PS1_DRIVER.read_text(encoding="utf-8")
    assert '"schema_version": 1' in sh
    assert "schema_version = 1" in ps


def test_both_drivers_default_to_72_hours() -> None:
    sh = BASH_DRIVER.read_text(encoding="utf-8")
    ps = PS1_DRIVER.read_text(encoding="utf-8")
    assert "259200" in sh, "bash default duration must be 72*3600"
    assert "259200" in ps, "ps1 default duration must be 72*3600"


def test_both_drivers_smoke_collapses_duration() -> None:
    sh = BASH_DRIVER.read_text(encoding="utf-8")
    ps = PS1_DRIVER.read_text(encoding="utf-8")
    assert "DURATION_SECONDS=60" in sh
    assert "$DurationSeconds = 60" in ps


def test_both_drivers_invoke_signer() -> None:
    sh = BASH_DRIVER.read_text(encoding="utf-8")
    ps = PS1_DRIVER.read_text(encoding="utf-8")
    assert "burn-in-report-sign.py" in sh
    assert "burn-in-report-sign.py" in ps


def test_both_drivers_have_policy_no_payload_guarantee() -> None:
    """SHIP-HARDENING-PLAN §11.3: policy-triggering prompts must not log payloads."""
    sh = BASH_DRIVER.read_text(encoding="utf-8")
    ps = PS1_DRIVER.read_text(encoding="utf-8")
    assert '"payloads_logged": false' in sh
    assert "payloads_logged = $false" in ps


def test_both_drivers_use_admin_binary_for_backup_and_restore() -> None:
    sh = BASH_DRIVER.read_text(encoding="utf-8")
    ps = PS1_DRIVER.read_text(encoding="utf-8")
    assert "backup create" in sh
    assert "backup verify" in sh
    assert "restore plan" in sh
    assert "restore apply" in sh
    assert '"backup", "create"' in ps
    assert '"backup", "verify"' in ps
    assert '"restore", "plan"' in ps
    assert '"restore", "apply"' in ps


def test_both_drivers_call_ship_validate() -> None:
    sh = BASH_DRIVER.read_text(encoding="utf-8")
    ps = PS1_DRIVER.read_text(encoding="utf-8")
    assert "mai-ship-validate" in sh
    assert "mai-ship-validate" in ps


def test_both_drivers_document_exit_codes() -> None:
    sh = BASH_DRIVER.read_text(encoding="utf-8")
    ps = PS1_DRIVER.read_text(encoding="utf-8")
    # Exit codes 0..4 must be reachable in both.
    for code in ["exit 0", "exit 1", "exit 2", "exit 3", "exit 4"]:
        assert code in sh, f"bash missing {code}"
        assert code in ps, f"ps1 missing {code}"
