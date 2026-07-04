"""Sync API namespaces for ``MaiClient``.

Each namespace class wraps a thin slice of the REST API (models,
power, scheduler, …) and is instantiated once per client. They
delegate all transport to the client's ``_request_with_retry`` so
retry/auth/error mapping stays in one place.

Async equivalents live in :mod:`mai.async_client` (AsyncMaiClient
attaches its own namespace instances bound to its async transport).
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from mai.errors import MaiError
from mai.types import (
    AirgapStatusResponse,
    AuditLogResponse,
    AuditQueryResponse,
    BenchmarkResult,
    ComplianceReport,
    ComplianceReportList,
    ComplianceStatus,
    ExchangeTokenResponse,
    HardwareHealthResponse,
    InstanceHealthResponse,
    InstanceMetricsResponse,
    ModelDetail,
    ModelDiscoverResponse,
    ModelInstallResponse,
    ModelLoadResponse,
    ModelObject,
    ModelRemoveResponse,
    ModelUnloadResponse,
    PowerStateResponse,
    PowerTransitionRequest,
    PowerTransitionResponse,
    ProfileObject,
    RevocationStatusResponse,
    SchedulerAnomaliesResponse,
    SchedulerMetricsResponse,
    SystemHealthResponse,
    TrustBundleStatus,
    TrustClaimsResponse,
    TrustStatusResponse,
    UpdateCheckResponse,
    UpdateStatusResponse,
)

if TYPE_CHECKING:
    from mai.client import MaiClient


# ---------------------------------------------------------------------------
# Trust namespace error
# ---------------------------------------------------------------------------

class TrustNotProvisionedError(MaiError):
    """Raised when a trust API is called but the server has no trust bridge.

    The SDK trust surface currently ships as a stub; a later build wires
    the real OpenBao Trust Manifold backend. Applications that catch this
    can fall back to API-key auth.
    """


_TRUST_STUB_MESSAGE = (
    "trust API is not provisioned in this build. "
    "Configure a Trust Manifold backend or use API-key auth."
)


# ---------------------------------------------------------------------------
# Models
# ---------------------------------------------------------------------------

class Models:
    """Model management namespace (``client.models``)."""

    def __init__(self, client: MaiClient) -> None:
        self._client = client

    def list(self, **filters: Any) -> list[ModelObject]:
        """GET /v1/models."""
        resp = self._client._request_with_retry("GET", "/models", params=filters)
        data = resp.json()
        return [ModelObject.model_validate(m) for m in data.get("data", [])]

    def get(self, model_id: str) -> ModelDetail:
        """GET /v1/models/{model_id}."""
        resp = self._client._request_with_retry("GET", f"/models/{model_id}")
        return ModelDetail.model_validate(resp.json())

    def load(self, model_id: str) -> ModelLoadResponse:
        """POST /v1/models/{model_id}/load."""
        resp = self._client._request_with_retry("POST", f"/models/{model_id}/load")
        return ModelLoadResponse.model_validate(resp.json())

    def unload(self, model_id: str) -> ModelUnloadResponse:
        """POST /v1/models/{model_id}/unload."""
        resp = self._client._request_with_retry("POST", f"/models/{model_id}/unload")
        return ModelUnloadResponse.model_validate(resp.json())

    def benchmark(self, model_id: str, **opts: Any) -> BenchmarkResult:
        """POST /v1/models/{model_id}/benchmark — kick off / return result."""
        resp = self._client._request_with_retry(
            "POST", f"/models/{model_id}/benchmark", json=opts or None,
        )
        return BenchmarkResult.model_validate(resp.json())

    def get_benchmark(self, model_id: str) -> BenchmarkResult:
        """GET /v1/models/{model_id}/benchmark — most recent benchmark."""
        resp = self._client._request_with_retry(
            "GET", f"/models/{model_id}/benchmark",
        )
        return BenchmarkResult.model_validate(resp.json())

    def discover(self, path: str | None = None) -> ModelDiscoverResponse:
        """POST /v1/models/discover — scan a path for installable packages."""
        body = {"path": path} if path else {}
        resp = self._client._request_with_retry(
            "POST", "/models/discover", json=body,
        )
        return ModelDiscoverResponse.model_validate(resp.json())

    def install(
        self, package_bytes: bytes, *, filename: str = "package.mpkg",
    ) -> ModelInstallResponse:
        """POST /v1/models/install — upload a package (multipart)."""
        files = {"package": (filename, package_bytes, "application/octet-stream")}
        resp = self._client._request_with_retry(
            "POST", "/models/install", files=files,
        )
        return ModelInstallResponse.model_validate(resp.json())

    def remove(self, model_id: str) -> ModelRemoveResponse:
        """POST /v1/models/{model_id}/remove (DELETE also accepted)."""
        resp = self._client._request_with_retry(
            "POST", f"/models/{model_id}/remove",
        )
        return ModelRemoveResponse.model_validate(resp.json())


# ---------------------------------------------------------------------------
# Power
# ---------------------------------------------------------------------------

class Power:
    """Power management namespace (``client.power``)."""

    def __init__(self, client: MaiClient) -> None:
        self._client = client

    def get_state(self) -> PowerStateResponse:
        """GET /v1/power/state."""
        resp = self._client._request_with_retry("GET", "/power/state")
        return PowerStateResponse.model_validate(resp.json())

    def transition(
        self, request: PowerTransitionRequest,
    ) -> PowerTransitionResponse:
        """POST /v1/power/transition."""
        resp = self._client._request_with_retry(
            "POST", "/power/transition", json=request.model_dump(),
        )
        return PowerTransitionResponse.model_validate(resp.json())


# ---------------------------------------------------------------------------
# System
# ---------------------------------------------------------------------------

class System:
    """System status namespace (``client.system``)."""

    def __init__(self, client: MaiClient) -> None:
        self._client = client

    def airgap(self) -> AirgapStatusResponse:
        """GET /v1/system/airgap."""
        resp = self._client._request_with_retry("GET", "/system/airgap")
        return AirgapStatusResponse.model_validate(resp.json())

    def system_health(self) -> SystemHealthResponse:
        """GET /v1/health/system."""
        resp = self._client._request_with_retry("GET", "/health/system")
        return SystemHealthResponse.model_validate(resp.json())

    def hardware_health(self) -> HardwareHealthResponse:
        """GET /v1/health/hardware."""
        resp = self._client._request_with_retry("GET", "/health/hardware")
        return HardwareHealthResponse.model_validate(resp.json())


# ---------------------------------------------------------------------------
# Scheduler / telemetry
# ---------------------------------------------------------------------------

class Scheduler:
    """Scheduler metrics namespace (``client.scheduler``)."""

    def __init__(self, client: MaiClient) -> None:
        self._client = client

    def metrics(self) -> SchedulerMetricsResponse:
        """GET /v1/scheduler/metrics."""
        resp = self._client._request_with_retry("GET", "/scheduler/metrics")
        return SchedulerMetricsResponse.model_validate(resp.json())

    def instance_metrics(self, instance_id: str) -> InstanceMetricsResponse:
        """GET /v1/scheduler/instances/{id}/metrics."""
        resp = self._client._request_with_retry(
            "GET", f"/scheduler/instances/{instance_id}/metrics",
        )
        return InstanceMetricsResponse.model_validate(resp.json())

    def instance_health(self, instance_id: str) -> InstanceHealthResponse:
        """GET /v1/scheduler/instances/{id}/health."""
        resp = self._client._request_with_retry(
            "GET", f"/scheduler/instances/{instance_id}/health",
        )
        return InstanceHealthResponse.model_validate(resp.json())

    def anomalies(self) -> SchedulerAnomaliesResponse:
        """GET /v1/scheduler/anomalies."""
        resp = self._client._request_with_retry("GET", "/scheduler/anomalies")
        return SchedulerAnomaliesResponse.model_validate(resp.json())


# ---------------------------------------------------------------------------
# Updates (OTA)
# ---------------------------------------------------------------------------

class Updates:
    """Update channel namespace (``client.updates``)."""

    def __init__(self, client: MaiClient) -> None:
        self._client = client

    def check(self) -> UpdateCheckResponse:
        """GET /v1/updates/check."""
        resp = self._client._request_with_retry("GET", "/updates/check")
        return UpdateCheckResponse.model_validate(resp.json())

    def download(self, component: str, target_version: str) -> dict[str, Any]:
        """POST /v1/updates/download."""
        resp = self._client._request_with_retry(
            "POST", "/updates/download",
            json={"component": component, "target_version": target_version},
        )
        return resp.json()  # type: ignore[no-any-return]

    def status(self) -> UpdateStatusResponse:
        """GET /v1/updates/status."""
        resp = self._client._request_with_retry("GET", "/updates/status")
        return UpdateStatusResponse.model_validate(resp.json())


# ---------------------------------------------------------------------------
# Admin
# ---------------------------------------------------------------------------

class Admin:
    """Admin operations namespace (``client.admin``).

    All methods require admin-class credentials. The server enforces
    permissions; failures surface as ``PermissionError``.
    """

    def __init__(self, client: MaiClient) -> None:
        self._client = client

    def list_profiles(self) -> list[ProfileObject]:
        """GET /v1/profiles."""
        resp = self._client._request_with_retry("GET", "/profiles")
        data = resp.json()
        return [ProfileObject.model_validate(p) for p in data.get("data", [])]

    def get_profile(self, profile_id: str) -> ProfileObject:
        """GET /v1/profiles/{profile_id}."""
        resp = self._client._request_with_retry("GET", f"/profiles/{profile_id}")
        return ProfileObject.model_validate(resp.json())

    def audit_log(
        self,
        *,
        offset: int = 0,
        limit: int = 100,
        since_unix: int | None = None,
    ) -> AuditLogResponse:
        """GET /v1/audit/log."""
        params: dict[str, Any] = {"offset": offset, "limit": limit}
        if since_unix is not None:
            params["since"] = since_unix
        resp = self._client._request_with_retry(
            "GET", "/audit/log", params=params,
        )
        return AuditLogResponse.model_validate(resp.json())

    def adapters(self) -> dict[str, Any]:
        """GET /v1/adapters — raw adapter inventory."""
        resp = self._client._request_with_retry("GET", "/adapters")
        return resp.json()  # type: ignore[no-any-return]

    def registry(self) -> dict[str, Any]:
        """GET /v1/registry — raw registry manifest."""
        resp = self._client._request_with_retry("GET", "/registry")
        return resp.json()  # type: ignore[no-any-return]

    def registry_scan(self) -> dict[str, Any]:
        """POST /v1/registry/scan — trigger a rescan."""
        resp = self._client._request_with_retry("POST", "/registry/scan")
        return resp.json()  # type: ignore[no-any-return]


# ---------------------------------------------------------------------------
# Auth
# ---------------------------------------------------------------------------

class Auth:
    """Auth/token operations (``client.auth``).

    Implements ``POST /v1/auth/exchange_token`` against the local-dev
    stub. The token is opaque to consumers — pass it back to the
    server as part of the request envelope; the real OpenBao-backed
    exchange replaces the body of the server handler without changing
    the wire shape.
    """

    def __init__(self, client: MaiClient) -> None:
        self._client = client

    def exchange_token(
        self,
        subject_id: str,
        *,
        tenant_id: str | None = None,
        scopes: list[str] | None = None,
    ) -> ExchangeTokenResponse:
        """POST /v1/auth/exchange_token — mint a short-lived access token."""
        body: dict[str, Any] = {"subject_id": subject_id}
        if tenant_id is not None:
            body["tenant_id"] = tenant_id
        if scopes is not None:
            body["scopes"] = scopes
        resp = self._client._request_with_retry("POST", "/auth/exchange_token", json=body)
        return ExchangeTokenResponse.model_validate(resp.json())


# ---------------------------------------------------------------------------
# Trust
# ---------------------------------------------------------------------------

class Trust:
    """Trust Manifold namespace (``client.trust``).

    Reads the local trust cache via the server endpoints. Every
    method is metadata-only — no prompt, completion, embedding, or
    regulated payload travels through this surface (Trust Manifold
    hard rule §A.2.4).
    """

    def __init__(self, client: MaiClient) -> None:
        self._client = client

    def status(self) -> TrustStatusResponse:
        """GET /v1/trust/status — consolidated trust mode."""
        resp = self._client._request_with_retry("GET", "/trust/status")
        return TrustStatusResponse.model_validate(resp.json())

    def claims(self) -> TrustClaimsResponse:
        """GET /v1/trust/claims — every claim held in the local cache."""
        resp = self._client._request_with_retry("GET", "/trust/claims")
        return TrustClaimsResponse.model_validate(resp.json())

    def bundle_status(self) -> TrustBundleStatus:
        """GET /v1/trust/bundle_status — bundle version + freshness."""
        resp = self._client._request_with_retry("GET", "/trust/bundle_status")
        return TrustBundleStatus.model_validate(resp.json())

    def revocation_status(self, claim_id: str) -> RevocationStatusResponse:
        """GET /v1/trust/revocation_status — per-claim snapshot lookup."""
        resp = self._client._request_with_retry(
            "GET", "/trust/revocation_status", params={"claim_id": claim_id},
        )
        return RevocationStatusResponse.model_validate(resp.json())


# ---------------------------------------------------------------------------
# Compliance
# ---------------------------------------------------------------------------

class Compliance:
    """Lamprey compliance namespace (``client.compliance``).

    Wires the PolicyManager, AuditLog, and ReportManager
    onto the SDK. Mirrors the dashboard surface so SDK callers can
    do everything the dashboard can do programmatically.
    """

    def __init__(self, client: MaiClient) -> None:
        self._client = client

    # --- status / policy ----------------------------------------------

    def get_status(self) -> ComplianceStatus:
        """GET /v1/compliance/status — composer + module + audit snapshot."""
        resp = self._client._request_with_retry("GET", "/compliance/status")
        return ComplianceStatus.model_validate(resp.json())

    def get_policies(self) -> list[dict[str, Any]]:
        """GET /v1/compliance/policies — per-module status rows."""
        resp = self._client._request_with_retry("GET", "/compliance/policies")
        data = resp.json()
        return list(data.get("modules", []))

    def get_policy(self, module: str) -> dict[str, Any]:
        """GET /v1/compliance/policies/{module} — single module status."""
        resp = self._client._request_with_retry("GET", f"/compliance/policies/{module}")
        return resp.json()  # type: ignore[no-any-return]

    def update_policy(self, module: str, *, enabled: bool) -> dict[str, Any]:
        """PUT /v1/compliance/policies/{module} — flip a module on/off."""
        resp = self._client._request_with_retry(
            "PUT", f"/compliance/policies/{module}", json={"enabled": enabled},
        )
        return resp.json()  # type: ignore[no-any-return]

    def reload_policy(self) -> dict[str, Any]:
        """POST /v1/compliance/policies/reload — re-apply the active config."""
        resp = self._client._request_with_retry("POST", "/compliance/policies/reload")
        return resp.json()  # type: ignore[no-any-return]

    def apply_template(self, template: str) -> dict[str, Any]:
        """POST /v1/compliance/policies/template — swap to a named template."""
        resp = self._client._request_with_retry(
            "POST", "/compliance/policies/template", json={"template": template},
        )
        return resp.json()  # type: ignore[no-any-return]

    def enable_module(self, module: str) -> dict[str, Any]:
        """POST /v1/compliance/modules/{name}/enable."""
        resp = self._client._request_with_retry(
            "POST", f"/compliance/modules/{module}/enable",
        )
        return resp.json()  # type: ignore[no-any-return]

    def disable_module(self, module: str) -> dict[str, Any]:
        """POST /v1/compliance/modules/{name}/disable."""
        resp = self._client._request_with_retry(
            "POST", f"/compliance/modules/{module}/disable",
        )
        return resp.json()  # type: ignore[no-any-return]

    # --- audit --------------------------------------------------------

    def query_audit(
        self,
        *,
        from_unix_nanos: int | None = None,
        to_unix_nanos: int | None = None,
        module: str | None = None,
        decision: str | None = None,
        tenant: str | None = None,
        limit: int | None = None,
    ) -> AuditQueryResponse:
        """GET /v1/compliance/audit — query the tamper-evident log."""
        params: dict[str, Any] = {}
        if from_unix_nanos is not None:
            params["from"] = from_unix_nanos
        if to_unix_nanos is not None:
            params["to"] = to_unix_nanos
        if module is not None:
            params["module"] = module
        if decision is not None:
            params["decision"] = decision
        if tenant is not None:
            params["tenant"] = tenant
        if limit is not None:
            params["limit"] = limit
        resp = self._client._request_with_retry("GET", "/compliance/audit", params=params)
        return AuditQueryResponse.model_validate(resp.json())

    def get_audit_entry(self, entry_id: int) -> dict[str, Any]:
        """GET /v1/compliance/audit/{id} — single row by id."""
        resp = self._client._request_with_retry("GET", f"/compliance/audit/{entry_id}")
        return resp.json()  # type: ignore[no-any-return]

    def verify_audit(self) -> dict[str, Any]:
        """GET /v1/compliance/audit/verify — full-chain verification."""
        resp = self._client._request_with_retry("GET", "/compliance/audit/verify")
        return resp.json()  # type: ignore[no-any-return]

    def audit_integrity(self) -> dict[str, Any]:
        """GET /v1/compliance/audit/integrity — cheap integrity snapshot."""
        resp = self._client._request_with_retry("GET", "/compliance/audit/integrity")
        return resp.json()  # type: ignore[no-any-return]

    # --- reports ------------------------------------------------------

    def list_reports(self) -> ComplianceReportList:
        """GET /v1/compliance/reports — every record (newest first)."""
        resp = self._client._request_with_retry("GET", "/compliance/reports")
        return ComplianceReportList.model_validate(resp.json())

    def generate_report(
        self,
        *,
        report_type: str,
        from_unix_nanos: int,
        to_unix_nanos: int,
        tenant: str | None = None,
        report_format: str = "json",
        policy_version: str = "local-dev",
        **kwargs: Any,
    ) -> ComplianceReport:
        """POST /v1/compliance/reports/generate — synchronous generation."""
        if "format" in kwargs:
            report_format = kwargs.pop("format")
        if kwargs:
            unexpected = ", ".join(sorted(kwargs))
            msg = f"Unexpected generate_report keyword(s): {unexpected}"
            raise TypeError(msg)
        body: dict[str, Any] = {
            "report_type": report_type,
            "from_unix_nanos": from_unix_nanos,
            "to_unix_nanos": to_unix_nanos,
            "format": report_format,
            "policy_version": policy_version,
        }
        if tenant is not None:
            body["tenant"] = tenant
        resp = self._client._request_with_retry(
            "POST", "/compliance/reports/generate", json=body,
        )
        return ComplianceReport.model_validate(resp.json())

    def get_report(self, report_id: str) -> ComplianceReport:
        """GET /v1/compliance/reports/{id} — record metadata (no body bytes)."""
        resp = self._client._request_with_retry("GET", f"/compliance/reports/{report_id}")
        return ComplianceReport.model_validate(resp.json())

    def download_report(self, report_id: str) -> bytes:
        """GET /v1/compliance/reports/{id}/download — rendered body bytes."""
        resp = self._client._request_with_retry(
            "GET", f"/compliance/reports/{report_id}/download",
        )
        return resp.content

    def delete_report(self, report_id: str) -> ComplianceReport:
        """DELETE /v1/compliance/reports/{id} — refuses protected records."""
        resp = self._client._request_with_retry(
            "DELETE", f"/compliance/reports/{report_id}",
        )
        return ComplianceReport.model_validate(resp.json())


__all__ = [
    "Admin",
    "Auth",
    "Compliance",
    "Models",
    "Power",
    "Scheduler",
    "System",
    "Trust",
    "TrustNotProvisionedError",
    "Updates",
]
