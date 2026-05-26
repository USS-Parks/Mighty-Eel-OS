"""End-to-end compliance smoke (W5 / J-10).

Spawns the real `mai-api` binary as a subprocess, exercises the
compliance code path that the dashboard and the L4-L5 application
scaffolds rely on (status, policy template apply, audit chain query,
chain verify, report generation), and tears the process down cleanly.

This is the first Python-side e2e in the tree. It exists because John
Dougherty's GitDoctor scan flagged TST-005 (no e2e tests) and J-09
exposed mock-only adapter tests as performative. The contract here is
the opposite: every byte of this file's HTTP traffic hits a real Rust
binary, no in-process router, no `axum::test::oneshot`, no mocked
state.

Skip semantics: the binary is not built by this test. When
`mai/target/release/lamprey-mai-api(.exe)` (or the debug equivalent)
is absent, the entire module is skipped with a clear message
pointing the developer at `cargo build --release -p mai-api`. CI
invokes that build before pytest. (Package name kept as `mai-api`;
BRAND-01 only renamed the produced binary file.)

Auth: the binary is started against a temp working directory whose
`config/auth_keys.toml` sets `allow_internal_profile_header = true`,
so the smoke can authenticate via the `X-IM-Profile` header without
provisioning a real API key. The flag is the documented dev/test path
(see auth.rs:78). Production deployments never set it.
"""

from __future__ import annotations

import json
import os
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request
import uuid
from collections.abc import Iterator
from contextlib import suppress
from pathlib import Path
from typing import Any

import pytest

pytestmark = pytest.mark.e2e

REPO_ROOT = Path(__file__).resolve().parents[2]
PROFILE_HEADER = "X-IM-Profile"
ADMIN_PROFILE = "admin:Admin"
# Debug builds can take longer to warm up (registry, vault stubs, etc.).
STARTUP_TIMEOUT_S = 90.0


def _find_binary() -> Path | None:
    """Return the path to a built lamprey-mai-api binary, or None if
    absent. BRAND-01 renamed the cargo bin from mai-api; the package
    name (`-p mai-api` in cargo) is unchanged.
    """
    ext = ".exe" if os.name == "nt" else ""
    for variant in ("release", "debug"):
        candidate = REPO_ROOT / "target" / variant / f"lamprey-mai-api{ext}"
        if candidate.is_file():
            return candidate
    return None


def _free_port() -> int:
    """Ask the OS for a free localhost port; release it before returning."""
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return int(s.getsockname()[1])


def _wait_for_live(port: int, timeout_s: float) -> None:
    """Poll /v1/health/live until 200, or raise TimeoutError."""
    deadline = time.monotonic() + timeout_s
    last_err: BaseException | None = None
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(
                f"http://127.0.0.1:{port}/v1/health/live", timeout=1.0,
            ) as resp:
                if resp.status == 200:
                    return
        except (urllib.error.URLError, ConnectionError, OSError) as e:
            last_err = e
        time.sleep(0.1)
    raise TimeoutError(
        f"mai-api did not respond on /v1/health/live within {timeout_s}s "
        f"(last error: {last_err!r})",
    )


def _request(
    port: int, method: str, path: str, body: dict[str, Any] | None = None,
) -> tuple[int, dict[str, Any]]:
    """Issue a request with admin profile. Returns (status, parsed JSON)."""
    headers = {PROFILE_HEADER: ADMIN_PROFILE}
    data: bytes | None = None
    if body is not None:
        data = json.dumps(body).encode("utf-8")
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(
        f"http://127.0.0.1:{port}{path}",
        data=data,
        headers=headers,
        method=method,
    )
    try:
        with urllib.request.urlopen(req, timeout=5.0) as r:
            return r.status, json.loads(r.read().decode("utf-8") or "null")
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read().decode("utf-8") or "null")


@pytest.fixture(scope="module")
def running_server() -> Iterator[int]:
    """Spawn mai-api in a temp working directory; yield the REST port."""
    binary = _find_binary()
    if binary is None:
        pytest.skip(
            "lamprey-mai-api binary not built. Run "
            "`cargo build --release -p mai-api` before invoking this e2e.",
        )

    rest_port = _free_port()
    grpc_port = _free_port()

    # Avoid system temp directories (may be ACL-restricted in some Windows setups).
    tmp = REPO_ROOT / "py_tmp_dir" / f"mai-e2e-{uuid.uuid4().hex}"
    tmp.mkdir(parents=True, exist_ok=False)
    config_dir = tmp / "config"
    config_dir.mkdir()

    # `allow_internal_profile_header = true` enables the X-IM-Profile
    # bypass used by every assertion below. This is the dev/test
    # posture; production profiles set it to false and never accept
    # this header (see auth.rs and SHIP-17).
    (config_dir / "auth_keys.toml").write_text(
        "[settings]\n"
        "allow_internal_profile_header = true\n"
        "rate_limit_per_minute = 600\n",
        encoding="utf-8",
    )

    server_toml = tmp / "server.toml"
    server_toml.write_text(
        f'[server]\n'
        f'port = {rest_port}\n'
        f'grpc_port = {grpc_port}\n'
        f'bind_address = "127.0.0.1"\n',
        encoding="utf-8",
    )

    env = {**os.environ, "RUST_LOG": "mai_api=warn,warn"}
    # Both arguments are paths we control: `binary` comes from
    # _find_binary() (only target/release/ or target/debug/), and
    # `server_toml` is the file we just wrote inside `tmp`. No
    # caller-supplied input reaches this call.
    log_path = tmp / "mai-api.stdout.log"
    log_fh = log_path.open("wb")
    proc = subprocess.Popen(
        [str(binary), str(server_toml)],
        cwd=str(tmp),
        env=env,
        stdout=log_fh,
        stderr=subprocess.STDOUT,
    )

    try:
        _wait_for_live(rest_port, timeout_s=STARTUP_TIMEOUT_S)
        yield rest_port
    except BaseException:
        # If startup fails or a test raises, capture the binary's
        # stdout so the failure message tells us why.
        with suppress(Exception):
            if log_path.is_file():
                tail = log_path.read_text(errors="replace")[-2000:]
                sys.stderr.write(f"\nmai-api stdout tail:\n{tail}\n")
        raise
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=5)
        with suppress(Exception):
            log_fh.close()


