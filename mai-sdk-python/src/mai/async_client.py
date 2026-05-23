"""MAI SDK async client.

Mirror of :mod:`mai.client`. Same surface, but every method is a
coroutine and uses ``httpx.AsyncClient``. Async namespace classes
live here to avoid a coupling cycle with :mod:`mai._namespaces`.
"""

from __future__ import annotations

import asyncio
import json
from collections.abc import AsyncIterator
from typing import TYPE_CHECKING, Any

import httpx

from mai.config import MaiClientConfig
from mai.errors import MaiError, from_response, from_transport
from mai.types import (
    AirgapStatusResponse,
    AuditLogResponse,
    AuditQueryResponse,
    BenchmarkResult,
    ChatCompletionChunk,
    ChatCompletionRequest,
    ChatCompletionResponse,
    ChatMessage,
    CompletionRequest,
    CompletionResponse,
    ComplianceReport,
    ComplianceReportList,
    ComplianceStatus,
    EmbeddingRequest,
    EmbeddingResponse,
    ErrorResponse,
    ExchangeTokenResponse,
    FunctionCallRequest,
    FunctionCallResponse,
    HardwareHealthResponse,
    HealthResponse,
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
    StructuredRequest,
    StructuredResponse,
    SystemHealthResponse,
    TrustBundleStatus,
    TrustClaimsResponse,
    TrustStatusResponse,
    UpdateCheckResponse,
    UpdateStatusResponse,
)

if TYPE_CHECKING:
    from types import TracebackType

_HTTP_ERROR_STATUS = 400


def _parse_sse_line(line: str) -> str | None:
    line = line.strip()
    if not line or line.startswith(":"):
        return None
    if line.startswith("data: "):
        data = line[6:]
        if data == "[DONE]":
            return None
        return data
    return None


def _build_error(resp: httpx.Response) -> MaiError:
    try:
        err_resp = ErrorResponse.model_validate(resp.json())
    except Exception:
        err_resp = ErrorResponse.model_validate({"error": {
            "code": f"MAI-{resp.status_code}0",
            "message": resp.text or f"HTTP {resp.status_code}",
            "type": "internal_error",
        }})
    return from_response(err_resp, resp.status_code)


# ---------------------------------------------------------------------------
# Async namespaces
# ---------------------------------------------------------------------------

class AsyncModels:
    def __init__(self, client: AsyncMaiClient) -> None:
        self._client = client

    async def list(self, **filters: Any) -> list[ModelObject]:
        resp = await self._client._request_with_retry("GET", "/models", params=filters)
        data = resp.json()
        return [ModelObject.model_validate(m) for m in data.get("data", [])]

    async def get(self, model_id: str) -> ModelDetail:
        resp = await self._client._request_with_retry("GET", f"/models/{model_id}")
        return ModelDetail.model_validate(resp.json())

    async def load(self, model_id: str) -> ModelLoadResponse:
        resp = await self._client._request_with_retry("POST", f"/models/{model_id}/load")
        return ModelLoadResponse.model_validate(resp.json())

    async def unload(self, model_id: str) -> ModelUnloadResponse:
        resp = await self._client._request_with_retry(
            "POST", f"/models/{model_id}/unload",
        )
        return ModelUnloadResponse.model_validate(resp.json())

    async def benchmark(self, model_id: str, **opts: Any) -> BenchmarkResult:
        resp = await self._client._request_with_retry(
            "POST", f"/models/{model_id}/benchmark", json=opts or None,
        )
        return BenchmarkResult.model_validate(resp.json())

    async def get_benchmark(self, model_id: str) -> BenchmarkResult:
        resp = await self._client._request_with_retry(
            "GET", f"/models/{model_id}/benchmark",
        )
        return BenchmarkResult.model_validate(resp.json())

    async def discover(self, path: str | None = None) -> ModelDiscoverResponse:
        body = {"path": path} if path else {}
        resp = await self._client._request_with_retry(
            "POST", "/models/discover", json=body,
        )
        return ModelDiscoverResponse.model_validate(resp.json())

    async def install(
        self, package_bytes: bytes, *, filename: str = "package.mpkg",
    ) -> ModelInstallResponse:
        files = {"package": (filename, package_bytes, "application/octet-stream")}
        resp = await self._client._request_with_retry(
            "POST", "/models/install", files=files,
        )
        return ModelInstallResponse.model_validate(resp.json())

    async def remove(self, model_id: str) -> ModelRemoveResponse:
        resp = await self._client._request_with_retry(
            "POST", f"/models/{model_id}/remove",
        )
        return ModelRemoveResponse.model_validate(resp.json())


