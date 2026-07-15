#!/usr/bin/env python3
"""Validate the Lamprey Saddle finding register against frozen raw evidence."""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
REGISTER = ROOT / "docs/scans/LAMPREY-SADDLE-HARDENING-FINDINGS.json"
EVIDENCE = ROOT / "test-evidence/lamprey-saddle-hardening/M0/source-scan"
PROMPT = re.compile(r"^LSH-(?:[A-Z]+\d+|\d+)$")


def fail(message: str) -> None:
    print(f"lamprey-finding-register: FAIL — {message}", file=sys.stderr)
    raise SystemExit(1)


def raw_candidate_ids() -> list[str]:
    ids: list[str] = []
    review_root = EVIDENCE / "file-reviews"
    for name in ("raw_candidates.jsonl", "candidates.jsonl"):
        for path in sorted(review_root.rglob(name)):
            for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
                if not line.strip():
                    continue
                try:
                    row = json.loads(line)
                except json.JSONDecodeError as error:
                    fail(f"{path.relative_to(ROOT)}:{line_number}: {error}")
                candidate_id = row.get("candidate_id")
                if not isinstance(candidate_id, str) or not candidate_id:
                    fail(f"{path.relative_to(ROOT)}:{line_number}: missing candidate_id")
                ids.append(candidate_id)
    return ids


def main() -> None:
    data = json.loads(REGISTER.read_text(encoding="utf-8"))
    confirmed = data.get("confirmed", [])
    deferred = data.get("deferred", [])
    totals = data.get("totals", {})

    family_ids = [row.get("id") for row in [*confirmed, *deferred]]
    if len(family_ids) != len(set(family_ids)):
        fail("duplicate family id")
    if len(confirmed) != totals.get("confirmed_families"):
        fail("confirmed family total does not match register rows")
    if len(deferred) != totals.get("deferred_families"):
        fail("deferred family total does not match register rows")

    confirmed_instances = [item for row in confirmed for item in row.get("instances", [])]
    deferred_instances = [item for row in deferred for item in row.get("instances", [])]
    registered = [*confirmed_instances, *deferred_instances]

    if len(confirmed_instances) != totals.get("validated_or_reportable"):
        fail("validated/reportable instance total does not match")
    if len(deferred_instances) != totals.get("deferred"):
        fail("deferred instance total does not match")
    if len(registered) != totals.get("raw_instances"):
        fail("raw instance total does not match")
    if len(registered) != len(set(registered)):
        fail("a raw instance is owned by more than one family")

    raw = raw_candidate_ids()
    if len(raw) != len(set(raw)):
        fail("frozen raw evidence contains duplicate candidate ids")
    if set(raw) != set(registered):
        missing = sorted(set(raw) - set(registered))
        unknown = sorted(set(registered) - set(raw))
        fail(f"raw/register mismatch; missing={missing}, unknown={unknown}")

    ledger_count = len(list((EVIDENCE / "candidate-ledgers").rglob("candidate_ledger.jsonl")))
    if ledger_count != totals.get("raw_instances"):
        fail(f"candidate-ledger count {ledger_count} != {totals.get('raw_instances')}")

    for row in [*confirmed, *deferred]:
        prompts = row.get("prompts", [])
        if not prompts or any(not isinstance(prompt, str) or not PROMPT.match(prompt) for prompt in prompts):
            fail(f"{row.get('id')}: invalid or empty prompt ownership")
        if row in confirmed and not row.get("regression"):
            fail(f"{row.get('id')}: confirmed family lacks regression id")
        if row in deferred and not row.get("reachability_question"):
            fail(f"{row.get('id')}: deferred family lacks reachability question")

    print(
        "lamprey-finding-register: OK — "
        f"{len(registered)} raw instances, {len(confirmed)} confirmed families, "
        f"{len(deferred)} deferred families, {ledger_count} candidate ledgers"
    )


if __name__ == "__main__":
    main()
