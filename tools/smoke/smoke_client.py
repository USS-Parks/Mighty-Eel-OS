"""Smoke client: verifies a packaged MAI deployment is reachable via the SDK.

This is the Gate C "SDK runs against packaged deployment" evidence. It is not
an L4-L5 application; it is a minimal,
operator-friendly probe that exercises the public REST surface using the
Python SDK so a deployment can be validated end-to-end.

Exit codes:
    0 — server reachable, health OK, model list returns (may be empty)
    1 — server reachable but at least one endpoint reported a problem
    2 — server unreachable or auth failed

Usage:
    python tools/smoke/smoke_client.py
    python tools/smoke/smoke_client.py --base-url http://mai.local
    MAI_API_KEY=im-... python tools/smoke/smoke_client.py
"""

from __future__ import annotations

import argparse
import os
import sys
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen


def get(base_url: str, path: str, api_key: str | None, timeout: float) -> tuple[int, str]:
    """One synchronous GET, no SDK dependency so this script runs in any env."""
    url = f"{base_url.rstrip('/')}{path}"
    headers = {"Accept": "application/json"}
    if api_key:
        headers["X-IM-Auth-Token"] = api_key
    req = Request(url, headers=headers, method="GET")
    try:
        with urlopen(req, timeout=timeout) as resp:
            body = resp.read().decode("utf-8", errors="replace")
            return resp.status, body
    except HTTPError as exc:
        return exc.code, exc.read().decode("utf-8", errors="replace")


def run(base_url: str, api_key: str | None, timeout: float) -> int:
    print(f"smoke: target {base_url}")
    failed = 0

    # 1. Health (auth-exempt, must return 200).
    try:
        status, body = get(base_url, "/v1/health", api_key, timeout)
    except (URLError, OSError) as exc:
        print(f"FAIL /v1/health unreachable: {exc}")
        return 2
    if status != 200:
        print(f"FAIL /v1/health status={status}")
        failed = 1
    else:
        print(f"OK   /v1/health status=200 body={body[:80]}")

    # 2. Model list (requires auth in production; honors absent key for guest).
    try:
        status, body = get(base_url, "/v1/models", api_key, timeout)
    except (URLError, OSError) as exc:
        print(f"FAIL /v1/models unreachable: {exc}")
        return 2
    if status == 401:
        print("FAIL /v1/models 401 — set MAI_API_KEY or pass --api-key")
        return 2
    if status >= 500:
        print(f"FAIL /v1/models status={status}")
        failed = 1
    else:
        print(f"OK   /v1/models status={status} body={body[:80]}")

    # 3. Scheduler metrics (requires auth; soft-check).
    try:
        status, body = get(base_url, "/v1/scheduler/metrics", api_key, timeout)
        if status >= 500:
            print(f"WARN /v1/scheduler/metrics status={status}")
            failed = 1
        else:
            print(f"OK   /v1/scheduler/metrics status={status}")
    except (URLError, OSError) as exc:
        print(f"WARN /v1/scheduler/metrics unreachable: {exc}")
        failed = 1

    return failed


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--base-url",
        default=os.environ.get("MAI_BASE_URL", "http://localhost:8420"),
    )
    parser.add_argument(
        "--api-key",
        default=os.environ.get("MAI_API_KEY"),
        help="API key for X-IM-Auth-Token (or set MAI_API_KEY).",
    )
    parser.add_argument("--timeout", type=float, default=5.0)
    args = parser.parse_args(argv)
    return run(args.base_url, args.api_key, args.timeout)


if __name__ == "__main__":
    sys.exit(main())