class AsyncPower:
    def __init__(self, client: AsyncMaiClient) -> None:
        self._client = client

    async def get_state(self) -> PowerStateResponse:
        resp = await self._client._request_with_retry("GET", "/power/state")
        return PowerStateResponse.model_validate(resp.json())

    async def transition(
        self, request: PowerTransitionRequest,
    ) -> PowerTransitionResponse:
        resp = await self._client._request_with_retry(
            "POST", "/power/transition", json=request.model_dump(),
        )
        return PowerTransitionResponse.model_validate(resp.json())


class AsyncSystem:
    def __init__(self, client: AsyncMaiClient) -> None:
        self._client = client

    async def airgap(self) -> AirgapStatusResponse:
        resp = await self._client._request_with_retry("GET", "/system/airgap")
        return AirgapStatusResponse.model_validate(resp.json())

    async def system_health(self) -> SystemHealthResponse:
        resp = await self._client._request_with_retry("GET", "/health/system")
        return SystemHealthResponse.model_validate(resp.json())

    async def hardware_health(self) -> HardwareHealthResponse:
        resp = await self._client._request_with_retry("GET", "/health/hardware")
        return HardwareHealthResponse.model_validate(resp.json())


class AsyncScheduler:
    def __init__(self, client: AsyncMaiClient) -> None:
        self._client = client

    async def metrics(self) -> SchedulerMetricsResponse:
        resp = await self._client._request_with_retry("GET", "/scheduler/metrics")
        return SchedulerMetricsResponse.model_validate(resp.json())

    async def instance_metrics(self, instance_id: str) -> InstanceMetricsResponse:
        resp = await self._client._request_with_retry(
            "GET", f"/scheduler/instances/{instance_id}/metrics",
        )
        return InstanceMetricsResponse.model_validate(resp.json())

    async def instance_health(self, instance_id: str) -> InstanceHealthResponse:
        resp = await self._client._request_with_retry(
            "GET", f"/scheduler/instances/{instance_id}/health",
        )
        return InstanceHealthResponse.model_validate(resp.json())

    async def anomalies(self) -> SchedulerAnomaliesResponse:
        resp = await self._client._request_with_retry("GET", "/scheduler/anomalies")
        return SchedulerAnomaliesResponse.model_validate(resp.json())


class AsyncUpdates:
    def __init__(self, client: AsyncMaiClient) -> None:
        self._client = client

    async def check(self) -> UpdateCheckResponse:
        resp = await self._client._request_with_retry("GET", "/updates/check")
        return UpdateCheckResponse.model_validate(resp.json())

    async def download(self, component: str, target_version: str) -> dict[str, Any]:
        resp = await self._client._request_with_retry(
            "POST", "/updates/download",
            json={"component": component, "target_version": target_version},
        )
        return resp.json()  # type: ignore[no-any-return]

    async def status(self) -> UpdateStatusResponse:
        resp = await self._client._request_with_retry("GET", "/updates/status")
        return UpdateStatusResponse.model_validate(resp.json())


class AsyncAdmin:
    def __init__(self, client: AsyncMaiClient) -> None:
        self._client = client

    async def list_profiles(self) -> list[ProfileObject]:
        resp = await self._client._request_with_retry("GET", "/profiles")
        data = resp.json()
        return [ProfileObject.model_validate(p) for p in data.get("data", [])]

    async def get_profile(self, profile_id: str) -> ProfileObject:
        resp = await self._client._request_with_retry(
            "GET", f"/profiles/{profile_id}",
        )
        return ProfileObject.model_validate(resp.json())

    async def audit_log(
        self, *, offset: int = 0, limit: int = 100, since_unix: int | None = None,
    ) -> AuditLogResponse:
        params: dict[str, Any] = {"offset": offset, "limit": limit}
        if since_unix is not None:
            params["since"] = since_unix
        resp = await self._client._request_with_retry(
            "GET", "/audit/log", params=params,
        )
        return AuditLogResponse.model_validate(resp.json())

    async def adapters(self) -> dict[str, Any]:
        resp = await self._client._request_with_retry("GET", "/adapters")
        return resp.json()  # type: ignore[no-any-return]

    async def registry(self) -> dict[str, Any]:
        resp = await self._client._request_with_retry("GET", "/registry")
        return resp.json()  # type: ignore[no-any-return]

    async def registry_scan(self) -> dict[str, Any]:
        resp = await self._client._request_with_retry("POST", "/registry/scan")
        return resp.json()  # type: ignore[no-any-return]


