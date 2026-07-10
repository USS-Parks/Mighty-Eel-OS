"""SHIP-12 workflow structural checks for ship-validation.yml.

Pins the contract that .github/workflows/ship-validation.yml encodes the
gates the SHIP-12 plan calls for:
  - forbidden-term-scan, ship-validator, mai-admin-backup,
    package-build-validate, mypy-strict-sdk, nightly-integration
  - schedule trigger present so the nightly matrix actually fires
  - no `continue-on-error` on any step (SHIP-12 is hard-gate enforcement)
  - every `uses:` action is pinned to a major version, not `@latest`
"""

from __future__ import annotations

import re
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = REPO_ROOT / ".github" / "workflows" / "ship-validation.yml"
README = REPO_ROOT / ".github" / "workflows" / "ship-validation-README.md"

REQUIRED_JOBS = {
    "forbidden-term-scan",
    "ship-validator",
    "mai-admin-backup",
    "package-build-validate",
    "compose-trust-validation",
    "mypy-strict-sdk",
    "nightly-integration",
}


@pytest.fixture(scope="module")
def workflow_yaml() -> dict:
    yaml = pytest.importorskip("yaml")
    assert WORKFLOW.exists(), f"missing workflow: {WORKFLOW}"
    return yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))


def test_workflow_file_exists() -> None:
    assert WORKFLOW.exists(), "SHIP-12 ship-validation.yml is required"


def test_workflow_readme_exists() -> None:
    assert README.exists(), "SHIP-12 ship-validation-README.md is required"


def test_workflow_has_name(workflow_yaml: dict) -> None:
    assert workflow_yaml.get("name", "").strip() != ""


def test_workflow_has_required_jobs(workflow_yaml: dict) -> None:
    jobs = set(workflow_yaml.get("jobs", {}).keys())
    missing = REQUIRED_JOBS - jobs
    assert not missing, f"missing SHIP-12 jobs: {sorted(missing)}"


def test_workflow_has_schedule_trigger(workflow_yaml: dict) -> None:
    # PyYAML coerces the bare key `on:` to Python boolean True. Honor either.
    triggers = workflow_yaml.get("on") or workflow_yaml.get(True)
    assert isinstance(triggers, dict), f"unexpected `on:` shape: {triggers!r}"
    assert "schedule" in triggers, "nightly schedule trigger is required"
    schedule = triggers["schedule"]
    assert isinstance(schedule, list) and schedule, "schedule must be a non-empty list"
    assert any("cron" in entry for entry in schedule), "at least one cron entry required"


def test_workflow_triggers_on_push_and_pr(workflow_yaml: dict) -> None:
    triggers = workflow_yaml.get("on") or workflow_yaml.get(True)
    assert "push" in triggers
    assert "pull_request" in triggers


def test_workflow_triggers_have_workflow_dispatch(workflow_yaml: dict) -> None:
    triggers = workflow_yaml.get("on") or workflow_yaml.get(True)
    assert "workflow_dispatch" in triggers, "manual rerun must be available"


def test_no_continue_on_error_anywhere(workflow_yaml: dict) -> None:
    offenders: list[str] = []
    for job_name, job in workflow_yaml.get("jobs", {}).items():
        for step in job.get("steps", []):
            if step.get("continue-on-error"):
                offenders.append(f"{job_name}:{step.get('name', '?')}")
    assert not offenders, (
        f"SHIP-12 is hard-gate; remove continue-on-error from: {offenders}"
    )


def test_every_action_uses_pinned_major(workflow_yaml: dict) -> None:
    bad: list[str] = []
    for job_name, job in workflow_yaml.get("jobs", {}).items():
        for step in job.get("steps", []):
            uses = step.get("uses")
            if not uses:
                continue
            if uses.endswith("@latest") or "@" not in uses:
                bad.append(f"{job_name}:{step.get('name', '?')} -> {uses}")
    assert not bad, f"actions must pin a version: {bad}"


