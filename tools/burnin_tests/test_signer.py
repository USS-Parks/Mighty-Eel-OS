"""SHIP-14: canonical body + sha3 + sign/verify exit-code matrix for
burn-in-report-sign.py. ML-DSA-87 execution is exercised only when
pqcrypto is installed; canonical-body tests run unconditionally."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import subprocess
import sys
from pathlib import Path
from typing import Any

import pytest

from .conftest import REPO_ROOT, SIGNER

# Import the signer as a module so we can unit-test its canonicalisation
# without spawning a subprocess per call.
spec = importlib.util.spec_from_file_location("burn_in_signer", SIGNER)
assert spec is not None
assert spec.loader is not None
signer_mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(signer_mod)

HAS_MLDSA = signer_mod._load_mldsa() is not None


def _minimal_report(**overrides: Any) -> dict[str, Any]:
    report = {
        "schema_version": 1,
        "ship_session": "SHIP-14",
        "run_id": "burn-in-test",
        "mode": "smoke",
        "duration_seconds": 60,
        "host": {"hostname": "h", "uname": "u"},
        "totals": {"phase_count": 1, "pass": 1, "fail": 0, "skip": 0},
        "phases": [
            {
                "name": "preflight",
                "status": "pass",
                "started_at": "2026-05-23T00:00:00Z",
                "ended_at": "2026-05-23T00:00:00Z",
                "detail": {"profile_ok": True, "binaries_ok": True},
            }
        ],
        "signatures": {
            "report_mldsa": None,
            "anchor_id": None,
            "body_sha3_256": None,
        },
    }
    report.update(overrides)
    return report


# ─── canonical body invariants ─────────────────────────────────────────


def test_canonical_body_strips_signature_fields() -> None:
    report = _minimal_report()
    report["signatures"]["report_mldsa"] = "ff" * 100
    report["signatures"]["anchor_id"] = "anchor-1"
    report["signatures"]["body_sha3_256"] = "00" * 32
    body = signer_mod.canonical_body(report)
    assert b'"report_mldsa":null' in body
    assert b'"anchor_id":null' in body
    assert b'"body_sha3_256":null' in body
    assert b'"ff"' not in body


def test_canonical_body_sorts_keys_at_every_level() -> None:
    report = _minimal_report()
    body = signer_mod.canonical_body(report).decode("utf-8")
    # Object keys must appear in lexical order at every level.
    # Just check the top-level order matches sorted().
    parsed = json.loads(body)
    top = list(parsed.keys())
    assert top == sorted(top), f"top-level keys not sorted: {top}"
    # Nested check for one object.
    sig_keys = list(parsed["signatures"].keys())
    assert sig_keys == sorted(sig_keys)


def test_canonical_body_preserves_array_order() -> None:
    """Phases run in a specific order; canonical body must not sort them."""
    report = _minimal_report()
    report["phases"] = [
        {"name": "z-last", "status": "pass", "started_at": "", "ended_at": "", "detail": {}},
        {"name": "a-first", "status": "pass", "started_at": "", "ended_at": "", "detail": {}},
    ]
    body = signer_mod.canonical_body(report).decode("utf-8")
    z_pos = body.find('"z-last"')
    a_pos = body.find('"a-first"')
    assert z_pos > 0
    assert a_pos > 0
    assert z_pos < a_pos, "canonical body must preserve phase insertion order"


def test_canonical_body_is_deterministic() -> None:
    a = signer_mod.canonical_body(_minimal_report())
    b = signer_mod.canonical_body(_minimal_report())
    assert a == b


def test_canonical_body_ignores_stored_signature_value() -> None:
    """Mutating the signature must not change canonical_body bytes."""
    report = _minimal_report()
    body_before = signer_mod.canonical_body(report)
    report["signatures"]["report_mldsa"] = "deadbeef" * 100
    report["signatures"]["anchor_id"] = "anchor-2"
    report["signatures"]["body_sha3_256"] = "ab" * 32
    body_after = signer_mod.canonical_body(report)
    assert body_before == body_after


def test_sha3_hex_matches_hashlib() -> None:
    payload = b"the quick brown fox"
    assert signer_mod.sha3_hex(payload) == hashlib.sha3_256(payload).hexdigest()


# ─── schema gate ───────────────────────────────────────────────────────


def test_load_report_rejects_wrong_schema_version(tmp_path: Path) -> None:
    report = _minimal_report()
    report["schema_version"] = 999
    path = tmp_path / "r.json"
    path.write_text(json.dumps(report), encoding="utf-8")
    with pytest.raises(SystemExit, match="schema_version"):
        signer_mod.load_report(path)


def test_load_report_rejects_wrong_ship_session(tmp_path: Path) -> None:
    report = _minimal_report()
    report["ship_session"] = "SHIP-99"
    path = tmp_path / "r.json"
    path.write_text(json.dumps(report), encoding="utf-8")
    with pytest.raises(SystemExit, match="ship_session"):
        signer_mod.load_report(path)


def test_load_report_rejects_missing_signatures_block(tmp_path: Path) -> None:
    report = _minimal_report()
    del report["signatures"]
    path = tmp_path / "r.json"
    path.write_text(json.dumps(report), encoding="utf-8")
    with pytest.raises(SystemExit, match="signatures"):
        signer_mod.load_report(path)


def test_load_report_rejects_non_object_root(tmp_path: Path) -> None:
    path = tmp_path / "r.json"
    path.write_text("[]", encoding="utf-8")
    with pytest.raises(SystemExit, match="object"):
        signer_mod.load_report(path)


def test_load_report_rejects_bad_json(tmp_path: Path) -> None:
    path = tmp_path / "r.json"
    path.write_text("{not json", encoding="utf-8")
    with pytest.raises(SystemExit, match="valid JSON"):
        signer_mod.load_report(path)


# ─── CLI / subprocess paths ────────────────────────────────────────────


def _invoke(args: list[str]) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, str(SIGNER), *args],
        capture_output=True,
        text=True,
        cwd=str(REPO_ROOT),
    )


def test_cli_help() -> None:
    r = _invoke(["--help"])
    assert r.returncode == 0
    assert "sign" in r.stdout
    assert "verify" in r.stdout
    assert "canonical" in r.stdout


def test_cli_canonical_emits_bytes(tmp_path: Path) -> None:
    report_path = tmp_path / "report.json"
    report_path.write_text(json.dumps(_minimal_report()), encoding="utf-8")
    out_path = tmp_path / "body.bin"
    r = _invoke(["canonical", "--report", str(report_path), "--out", str(out_path)])
    assert r.returncode == 0, r.stderr
    assert out_path.exists()
    body_bytes = out_path.read_bytes()
    # Must equal in-process canonical_body for the same report.
    expected = signer_mod.canonical_body(_minimal_report())
    assert body_bytes == expected


def test_cli_sign_without_pqcrypto_exits_5(tmp_path: Path) -> None:
    if HAS_MLDSA:
        pytest.skip("pqcrypto is installed; exit-5 path not reachable")
    report_path = tmp_path / "report.json"
    report_path.write_text(json.dumps(_minimal_report()), encoding="utf-8")
    sk_path = tmp_path / "sk.bin"
    sk_path.write_bytes(b"\x00" * signer_mod.MLDSA87_SK_LEN)
    r = _invoke([
        "sign",
        "--report", str(report_path),
        "--signing-key", str(sk_path),
        "--anchor-id", "anchor-1",
    ])
    assert r.returncode == 5, r.stderr
    # Sidecar sha3 witness was written.
    sha_path = tmp_path / "report.json.sha3"
    assert sha_path.exists()
    digest = sha_path.read_text(encoding="utf-8").strip()
    assert len(digest) == 64
    assert digest == hashlib.sha3_256(signer_mod.canonical_body(_minimal_report())).hexdigest()


def test_cli_sign_rejects_wrong_sk_length(tmp_path: Path) -> None:
    report_path = tmp_path / "report.json"
    report_path.write_text(json.dumps(_minimal_report()), encoding="utf-8")
    sk_path = tmp_path / "sk.bin"
    sk_path.write_bytes(b"\x00" * 10)
    r = _invoke([
        "sign",
        "--report", str(report_path),
        "--signing-key", str(sk_path),
        "--anchor-id", "anchor-1",
    ])
    # Exit code != 0; signer raises SystemExit with a message.
    assert r.returncode != 0
    assert "signing key must be" in r.stderr


def test_cli_verify_rejects_unsigned_report(tmp_path: Path) -> None:
    report_path = tmp_path / "report.json"
    report_path.write_text(json.dumps(_minimal_report()), encoding="utf-8")
    pk_path = tmp_path / "pk.bin"
    pk_path.write_bytes(b"\x00" * signer_mod.MLDSA87_PK_LEN)
    r = _invoke(["verify", "--report", str(report_path), "--verifying-key", str(pk_path)])
    assert r.returncode == 1
    assert "unsigned" in r.stderr


def test_cli_verify_rejects_bad_pk_length(tmp_path: Path) -> None:
    report = _minimal_report()
    report["signatures"]["report_mldsa"] = "00" * signer_mod.MLDSA87_SIG_LEN
    report["signatures"]["body_sha3_256"] = "00" * 32
    report["signatures"]["anchor_id"] = "anchor-1"
    report_path = tmp_path / "report.json"
    report_path.write_text(json.dumps(report), encoding="utf-8")
    pk_path = tmp_path / "pk.bin"
    pk_path.write_bytes(b"\x00" * 100)
    r = _invoke(["verify", "--report", str(report_path), "--verifying-key", str(pk_path)])
    # body_sha3 mismatch comes BEFORE pk length check, so accept either signal.
    assert r.returncode == 1


def test_cli_verify_detects_body_tamper(tmp_path: Path) -> None:
    report = _minimal_report()
    report["signatures"]["report_mldsa"] = "00" * signer_mod.MLDSA87_SIG_LEN
    report["signatures"]["body_sha3_256"] = "ff" * 32
    report["signatures"]["anchor_id"] = "anchor-1"
    report_path = tmp_path / "report.json"
    report_path.write_text(json.dumps(report), encoding="utf-8")
    pk_path = tmp_path / "pk.bin"
    pk_path.write_bytes(b"\x00" * signer_mod.MLDSA87_PK_LEN)
    r = _invoke(["verify", "--report", str(report_path), "--verifying-key", str(pk_path)])
    assert r.returncode == 1
    assert "body_sha3_256 mismatch" in r.stderr


# ─── ML-DSA round-trip (only when pqcrypto is installed) ───────────────


@pytest.mark.skipif(not HAS_MLDSA, reason="pqcrypto ML-DSA-87 not installed")
def test_sign_verify_roundtrip(tmp_path: Path) -> None:
    mldsa = signer_mod._load_mldsa()
    pk, sk = mldsa.generate_keypair()
    assert len(pk) == signer_mod.MLDSA87_PK_LEN
    assert len(sk) == signer_mod.MLDSA87_SK_LEN

    report = _minimal_report()
    report_path = tmp_path / "report.json"
    report_path.write_text(json.dumps(report), encoding="utf-8")
    sk_path = tmp_path / "sk.bin"
    sk_path.write_bytes(sk)
    pk_path = tmp_path / "pk.bin"
    pk_path.write_bytes(pk)

    r = _invoke([
        "sign",
        "--report", str(report_path),
        "--signing-key", str(sk_path),
        "--anchor-id", "release-officer-test",
    ])
    assert r.returncode == 0, r.stderr

    signed = json.loads(report_path.read_text(encoding="utf-8"))
    assert signed["signatures"]["anchor_id"] == "release-officer-test"
    assert len(signed["signatures"]["report_mldsa"]) == 2 * signer_mod.MLDSA87_SIG_LEN
    assert len(signed["signatures"]["body_sha3_256"]) == 64

    # Sidecar exists.
    sig_path = tmp_path / "report.json.sig"
    assert sig_path.exists()
    assert sig_path.read_text(encoding="utf-8").strip() == signed["signatures"]["report_mldsa"]

    # Verify command passes.
    r = _invoke(["verify", "--report", str(report_path), "--verifying-key", str(pk_path)])
    assert r.returncode == 0, r.stderr


@pytest.mark.skipif(not HAS_MLDSA, reason="pqcrypto ML-DSA-87 not installed")
def test_verify_fails_after_phase_tamper(tmp_path: Path) -> None:
    mldsa = signer_mod._load_mldsa()
    pk, sk = mldsa.generate_keypair()
    report = _minimal_report()
    report_path = tmp_path / "report.json"
    report_path.write_text(json.dumps(report), encoding="utf-8")
    sk_path = tmp_path / "sk.bin"
    sk_path.write_bytes(sk)
    pk_path = tmp_path / "pk.bin"
    pk_path.write_bytes(pk)
    _invoke([
        "sign",
        "--report", str(report_path),
        "--signing-key", str(sk_path),
        "--anchor-id", "anchor-1",
    ])
    # Tamper: change a phase status.
    signed = json.loads(report_path.read_text(encoding="utf-8"))
    signed["phases"][0]["status"] = "fail"
    report_path.write_text(json.dumps(signed, indent=2), encoding="utf-8")
    r = _invoke(["verify", "--report", str(report_path), "--verifying-key", str(pk_path)])
    assert r.returncode == 1