class AsyncAuth:
    def __init__(self, client: AsyncMaiClient) -> None:
        self._client = client

    async def exchange_token(
        self,
        subject_id: str,
        *,
        tenant_id: str | None = None,
        scopes: list[str] | None = None,
    ) -> ExchangeTokenResponse:
        body: dict[str, Any] = {"subject_id": subject_id}
        if tenant_id is not None:
            body["tenant_id"] = tenant_id
        if scopes is not None:
            body["scopes"] = scopes
        resp = await self._client._request_with_retry(
            "POST", "/auth/exchange_token", json=body,
        )
        return ExchangeTokenResponse.model_validate(resp.json())


class AsyncTrust:
    def __init__(self, client: AsyncMaiClient) -> None:
        self._client = client

    async def status(self) -> TrustStatusResponse:
        resp = await self._client._request_with_retry("GET", "/trust/status")
        return TrustStatusResponse.model_validate(resp.json())

    async def claims(self) -> TrustClaimsResponse:
        resp = await self._client._request_with_retry("GET", "/trust/claims")
        return TrustClaimsResponse.model_validate(resp.json())

    async def bundle_status(self) -> TrustBundleStatus:
        resp = await self._client._request_with_retry("GET", "/trust/bundle_status")
        return TrustBundleStatus.model_validate(resp.json())

    async def revocation_status(self, claim_id: str) -> RevocationStatusResponse:
        resp = await self._client._request_with_retry(
            "GET", "/trust/revocation_status", params={"claim_id": claim_id},
        )
        return RevocationStatusResponse.model_validate(resp.json())


