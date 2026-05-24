"""SHIP-13 static checks for config/gpu-release-thresholds.toml.

Validates that the threshold file:
- parses as TOML
- declares every benchmark currently in mai-adapters/tests/benchmarks.rs
- declares no phantom benchmarks (entries the Rust source doesn't emit)
- has well-formed bounds (max_us >= 0, min_us >= 0, max_us >= min_us
  unless max_us is 0 sentinel)
- declares a sensible regression policy
"""

from __future__ import annotations

import re
import tomllib
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
THRESHOLDS_FILE = REPO_ROOT / "config" / "gpu-release-thresholds.toml"
BENCH_SOURCE = REPO_ROOT / "mai-adapters" / "tests" / "benchmarks.rs"

NAME_RE = re.compile(r'name:\s*"([a-z0-9_]+)"\.to_string\(\)')


@pytest.fixture(scope="module")
def thresholds() -> dict:
    assert THRESHOLDS_FILE.exists(), f"missing: {THRESHOLDS_FILE}"
    with open(THRESHOLDS_FILE, "rb") as f:
        return tomllib.load(f)


@pytest.fixture(scope="module")
def declared_names(thresholds: dict) -> set[str]:
    return {b["name"] for b in thresholds.get("benchmark", [])}


@pytest.fixture(scope="module")
def source_names() -> set[str]:
    assert BENCH_SOURCE.exists(), f"missing: {BENCH_SOURCE}"
    return set(NAME_RE.findall(BENCH_SOURCE.read_text(encoding="utf-8")))


def test_thresholds_file_exists() -> None:
    assert THRESHOLDS_FILE.exists()


def test_thresholds_parses(thresholds: dict) -> None:
    assert isinstance(thresholds, dict)
    assert "benchmark" in thresholds
    assert isinstance(thresholds["benchmark"], list)
    assert len(thresholds["benchmark"]) > 0


def test_policy_block_present(thresholds: dict) -> None:
    policy = thresholds.get("policy", {})
    assert "regression_pct" in policy
    assert isinstance(policy["regression_pct"], (int, float))
    assert 0 < policy["regression_pct"] <= 100


def test_policy_flags_boolean(thresholds: dict) -> None:
    policy = thresholds["policy"]
    for flag in ("allow_zero_target", "fail_on_missing", "fail_on_unknown"):
        assert isinstance(policy[flag], bool), f"{flag} must be bool"


def test_every_source_benchmark_has_a_threshold(
    source_names: set[str], declared_names: set[str]
) -> None:
    missing = source_names - declared_names
    assert not missing, (
        f"benchmarks present in Rust source but absent from thresholds: {sorted(missing)}"
    )


def test_no_phantom_benchmarks_in_thresholds(
    source_names: set[str], declared_names: set[str]
) -> None:
    phantom = declared_names - source_names
    assert not phantom, (
        f"thresholds declare benchmarks the Rust source does not emit: {sorted(phantom)}"
    )


def test_each_entry_has_required_fields(thresholds: dict) -> None:
    for entry in thresholds["benchmark"]:
        for field in ("name", "required", "max_us", "min_us", "description"):
            assert field in entry, f"{entry.get('name', '<unnamed>')}: missing {field}"


def test_max_us_non_negative(thresholds: dict) -> None:
    for entry in thresholds["benchmark"]:
        assert entry["max_us"] >= 0, f"{entry['name']}: max_us negative"


def test_min_us_non_negative(thresholds: dict) -> None:
    for entry in thresholds["benchmark"]:
        assert entry["min_us"] >= 0, f"{entry['name']}: min_us negative"


def test_max_us_above_min_us_when_nonzero(thresholds: dict) -> None:
    for entry in thresholds["benchmark"]:
        if entry["max_us"] > 0:
            assert entry["max_us"] >= entry["min_us"], (
                f"{entry['name']}: max_us {entry['max_us']} < min_us {entry['min_us']}"
            )


def test_required_field_is_bool(thresholds: dict) -> None:
    for entry in thresholds["benchmark"]:
        assert isinstance(entry["required"], bool), (
            f"{entry['name']}: required must be bool"
        )


def test_description_non_empty(thresholds: dict) -> None:
    for entry in thresholds["benchmark"]:
        assert isinstance(entry["description"], str) and entry["description"].strip(), (
            f"{entry['name']}: description must be a non-empty string"
        )


def test_names_unique(thresholds: dict) -> None:
    names = [b["name"] for b in thresholds["benchmark"]]
    assert len(names) == len(set(names)), f"duplicate benchmark names: {names}"


def test_minimum_benchmark_count(thresholds: dict) -> None:
    # SHIP-13 ships with 8 benchmarks; future releases may add more.
    assert len(thresholds["benchmark"]) >= 8


def test_all_required_benchmarks_marked_required(thresholds: dict) -> None:
    # If we drop required=true on any of the 8 SHIP-13 benchmarks, the
    # gate stops failing on missing — that's a regression of the release
    # contract and worth a specific test.
    required = [b["name"] for b in thresholds["benchmark"] if b["required"]]
    assert len(required) >= 8
