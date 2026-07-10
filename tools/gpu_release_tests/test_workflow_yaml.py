"""SHIP-13 static checks for .github/workflows/gpu-release.yml.

Validates structural invariants the workflow contract relies on:
- runs on self-hosted GPU runners (no GitHub-hosted leak)
- uploads each promised artifact
- enforces the threshold gate
- triggers cover tags + dispatch + schedule
- no `latest` action pin (every action references @vN)
"""

from __future__ import annotations

from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = REPO_ROOT / ".github" / "workflows" / "gpu-release.yml"

REQUIRED_JOBS = [
    "gpu-build",
    "gpu-integration",
    "gpu-benchmarks",
    "gpu-package",
    "gpu-bundle",
]

REQUIRED_ARTIFACT_NAMES = [
    "readiness-report-",
    "benchmark-results-",
    "mai-deb-",
    "release-bundle-",
]


@pytest.fixture(scope="module")
def workflow_text() -> str:
    assert WORKFLOW.exists(), f"missing workflow: {WORKFLOW}"
    return WORKFLOW.read_text(encoding="utf-8")


@pytest.fixture(scope="module")
def workflow_yaml(workflow_text: str) -> dict:
    yaml = pytest.importorskip("yaml")
    return yaml.safe_load(workflow_text)


def test_workflow_file_exists() -> None:
    assert WORKFLOW.exists(), "SHIP-13 gpu-release.yml is required"


def test_workflow_parses_as_yaml(workflow_yaml: dict) -> None:
    assert isinstance(workflow_yaml, dict)
    assert "jobs" in workflow_yaml


def test_workflow_has_name(workflow_yaml: dict) -> None:
    assert workflow_yaml.get("name", "").strip() != ""


@pytest.mark.parametrize("job", REQUIRED_JOBS)
def test_required_jobs_present(workflow_yaml: dict, job: str) -> None:
    assert job in workflow_yaml["jobs"], f"missing job: {job}"


@pytest.mark.parametrize("job", REQUIRED_JOBS)
def test_jobs_run_on_self_hosted_gpu(workflow_yaml: dict, job: str) -> None:
    runs_on = workflow_yaml["jobs"][job]["runs-on"]
    if isinstance(runs_on, str):
        labels = [runs_on]
    else:
        labels = list(runs_on)
    assert "self-hosted" in labels, f"{job}: must run self-hosted, got {labels}"
    assert "gpu" in labels, f"{job}: must require gpu label, got {labels}"


def test_no_github_hosted_runner(workflow_text: str) -> None:
    assert "ubuntu-latest" not in workflow_text, (
        "GPU workflow must not target ubuntu-latest (CI lane covers that)"
    )
    assert "windows-latest" not in workflow_text


def test_triggers_cover_tags_dispatch_schedule(workflow_yaml: dict) -> None:
    triggers = workflow_yaml.get(True) or workflow_yaml.get("on")
    # YAML's "on:" key can parse as Python True; accept both spellings.
    assert isinstance(triggers, dict), "workflow needs an `on:` mapping"
    assert "push" in triggers
    assert "workflow_dispatch" in triggers
    assert "schedule" in triggers
    # Tag-triggered release path is the headline use case.
    push = triggers["push"]
    assert "tags" in push
    assert any(t.startswith("v") for t in push["tags"])


def test_workflow_runs_threshold_gate(workflow_text: str) -> None:
    assert "bench_compare.py gate" in workflow_text, (
        "GPU benchmarks job must call the gate subcommand"
    )
    assert "--thresholds" in workflow_text
    assert "gpu-release-thresholds.toml" in workflow_text


def test_workflow_calls_release_bundle(workflow_text: str) -> None:
    assert "gpu-release-bundle.sh" in workflow_text, (
        "gpu-bundle job must invoke the bundle script"
    )


def test_workflow_invokes_validate_for_readiness(workflow_text: str) -> None:
    assert "mai-api validate" in workflow_text
    assert "readiness-report.json" in workflow_text