def test_health_live_returns_status_live(running_server: int) -> None:
    """The cheapest readiness probe returns 200 with `status: live`."""
    status, body = _request(running_server, "GET", "/v1/health/live")
    assert status == 200
    assert body["status"] == "live"
    assert "reasons" not in body or body["reasons"] == []


def test_compliance_status_exposes_audit_integrity(running_server: int) -> None:
    """`/v1/compliance/status` returns module + audit snapshot."""
    status, body = _request(running_server, "GET", "/v1/compliance/status")
    assert status == 200
    assert isinstance(body["modules"], list)
    assert isinstance(body["priority"], list)
    integrity = body["audit_integrity"]
    assert "entry_count" in integrity
    assert "head_hash" in integrity
    assert integrity["last_verify"] in {"unknown", "verified"}


def test_audit_chain_verifies_on_fresh_boot(running_server: int) -> None:
    """An empty (or freshly-bootstrapped) chain must verify."""
    status, body = _request(running_server, "GET", "/v1/compliance/audit/verify")
    assert status == 200
    assert body["verified"] is True
    assert body.get("error") is None


def test_apply_healthcare_template_succeeds(running_server: int) -> None:
    """Applying a named policy template returns 200 + echoes the name."""
    status, body = _request(
        running_server, "POST", "/v1/compliance/policies/template",
        body={"template": "healthcare"},
    )
    assert status == 200
    assert body.get("template") == "healthcare" or body.get("name") == "healthcare"


@pytest.mark.parametrize(
    ("method", "path"),
    [
        ("GET", "/v1/health/ready"),
        ("GET", "/v1/health/system"),
        ("GET", "/v1/compliance/status"),
        ("GET", "/v1/compliance/audit/verify"),
    ],
)
def test_core_endpoints_return_json_objects(
    running_server: int, method: str, path: str,
) -> None:
    status, body = _request(running_server, method, path)
    assert status == 200
    assert isinstance(body, dict)


def test_missing_profile_header_is_rejected(running_server: int) -> None:
    """A route that requires auth must reject requests with no auth headers."""
    port = running_server
    req = urllib.request.Request(
        f"http://127.0.0.1:{port}/v1/audit/log",
        method="GET",
    )
    with pytest.raises(urllib.error.HTTPError) as err:
        urllib.request.urlopen(req, timeout=5.0)
    assert err.value.code in {401, 403}


def test_audit_chain_still_verifies_after_template_apply(
    running_server: int,
) -> None:
    """Whatever audit entries the template apply produced, the chain
    must still verify. This is the real invariant: policy changes can
    advance the head, but they cannot corrupt the hash chain."""
    status, body = _request(
        running_server, "GET", "/v1/compliance/audit/verify",
    )
    assert status == 200
    assert body["verified"] is True
    assert body.get("error") is None


def test_generate_hipaa_report_synchronously(running_server: int) -> None:
    """`POST /v1/compliance/reports/generate` returns a complete record."""
    status, body = _request(
        running_server, "POST", "/v1/compliance/reports/generate",
        body={
            "report_type": "hipaa",
            "from_unix_nanos": 0,
            "to_unix_nanos": 2_000_000_000_000_000_000,
        },
    )
    assert status == 200
    assert isinstance(body.get("id"), str)
    assert body["id"]
    assert body["report_type"] in {"hipaa_audit_trail", "hipaa"}
    assert body["status"] in {"complete", "completed", "ready"}


def test_guest_blocked_from_view_audit_routes(running_server: int) -> None:
    """A request with no profile header degrades to the Guest role
    (under `allow_internal_profile_header = true`) and the audit-query
    route enforces `view_audit` — so the request must come back 403
    Forbidden, not 200.

    This pins the permission gate for the routes that carry regulated
    data. `/v1/compliance/status` is intentionally readable by Guest
    (it returns module health, not audit content); routes that touch
    the audit chain itself are not.
    """
    req = urllib.request.Request(
        f"http://127.0.0.1:{running_server}/v1/compliance/audit?limit=1",
        method="GET",
    )
    try:
        with urllib.request.urlopen(req, timeout=5.0) as r:
            actual_status = r.status
            body = json.loads(r.read().decode("utf-8") or "null")
    except urllib.error.HTTPError as e:
        actual_status = e.code
        body = json.loads(e.read().decode("utf-8") or "null")
    assert actual_status == 403
    assert body["error"]["code"] == "MAI-4001"
    assert body["error"]["type"] == "auth_error"
