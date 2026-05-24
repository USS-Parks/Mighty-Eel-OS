"""SHIP-14 burn-in report signer.

Canonicalises a burn-in-report.json and signs (or verifies) it with
ML-DSA-87. The canonical body matches the SHIP-09 manifest pattern:

    1. Strip the `signatures` block (set every signature field to null).
    2. Re-serialise with sorted object keys at every nesting level,
       no whitespace, arrays preserved in insertion order.
    3. SHA3-256 those bytes for tamper detection.
    4. ML-DSA-87 sign those bytes with the operator's secret key.

The signed report mutates `signatures` in place:

    signatures.report_mldsa  = hex(signature)        # 4627 bytes hex
    signatures.anchor_id     = "release-officer-..."
    signatures.body_sha3_256 = hex(sha3_256(body))

A sidecar `<report>.sig` carries just the hex signature so verifiers
that cannot re-parse JSON can still pin the signature byte-for-byte.

ML-DSA-87 is loaded from the `pqcrypto.sign.ml_dsa_87` module when
available. When the library is missing the script still computes the
canonical body + sha3 + writes the sidecar, but exits with code 5 so
operators know the report is not cryptographically signed. The
canonical-body path is therefore testable without ML-DSA installed.

Usage:
    burn-in-report-sign.py sign   --report PATH --signing-key PATH --anchor-id ID
    burn-in-report-sign.py verify --report PATH --verifying-key PATH
    burn-in-report-sign.py canonical --report PATH [--out PATH]

Exit codes:
    0  success
    1  signature verification failed (verify only)
    2  arguments unreadable
    3  report unreadable or schema mismatch
    4  internal error
    5  ML-DSA-87 library missing (sign only)
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path
from typing import Any

MLDSA87_PK_LEN = 2592
MLDSA87_SK_LEN = 4896
MLDSA87_SIG_LEN = 4627

SCHEMA_VERSION = 1
SHIP_SESSION = "SHIP-14"


# ─── canonicalisation ──────────────────────────────────────────────────


def canonical_body(report: dict[str, Any]) -> bytes:
    """Strip signatures, sort keys at every level, emit deterministic bytes.

    Mirrors mai/tools/mai-admin/src/manifest.rs::canonical_json — the same
    operator should be able to verify either a backup manifest or a burn-in
    report with the same canonicalisation contract.
    """
    copy = json.loads(json.dumps(report))  # deep clone via serialise/parse
    copy["signatures"] = {
        "report_mldsa": None,
        "anchor_id": None,
        "body_sha3_256": None,
    }
    return _canonical_json(copy).encode("utf-8")


def _canonical_json(value: Any) -> str:
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, (int, float)):
        return json.dumps(value)
    if isinstance(value, str):
        return json.dumps(value, ensure_ascii=False)
    if isinstance(value, list):
        return "[" + ",".join(_canonical_json(v) for v in value) + "]"
    if isinstance(value, dict):
        parts = []
        for key in sorted(value.keys()):
            parts.append(json.dumps(key, ensure_ascii=False) + ":" + _canonical_json(value[key]))
        return "{" + ",".join(parts) + "}"
    raise TypeError(f"non-canonical value: {type(value).__name__}")


def sha3_hex(payload: bytes) -> str:
    return hashlib.sha3_256(payload).hexdigest()


# ─── schema gate ───────────────────────────────────────────────────────


def load_report(path: Path) -> dict[str, Any]:
    try:
        raw = path.read_bytes()
    except OSError as exc:
        raise SystemExit(f"burn-in-sign: cannot read report: {exc}") from exc
    try:
        report = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise SystemExit(f"burn-in-sign: report is not valid JSON: {exc}") from exc
    if not isinstance(report, dict):
        raise SystemExit("burn-in-sign: report root must be an object")
    if report.get("schema_version") != SCHEMA_VERSION:
        raise SystemExit(
            f"burn-in-sign: schema_version != {SCHEMA_VERSION} (got {report.get('schema_version')})"
        )
    if report.get("ship_session") != SHIP_SESSION:
        raise SystemExit(
            f"burn-in-sign: ship_session != {SHIP_SESSION} (got {report.get('ship_session')})"
        )
    if "signatures" not in report or not isinstance(report["signatures"], dict):
        raise SystemExit("burn-in-sign: report.signatures missing")
    return report


def write_report(path: Path, report: dict[str, Any]) -> None:
    pretty = json.dumps(report, indent=2)
    path.write_text(pretty + "\n", encoding="utf-8")


# ─── ML-DSA-87 (optional) ──────────────────────────────────────────────


def _load_mldsa() -> Any:
    """Return the pqcrypto ML-DSA-87 module or None when unavailable."""
    try:
        from pqcrypto.sign import ml_dsa_87  # type: ignore[import-not-found]
        return ml_dsa_87
    except Exception:
        return None


def sign_body(secret_key: bytes, body: bytes) -> bytes:
    if len(secret_key) != MLDSA87_SK_LEN:
        raise SystemExit(
            f"burn-in-sign: signing key must be {MLDSA87_SK_LEN} bytes (got {len(secret_key)})"
        )
    mldsa = _load_mldsa()
    if mldsa is None:
        raise SystemExit("burn-in-sign: pqcrypto ML-DSA-87 not installed (exit 5)")
    sig = mldsa.sign(body, secret_key)
    if len(sig) != MLDSA87_SIG_LEN:
        raise SystemExit(
            f"burn-in-sign: signature length != {MLDSA87_SIG_LEN} (got {len(sig)}); pqcrypto version mismatch?"
        )
    return sig


def verify_body(public_key: bytes, body: bytes, signature: bytes) -> bool:
    if len(public_key) != MLDSA87_PK_LEN:
        raise SystemExit(
            f"burn-in-sign: verifying key must be {MLDSA87_PK_LEN} bytes (got {len(public_key)})"
        )
    if len(signature) != MLDSA87_SIG_LEN:
        raise SystemExit(
            f"burn-in-sign: signature length != {MLDSA87_SIG_LEN} (got {len(signature)})"
        )
    mldsa = _load_mldsa()
    if mldsa is None:
        raise SystemExit("burn-in-sign: pqcrypto ML-DSA-87 not installed (exit 5)")
    try:
        recovered = mldsa.open(signature + body, public_key)
        return recovered == body
    except Exception:
        return False


# ─── subcommands ───────────────────────────────────────────────────────


def cmd_canonical(args: argparse.Namespace) -> int:
    report_path = Path(args.report)
    report = load_report(report_path)
    body = canonical_body(report)
    if args.out:
        Path(args.out).write_bytes(body)
    else:
        sys.stdout.buffer.write(body)
    return 0


def cmd_sign(args: argparse.Namespace) -> int:
    report_path = Path(args.report)
    report = load_report(report_path)
    body = canonical_body(report)
    digest = sha3_hex(body)
    try:
        sk = Path(args.signing_key).read_bytes()
    except OSError as exc:
        print(f"burn-in-sign: cannot read signing key: {exc}", file=sys.stderr)
        return 2
    try:
        sig = sign_body(sk, body)
    except SystemExit as exc:
        msg = str(exc)
        if "pqcrypto ML-DSA-87 not installed" in msg:
            # Write sidecar + sha3 anyway so the witness exists.
            sha_path = report_path.with_suffix(report_path.suffix + ".sha3")
            sha_path.write_text(digest + "\n", encoding="utf-8")
            print(msg, file=sys.stderr)
            return 5
        raise
    report["signatures"] = {
        "report_mldsa": sig.hex(),
        "anchor_id": args.anchor_id,
        "body_sha3_256": digest,
    }
    write_report(report_path, report)
    sig_path = report_path.with_suffix(report_path.suffix + ".sig")
    sig_path.write_text(sig.hex() + "\n", encoding="utf-8")
    return 0


def cmd_verify(args: argparse.Namespace) -> int:
    report_path = Path(args.report)
    report = load_report(report_path)
    sigs = report["signatures"]
    sig_hex = sigs.get("report_mldsa")
    stored_sha = sigs.get("body_sha3_256")
    if not sig_hex or not stored_sha:
        print("burn-in-sign: report is unsigned", file=sys.stderr)
        return 1
    body = canonical_body(report)
    actual_sha = sha3_hex(body)
    if actual_sha != stored_sha:
        print(
            f"burn-in-sign: body_sha3_256 mismatch (stored={stored_sha} actual={actual_sha})",
            file=sys.stderr,
        )
        return 1
    try:
        pk = Path(args.verifying_key).read_bytes()
    except OSError as exc:
        print(f"burn-in-sign: cannot read verifying key: {exc}", file=sys.stderr)
        return 2
    sig = bytes.fromhex(sig_hex)
    if not verify_body(pk, body, sig):
        print("burn-in-sign: signature verification failed", file=sys.stderr)
        return 1
    print(f"burn-in-sign: signature OK (anchor_id={sigs.get('anchor_id')})")
    return 0


# ─── arg parser ────────────────────────────────────────────────────────


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="burn-in-report-sign.py",
        description="Canonicalise / sign / verify SHIP-14 burn-in reports.",
    )
    sub = parser.add_subparsers(dest="command", required=True)

    sp = sub.add_parser("sign", help="sign report in place")
    sp.add_argument("--report", required=True)
    sp.add_argument("--signing-key", required=True)
    sp.add_argument("--anchor-id", required=True)
    sp.set_defaults(func=cmd_sign)

    vp = sub.add_parser("verify", help="verify report signature")
    vp.add_argument("--report", required=True)
    vp.add_argument("--verifying-key", required=True)
    vp.set_defaults(func=cmd_verify)

    cp = sub.add_parser("canonical", help="emit canonical body to stdout or file")
    cp.add_argument("--report", required=True)
    cp.add_argument("--out", default=None)
    cp.set_defaults(func=cmd_canonical)

    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        return args.func(args)
    except SystemExit:
        raise
    except Exception as exc:
        print(f"burn-in-sign: internal error: {exc}", file=sys.stderr)
        return 4


if __name__ == "__main__":
    sys.exit(main())