@pytest.mark.parametrize("artifact_prefix", REQUIRED_ARTIFACT_NAMES)
def test_each_required_artifact_is_uploaded(
    workflow_text: str, artifact_prefix: str
) -> None:
    assert artifact_prefix in workflow_text, (
        f"workflow must upload artifact starting with '{artifact_prefix}'"
    )


def test_actions_pinned_to_major_version(workflow_text: str) -> None:
    # Every `uses:` reference must pin to @vN (or @sha). Latest tags are
    # banned per supply-chain hygiene.
    uses_lines = [
        line.strip()
        for line in workflow_text.splitlines()
        if line.lstrip().startswith("- uses:")
    ]
    assert uses_lines, "workflow should reference at least one action"
    for line in uses_lines:
        assert "@" in line, f"unpinned action reference: {line}"
        assert "@latest" not in line, f"forbidden @latest pin: {line}"
        assert "@main" not in line, f"forbidden @main pin: {line}"


def test_gpu_bundle_depends_on_all_upstream_jobs(workflow_yaml: dict) -> None:
    needs = workflow_yaml["jobs"]["gpu-bundle"].get("needs", [])
    if isinstance(needs, str):
        needs = [needs]
    for dep in ("gpu-build", "gpu-integration", "gpu-benchmarks", "gpu-package"):
        assert dep in needs, f"gpu-bundle must depend on {dep}"


def test_gpu_package_can_be_skipped(workflow_yaml: dict) -> None:
    pkg_job = workflow_yaml["jobs"]["gpu-package"]
    cond = pkg_job.get("if", "")
    assert "skip_package" in cond, "gpu-package must honour skip_package input"


def test_timeout_minutes_set_for_every_job(workflow_yaml: dict) -> None:
    for job in REQUIRED_JOBS:
        timeout = workflow_yaml["jobs"][job].get("timeout-minutes")
        assert isinstance(timeout, int) and timeout > 0, (
            f"{job}: timeout-minutes must be a positive int"
        )


def test_release_version_resolved_from_tag(workflow_text: str) -> None:
    # The build job extracts version from refs/tags/v* via a `version`
    # step output, which the bundle/package jobs depend on.
    assert "release_version" in workflow_text
    assert "refs/tags/v" in workflow_text


def test_correct_env_paths(workflow_yaml: dict) -> None:
    env = workflow_yaml.get("env", {})
    assert env.get("GPU_RELEASE_PROFILE") == "deployment/ship/profile.toml"
    assert env.get("GPU_RELEASE_THRESHOLDS") == "config/gpu-release-thresholds.toml"


def test_bundle_never_blessed_on_failed_gates(workflow_yaml: dict) -> None:
    # A failed integration / benchmark / package run must block the signed
    # bundle. `always()` would resurrect failures; the condition instead
    # requires success from every executed gate (gpu-package alone may be
    # 'skipped' via the explicit skip_package dispatch input).
    cond = workflow_yaml["jobs"]["gpu-bundle"].get("if", "")
    assert "always()" not in cond, "gpu-bundle must not run via always()"
    for dep in ("gpu-build", "gpu-integration", "gpu-benchmarks"):
        assert f"needs.{dep}.result == 'success'" in cond, (
            f"gpu-bundle must require {dep} success, got: {cond}"
        )
    assert "needs.gpu-package.result == 'success'" in cond
    assert "needs.gpu-package.result == 'skipped'" in cond


def test_readiness_check_is_a_hard_gate(workflow_yaml: dict) -> None:
    # The only continue-on-error allowed in the release lane is the
    # advisory benchmark comparison; the readiness/validate step (and
    # everything else) must fail the build when it fails.
    soft_steps = [
        f"{job_name}:{step.get('name', '?')}"
        for job_name, job in workflow_yaml["jobs"].items()
        for step in job.get("steps", [])
        if step.get("continue-on-error")
    ]
    assert soft_steps == ["gpu-benchmarks:Compare with previous run (advisory)"], (
        f"unexpected soft steps in the release lane: {soft_steps}"
    )