def test_forbidden_term_scan_runs_python_scanner(workflow_yaml: dict) -> None:
    job = workflow_yaml["jobs"]["forbidden-term-scan"]
    runs = [step.get("run", "") for step in job["steps"] if "run" in step]
    assert any("ci_forbidden_terms.py" in r for r in runs), (
        "forbidden-term-scan must invoke scripts/ci_forbidden_terms.py"
    )


def test_ship_validator_exit_code_probes_present(workflow_yaml: dict) -> None:
    job = workflow_yaml["jobs"]["ship-validator"]
    step_text = "\n".join(step.get("run", "") for step in job["steps"])
    assert "exit 2" in step_text, "must probe exit-2 (config unreadable)"
    assert "exit 3" in step_text, "must probe exit-3 (state dir missing)"
    assert "deployment/ship/profile.toml" in step_text
    assert "config/production.example.toml" in step_text


def test_ship_validator_builds_with_locked(workflow_yaml: dict) -> None:
    job = workflow_yaml["jobs"]["ship-validator"]
    step_text = "\n".join(step.get("run", "") for step in job["steps"])
    assert "--locked" in step_text, "cargo build must use --locked in CI"


def test_mai_admin_job_runs_mai_admin_tests(workflow_yaml: dict) -> None:
    job = workflow_yaml["jobs"]["mai-admin-backup"]
    step_text = "\n".join(step.get("run", "") for step in job["steps"])
    assert "cargo test -p mai-admin" in step_text


def test_mypy_job_runs_strict_on_sdk(workflow_yaml: dict) -> None:
    job = workflow_yaml["jobs"]["mypy-strict-sdk"]
    step_text = "\n".join(step.get("run", "") for step in job["steps"])
    assert re.search(r"mypy\s+--strict\s+mai-sdk-python/src", step_text), (
        "mypy-strict-sdk must run `mypy --strict mai-sdk-python/src/`"
    )
    assert re.search(r"mypy\s+adapters/", step_text), (
        "mypy-strict-sdk must also enforce mypy on adapters/ (with overrides)"
    )


def test_package_build_job_validates_only(workflow_yaml: dict) -> None:
    job = workflow_yaml["jobs"]["package-build-validate"]
    step_text = "\n".join(step.get("run", "") for step in job["steps"])
    assert "build-package.sh --validate-only" in step_text
    assert "tools/packaging_tests" in step_text
    assert "tools/ship12_tests" in step_text


def test_compose_trust_job_validates_shipped_compositions(workflow_yaml: dict) -> None:
    job = workflow_yaml["jobs"]["compose-trust-validation"]
    step_text = "\n".join(step.get("run", "") for step in job["steps"])
    assert "--profile production deployment/wsf-ha/docker-compose.yml" in step_text, (
        "wsf-ha must be validated under the production rules"
    )
    assert "--profile demo deployment/appliance/docker-compose.yml" in step_text, (
        "the appliance demo must be validated under the demo rules"
    )
    assert "--profile demo deployment/shadow/docker-compose.yml" in step_text, (
        "the shadow lead artifact must be validated under the demo rules"
    )
    assert "deployment/appliance/tests" in step_text, (
        "the validator's own regression tests must run in CI"
    )


def test_nightly_job_gated_on_schedule(workflow_yaml: dict) -> None:
    job = workflow_yaml["jobs"]["nightly-integration"]
    condition = job.get("if", "")
    assert "schedule" in condition, (
        "nightly-integration must gate on github.event_name == 'schedule'"
    )


def test_nightly_job_depends_on_all_pr_gates(workflow_yaml: dict) -> None:
    job = workflow_yaml["jobs"]["nightly-integration"]
    needs = set(job.get("needs", []))
    expected = REQUIRED_JOBS - {"nightly-integration"}
    missing = expected - needs
    assert not missing, f"nightly must depend on every PR gate; missing: {missing}"


def test_workflow_uses_ubuntu_latest_for_pr_jobs(workflow_yaml: dict) -> None:
    pr_jobs = REQUIRED_JOBS - {"nightly-integration"}
    for name in pr_jobs:
        runs_on = workflow_yaml["jobs"][name].get("runs-on")
        assert runs_on == "ubuntu-latest", (
            f"PR gate {name} must run on ubuntu-latest (got {runs_on!r})"
        )