class AsyncCompliance:
    def __init__(self, client: AsyncMaiClient) -> None:
        self._client = client

    async def get_status(self) -> ComplianceStatus:
        resp = await self._client._request_with_retry("GET", "/compliance/status")
        return ComplianceStatus.model_validate(resp.json())

    async def get_policies(self) -> list[dict[str, Any]]:
        resp = await self._client._request_with_retry("GET", "/compliance/policies")
        return list(resp.json().get("modules", []))

    async def get_policy(self, module: str) -> dict[str, Any]:
        resp = await self._client._request_with_retry(
            "GET", f"/compliance/policies/{module}",
        )
        return resp.json()  # type: ignore[no-any-return]

    async def update_policy(self, module: str, *, enabled: bool) -> dict[str, Any]:
        resp = await self._client._request_with_retry(
            "PUT", f"/compliance/policies/{module}", json={"enabled": enabled},
        )
        return resp.json()  # type: ignore[no-any-return]

    async def reload_policy(self) -> dict[str, Any]:
        resp = await self._client._request_with_retry(
            "POST", "/compliance/policies/reload",
        )
        return resp.json()  # type: ignore[no-any-return]

    async def apply_template(self, template: str) -> dict[str, Any]:
        resp = await self._client._request_with_retry(
            "POST", "/compliance/policies/template", json={"template": template},
        )
        return resp.json()  # type: ignore[no-any-return]

    async def query_audit(
        self,
        *,
        from_unix_nanos: int | None = None,
        to_unix_nanos: int | None = None,
        module: str | None = None,
        decision: str | None = None,
        tenant: str | None = None,
        limit: int | None = None,
    ) -> AuditQueryResponse:
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
        resp = await self._client._request_with_retry(
            "GET", "/compliance/audit", params=params,
        )
        return AuditQueryResponse.model_validate(resp.json())

    async def verify_audit(self) -> dict[str, Any]:
        resp = await self._client._request_with_retry(
            "GET", "/compliance/audit/verify",
        )
        return resp.json()  # type: ignore[no-any-return]

    async def audit_integrity(self) -> dict[str, Any]:
        resp = await self._client._request_with_retry(
            "GET", "/compliance/audit/integrity",
        )
        return resp.json()  # type: ignore[no-any-return]

    async def list_reports(self) -> ComplianceReportList:
        resp = await self._client._request_with_retry("GET", "/compliance/reports")
        return ComplianceReportList.model_validate(resp.json())

    async def generate_report(
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
        resp = await self._client._request_with_retry(
            "POST", "/compliance/reports/generate", json=body,
        )
        return ComplianceReport.model_validate(resp.json())

    async def get_report(self, report_id: str) -> ComplianceReport:
        resp = await self._client._request_with_retry(
            "GET", f"/compliance/reports/{report_id}",
        )
        return ComplianceReport.model_validate(resp.json())

    async def download_report(self, report_id: str) -> bytes:
        resp = await self._client._request_with_retry(
            "GET", f"/compliance/reports/{report_id}/download",
        )
        return resp.content

    async def delete_report(self, report_id: str) -> ComplianceReport:
        resp = await self._client._request_with_retry(
            "DELETE", f"/compliance/reports/{report_id}",
        )
        return ComplianceReport.model_validate(resp.json())


# ---------------------------------------------------------------------------
# Client
# ---------------------------------------------------------------------------

class AsyncMaiClient:
    """Asynchronous MAI API client.

    Usage::

        async with AsyncMaiClient.from_env() as client:
            response = await client.chat("qwen3-14b:Q4_K_M", messages)
            async for chunk in client.chat_stream(...):
                ...
    """

    def __init__(self, config: MaiClientConfig | None = None) -> None:
        self._config = config or MaiClientConfig()
        self._http = httpx.AsyncClient(
            base_url=self._config.base_url,
            headers=self._config.headers(),
            timeout=self._config.timeout,
        )
        self.models = AsyncModels(self)
        self.power = AsyncPower(self)
        self.system = AsyncSystem(self)
        self.scheduler = AsyncScheduler(self)
        self.updates = AsyncUpdates(self)
        self.admin = AsyncAdmin(self)
        self.auth = AsyncAuth(self)
        self.trust = AsyncTrust(self)
        self.compliance = AsyncCompliance(self)

    @classmethod
    def from_env(cls, **overrides: Any) -> AsyncMaiClient:
        return cls(MaiClientConfig.from_env(**overrides))

    @classmethod
    def from_file(cls, path: str, **overrides: Any) -> AsyncMaiClient:
        return cls(MaiClientConfig.from_file(path, **overrides))

    @classmethod
    def load(cls, path: str | None = None, **overrides: Any) -> AsyncMaiClient:
        return cls(MaiClientConfig.load(path, **overrides))

    async def close(self) -> None:
        await self._http.aclose()

    async def __aenter__(self) -> AsyncMaiClient:
        return self

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        tb: TracebackType | None,
    ) -> None:
        await self.close()

    # --- Inference -----------------------------------------------------

    async def chat(
        self,
        model: str,
        messages: list[ChatMessage],
        *,
        temperature: float = 0.7,
        top_p: float = 0.9,
        max_tokens: int = 2048,
        **kwargs: Any,
    ) -> ChatCompletionResponse:
        req = ChatCompletionRequest(
            model=model, messages=messages, temperature=temperature,
            top_p=top_p, max_tokens=max_tokens, stream=False, **kwargs,
        )
        resp = await self._request_with_retry(
            "POST", "/chat/completions", json=req.model_dump(),
        )
        return ChatCompletionResponse.model_validate(resp.json())

    async def chat_stream(
        self,
        model: str,
        messages: list[ChatMessage],
        *,
        temperature: float = 0.7,
        top_p: float = 0.9,
        max_tokens: int = 2048,
        **kwargs: Any,
    ) -> AsyncIterator[ChatCompletionChunk]:
        req = ChatCompletionRequest(
            model=model, messages=messages, temperature=temperature,
            top_p=top_p, max_tokens=max_tokens, stream=True, **kwargs,
        )
        async with self._http.stream(
            "POST",
            "/chat/completions",
            json=req.model_dump(),
            timeout=self._config.stream_timeout,
        ) as response:
            if response.status_code >= _HTTP_ERROR_STATUS:
                await response.aread()
                raise _build_error(response)
            async for line in response.aiter_lines():
                data = _parse_sse_line(line)
                if data is not None:
                    yield ChatCompletionChunk.model_validate(json.loads(data))

    stream_chat = chat_stream

    async def stream_completions(
        self, model: str, prompt: str, **kwargs: Any,
    ) -> AsyncIterator[ChatCompletionChunk]:
        messages = [ChatMessage(role="user", content=prompt)]
        async for chunk in self.chat_stream(model, messages, **kwargs):
            yield chunk

    async def complete(
        self, model: str, prompt: str, **kwargs: Any,
    ) -> CompletionResponse:
        req = CompletionRequest(model=model, prompt=prompt, stream=False, **kwargs)
        resp = await self._request_with_retry(
            "POST", "/completions", json=req.model_dump(),
        )
        return CompletionResponse.model_validate(resp.json())

    completions = complete

    async def embed(
        self, model: str, input_: str | list[str],
    ) -> EmbeddingResponse:
        req = EmbeddingRequest(model=model, input=input_)
        resp = await self._request_with_retry(
            "POST", "/embeddings", json=req.model_dump(),
        )
        return EmbeddingResponse.model_validate(resp.json())

    embeddings = embed

    async def structured(
        self, model: str, prompt: str, schema: dict[str, Any], **kwargs: Any,
    ) -> StructuredResponse:
        req = StructuredRequest(model=model, prompt=prompt, schema=schema, **kwargs)
        resp = await self._request_with_retry(
            "POST", "/generate/structured", json=req.model_dump(by_alias=True),
        )
        return StructuredResponse.model_validate(resp.json())

    structured_generation = structured

    async def function_call(
        self,
        model: str,
        messages: list[ChatMessage],
        functions: list[dict[str, Any]],
    ) -> FunctionCallResponse:
        req = FunctionCallRequest(
            model=model, messages=messages, functions=functions,
        )
        resp = await self._request_with_retry(
            "POST", "/generate/function_call", json=req.model_dump(),
        )
        return FunctionCallResponse.model_validate(resp.json())

    # --- Top-level convenience -----------------------------------------

    async def list_models(self, **filters: Any) -> list[ModelObject]:
        return await self.models.list(**filters)

    async def get_model(self, model_id: str) -> ModelDetail:
        return await self.models.get(model_id)

    async def health(self) -> HealthResponse:
        resp = await self._http.get("/health")
        self._check_error(resp)
        return HealthResponse.model_validate(resp.json())

    async def health_check(self) -> bool:
        try:
            resp = await self._http.get("/health")
            return resp.status_code < _HTTP_ERROR_STATUS
        except Exception:
            return False

    async def hardware_health(self) -> HardwareHealthResponse:
        return await self.system.hardware_health()

    async def power_state(self) -> PowerStateResponse:
        return await self.power.get_state()

    # --- Transport core ------------------------------------------------

    async def _request_with_retry(
        self, method: str, url: str, **kwargs: Any,
    ) -> httpx.Response:
        last_error: MaiError | None = None
        policy = self._config.retry
        for attempt in range(policy.max_retries + 1):
            try:
                resp = await self._http.request(method, url, **kwargs)
            except httpx.TransportError as exc:
                last_error = from_transport(exc)
                delay = policy.should_retry(last_error, attempt)
                if delay is None:
                    raise last_error from exc
                await asyncio.sleep(delay)
                continue

            if resp.status_code < _HTTP_ERROR_STATUS:
                return resp

            last_error = _build_error(resp)
            delay = policy.should_retry(last_error, attempt)
            if delay is None:
                raise last_error
            await asyncio.sleep(delay)

        if last_error is not None:
            raise last_error
        raise RuntimeError("Retry loop exited without result")

    @staticmethod
    def _check_error(resp: httpx.Response) -> None:
        if resp.status_code >= _HTTP_ERROR_STATUS:
            raise _build_error(resp)


__all__ = [
    "AsyncAdmin",
    "AsyncAuth",
    "AsyncCompliance",
    "AsyncMaiClient",
    "AsyncModels",
    "AsyncPower",
    "AsyncScheduler",
    "AsyncSystem",
    "AsyncTrust",
    "AsyncUpdates",
]
