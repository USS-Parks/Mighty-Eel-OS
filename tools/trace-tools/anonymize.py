"""Anonymize a raw MAI trace into a form safe for sharing with the simulator.

The capture module already hashes session ids at write time. This tool adds a
second hash with a fresh per-run salt so that no shared trace can be correlated
back to the original deployment, and strips any fields outside the documented
schema.

Usage:
    python anonymize.py <input.ndjson> <output.ndjson> [--salt SALT]

If `--salt` is omitted, a random 32-byte hex string is generated for the run.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import secrets
import sys
from pathlib import Path

ALLOWED_FIELDS = frozenset(
    {
        "timestamp",
        "request_id",
        "session_id_hash",
        "model_alias",
        "input_tokens",
        "output_tokens",
        "latency_ms",
        "queue_wait_ms",
        "priority",
        "was_continuation",
    }
)


def rehash(value: str, salt: str) -> str:
    """Re-hash a session id hash with a fresh salt. Returns 32 hex chars."""
    return hashlib.blake2b(
        f"{salt}|{value}".encode(), digest_size=16
    ).hexdigest()


def anonymize_event(event: dict, salt: str) -> dict:
    """Project an event onto the allowed schema and re-hash the session id."""
    out = {}
    for field in ALLOWED_FIELDS:
        if field not in event:
            continue
        out[field] = event[field]
    if "session_id_hash" in out:
        out["session_id_hash"] = rehash(str(out["session_id_hash"]), salt)
    return out


def validate(event: dict) -> None:
    extras = set(event.keys()) - ALLOWED_FIELDS
    if extras:
        raise ValueError(f"event has disallowed fields: {sorted(extras)}")


def process(input_path: Path, output_path: Path, salt: str) -> int:
    count = 0
    with input_path.open("r", encoding="utf-8") as src, output_path.open(
        "w", encoding="utf-8"
    ) as dst:
        for line_no, line in enumerate(src, start=1):
            line = line.strip()
            if not line:
                continue
            try:
                event = json.loads(line)
            except json.JSONDecodeError as exc:
                raise ValueError(f"line {line_no}: invalid JSON: {exc}") from exc
            anonymized = anonymize_event(event, salt)
            validate(anonymized)
            dst.write(json.dumps(anonymized, sort_keys=True))
            dst.write("\n")
            count += 1
    return count


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Anonymize a MAI scheduler trace for simulator replay."
    )
    parser.add_argument("input", type=Path, help="Input raw trace NDJSON file.")
    parser.add_argument("output", type=Path, help="Output anonymized trace path.")
    parser.add_argument(
        "--salt",
        default=None,
        help="Optional anonymization salt. Random if omitted.",
    )
    args = parser.parse_args(argv)

    if not args.input.exists():
        print(f"error: input {args.input} does not exist", file=sys.stderr)
        return 2

    salt = args.salt or secrets.token_hex(32)
    count = process(args.input, args.output, salt)
    print(f"anonymized {count} events with salt prefix {salt[:8]}...")
    return 0


if __name__ == "__main__":
    sys.exit(main())
