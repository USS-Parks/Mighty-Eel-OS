# Lamprey MAI Stack Memo

**Project:** Island Mountain Mighty Eel OS / MAI / Lamprey  
**Purpose:** Describe the Lamprey MAI stack piece by piece, from back-end runtime foundations through API, governance, SDKs, applications, demos, and operator-facing front-end surfaces.  
**Audience:** engineering, acquisition diligence, implementation agents, security reviewers, and product leadership.  
**Status:** Architecture memo following the Ship Hardening Plan.  
**Last updated:** 2026-05-23.

---

## 1. Executive Summary

Lamprey MAI is the local inference, policy, trust, and audit layer for Island Mountain's sovereign AI operating system. It is not merely a model gateway, and it is not merely a compliance dashboard. It is a layered appliance architecture designed to let regulated organizations run AI close to sensitive data while proving who accessed it, which policy governed it, where the request was allowed to route, and how the resulting decision can be verified after the fact.

At the bottom of the stack are hardware, vault, scheduler, adapter, and model lifecycle components. In the middle are the REST/gRPC API, authentication, streaming, metrics, power state, and registry surfaces. Above that sits Lamprey: the governance layer that classifies requests, evaluates HIPAA, ITAR/EAR, and OCAP policy, composes conflicts, binds decisions to trust context, and writes tamper-evident audit evidence. Above Lamprey are SDKs, reference apps, demos, reports, and the compliance dashboard.

The product thesis is simple:

> Inference is replaceable. The sovereignty, policy, trust, and audit boundary is the product.

The stack therefore treats every request as more than an inference call. It is an event in a local trust system.

---

## 2. Stack Map

From lowest-level foundation to front-end/operator surfaces:

| Layer | Component Area | Primary Code Locations | Responsibility |
|---|---|---|---|
| Hardware foundation | HIL drivers and hardware traits | `mai-hil/` | Abstract GPU, CPU, and future hardware targets behind typed interfaces. |
| Vault and cryptography | model storage, PQC, TPM, ZFS, audit signing | `mai-vault/`, `mai-core/src/vault.rs` | Protect model packages, sealed secrets, signatures, snapshots, and future production persistence. |
| Core kernel | registry, power, health, cache, model lifecycle | `mai-core/` | Trusted core state machines and local node lifecycle. |
| Scheduler | topology, KV cache, batching, scoring, placement | `mai-scheduler/` | Decide which backend instance should serve each request. |
| Adapter framework | Rust manager and Python adapters | `mai-adapters/`, `adapters/` | Spawn, monitor, and communicate with backend inference engines. |
| API server | REST, gRPC, SSE, WebSocket, auth, audit middleware | `mai-api/` | Stable product boundary for apps, SDKs, dashboards, and operators. |
| Router | sensitivity classifier, entity detection, route rules | `mai-router/` | Pre-policy routing analysis and local/frontier decision support. |
| Compliance runtime | HIPAA, ITAR/EAR, OCAP, policy composer, trust, reports, audit | `mai-compliance/` | Lamprey governance layer. |
| SDKs | Python and Rust clients | `mai-sdk-python/`, `mai-sdk-rs/` | Developer-facing client libraries for product integrations. |
| Applications and demos | reference apps and acquisition demos | `apps/`, `tools/`, `docs/DEMO-SUITE.md` | Proof workflows showing secure inference, RAG, trust, OCAP, and operator flows. |
| Dashboard | compliance operator UI | `compliance-dashboard/` | Front-end proof surface for trust, audit, policies, reports, and alerts. |
| Deployment and operations | profiles, launch scripts, validation, hardening plan | `deployment/`, `scripts/`, `docs/` | Launch, validate, harden, package, and operate the stack. |

---

## 3. Hardware and Runtime Foundation

### 3.1 Hardware Interface Layer

The Hardware Interface Layer, under `mai-hil/`, defines the typed boundary between MAI and physical compute. It prevents the rest of the product from hard-coding assumptions about CUDA, NVLink, AMD, CPU fallback, or future accelerators.

The important idea is not that every future device is implemented today. The important idea is that hardware is a replaceable substrate. The scheduler and policy layers should not care whether the backend is a local NVIDIA GPU, AMD hardware, CPU fallback, or a future memristor/photonic target. They should care about capabilities, health, memory, placement cost, and policy eligibility.

Key responsibilities:

- expose hardware probe traits
- expose power state traits
- expose memory manager traits
- expose adapter traits
- provide NVIDIA, AMD, CPU, and future-target stubs where appropriate
- let higher layers reason about compute without depending directly on vendor APIs

This is one of the stack's long-term defensibility points: it keeps MAI from becoming a thin wrapper around a single inference backend.

### 3.2 Local-First Runtime Assumption

MAI is designed as a local-first appliance, not a cloud-hosted SaaS control plane. The node is expected to run on customer-controlled hardware, including disconnected or air-gapped environments.

That assumption shapes the whole stack:

- policy must work offline
- trust material must cache locally
- audit records must be locally verifiable
- model routing must not depend on cloud availability
- sensitive prompts, completions, embeddings, PHI, export-controlled material, and OCAP-governed data must not cross into the trust plane

The cloud, when present, is a trust and identity source. It is not the place where regulated payloads go.

---

## 4. Vault, Cryptography, and Protected State

### 4.1 Vault Boundary

The vault layer is responsible for protected local state: model package verification, encrypted storage, sealed keys, snapshots, and audit-signing support. The relevant code lives primarily in `mai-vault/`, with a vault interface exposed through `mai-core`.

Conceptually, the vault protects:

- model weights and packages
- package signatures
- sealed master keys
- TPM/PCR-related sealing state
- ZFS-backed storage paths where available
- vector store backup hooks
- audit checkpoint signing hooks

The current architecture distinguishes between real vault capability and bootstrap/test paths. The Ship Hardening Plan exists because production startup must use the real vault path, not the demo-safe `StubVault` path. As of SHIP-07 convergence (2026-05-23, commit `48c7d2e`), `MaiServer::run()` actually constructs the real vault via `vault_builder::build_vault` whenever `MAI_SHIP_PROFILE` is set or `MaiServer::with_ship_profile(path)` is called; `StubVault` remains in the tree only for the no-profile bring-up path (tests + local-dev).

### 4.2 Post-Quantum Cryptography

The vault and compliance stack use post-quantum cryptographic concepts around ML-KEM and ML-DSA. The product narrative is not "we sprinkled crypto on logs." The stronger claim is:

- signed policy bundles can be verified locally
- audit checkpoints can be signed
- report certification can be verified off-host
- the verifier does not need to trust the running MAI process if it has the public verification material and canonical output

That matters for acquisition, regulation, and field deployment. A regulator or reviewer should be able to verify a report or audit artifact without taking the vendor's word for it.

### 4.3 Production Hardening Status

As of SHIP-07 convergence (2026-05-23), the vault layer is wired directly into `mai-api` startup. When `MAI_SHIP_PROFILE` is set, `MaiServer::run()` constructs `ZfsVault` via `vault_builder::build_vault`, opens the persistent `WalAuditWriter` for API audit, installs the sealer-backed compliance log via `build_sealer`, and runs the real ML-DSA bundle verifier via `build_trust_components`. The production guard refuses to bind sockets if any Critical Fail surfaces in `ProductionReadinessReport::evaluate_with_runtime`.

The production profile now refuses, by typed-error and by readiness check:

- stub vault — `VaultBuildError::StubInProduction` + `PROD-VAULT-001` / `PROD-VAULT-002`
- missing vault root — `VaultBuildError::RootMissing` + `PROD-VAULT-100`
- missing sealed state — `PROD-VAULT-004` (parse-time)
- missing PQC provider when required — `PROD-VAULT-005` (parse-time)
- silent fallback to insecure storage — `MaiServer::run()` returns `ServerError::Init` on any builder rejection rather than falling back.

That closes the gap between having cryptographic modules in the repo and actually running a cryptographically anchored appliance. The remaining surfaces (admin HTTP readiness endpoint + standalone `mai-ship-validate` binary + profile-aware token exchange) are tracked in the SHIP-07-endpoint-and-cli slice.

---

## 5. Core Kernel

The core kernel lives in `mai-core/`. It contains the local trusted state machines and product primitives that the API server exposes.

Primary responsibilities:

- model registry
- model install/load/unload lifecycle
- package verification boundary
- health monitor
- circuit breaker
- cache primitives
- power state machine
- Sentinel promotion and demotion behavior
- hot-swap manager
- air-gap policy support
- core error taxonomy

### 5.1 Model Registry

The registry tracks available models, installed models, metadata, manifests, and lifecycle state. It is the local source of truth for what the node can serve.

In the full product path, the registry should be backed by persistent vault-aware storage. It should not depend on transient process memory for production state.

### 5.2 Model Lifecycle

The model lifecycle modules cover installation, package opening, verification, preload, remove, update, USB workflows, and benchmarking placeholders or implementations where appropriate.

The product-level lifecycle is:

1. discover model package
2. verify package manifest and signature
3. install into protected storage
4. load into selected backend
5. expose via registry/API
6. route requests through scheduler and policy
7. unload or update safely

### 5.3 Power and Sentinel State

MAI includes power-state and Sentinel logic so the node can degrade or promote compute behavior based on state, thermal pressure, idle time, or policy. This is especially relevant for local appliances where power, thermal, and hardware availability vary.

Sentinel is the lightweight path. Full inference is the heavy path. The system can promote or demote rather than treating inference capacity as binary.

### 5.4 Health and Circuit Breaking

The core health monitor and circuit breaker protect the system against bad adapters, unhealthy backends, and degraded hardware. Health is not just an operator display. It feeds routing, availability, and production readiness.

---

## 6. Scheduler

The scheduler lives in `mai-scheduler/`. It is one of the most important backend components because it turns local hardware and backend state into placement decisions.

### 6.1 Placement Role

The scheduler answers:

> Given this request, model, priority, sequence, token estimate, current hardware state, KV cache state, and backend health, which instance should serve it?

It is more sophisticated than a load balancer. A generic load balancer can count requests. MAI's scheduler reasons about model instances, topology, memory, KV locality, batching opportunity, preemption, and power state.

### 6.2 Core Scheduling Inputs

Important inputs include:

- model alias and model instance
- priority
- prompt and generation token estimates
- sequence/session ID
- adapter health
- backend load
- GPU topology
- VRAM usage
- KV cache residency
- batching state
- power state
- scoring weights

### 6.3 GPU Topology Awareness

Topology awareness lets the scheduler understand that not all GPU combinations are equal. NVLink, PCIe, cross-socket distance, mixed GPU topology, and single-GPU fallback have different costs.

The topology modules parse and represent GPU graphs, then expose penalties and placement signals.

### 6.4 KV Cache Management

KV cache behavior is central to inference performance. MAI includes:

- KV sequence tracking
- tiered/offload support
- eviction policy
- soft eviction
- guards and triggers
- pressure handling
- sequence locality

This lets the scheduler prefer placements that avoid throwing away useful context when possible.

### 6.5 Batching and Preemption

The scheduler includes batching and preemption logic so it can improve throughput while still respecting priority. It can admit, reject, defer, or preempt based on memory pressure, priority, and active batch state.

### 6.6 Multi-Factor Scoring

The scoring layer combines multiple signals:

- latency
- memory
- topology
- eviction cost
- batching value
- health
- policy constraints once wired through the router/compliance path

The outcome is a placement decision with a reason, not a black box.

---

## 7. Adapter Framework and Backend Engines

The adapter framework spans Rust and Python:

- Rust adapter manager: `mai-adapters/`
- Python backend adapters: `adapters/`

### 7.1 Adapter Manager

The Rust-side adapter manager discovers, starts, monitors, and shuts down backend adapter processes. It is the supervisor between the trusted Rust API/scheduler world and the less-trusted backend-specific Python world.

Responsibilities:

- adapter discovery
- process startup
- health checks
- IPC protocol
- generation and embedding requests
- streaming events
- shutdown
- validation
- adapter audit hooks

### 7.2 Python Backend Adapters

The Python adapters make MAI backend-neutral. The stack includes adapters for:

- Ollama
- vLLM
- llama.cpp
- TGI
- TensorRT-LLM
- ExLlamaV2
- SGLang

Each adapter follows a common base contract and translates MAI's request shape into backend-specific calls.

### 7.3 Trust Boundary

The adapter layer is intentionally not the trusted center of the product. It is a controlled execution boundary around third-party or backend-specific inference systems.

That means:

- policy decisions happen before adapter execution
- audit context is created outside the adapter
- backend-specific errors should not leak sensitive internals
- adapter crashes should degrade health, not corrupt product state
- a backend can be replaced without changing the governance model

---

## 8. API Server

The API server lives in `mai-api/`. It is the main runtime boundary for clients, SDKs, dashboards, demos, and operators.

### 8.1 Protocol Surfaces

MAI exposes:

- REST API
- gRPC API
- Server-Sent Events streaming
- WebSocket streaming/control surfaces

The API server owns:

- route definitions
- shared application state
- auth middleware
- audit middleware
- request handlers
- error mapping
- config loading
- server startup and shutdown
- gRPC service registration

### 8.2 REST Surface

Major REST route groups:

- `/v1/chat/completions`
- `/v1/completions`
- `/v1/embeddings`
- `/v1/generate/*`
- `/v1/models/*`
- `/v1/updates/*`
- `/v1/health/*`
- `/v1/system/*`
- `/v1/power/*`
- `/v1/registry/*`
- `/v1/adapters`
- `/v1/audit/*`
- `/v1/profiles/*`
- `/v1/scheduler/*`
- `/v1/ws`
- `/v1/trust/*`
- `/v1/auth/exchange_token`
- `/v1/compliance/*`

This is the stable product boundary that higher-level applications depend on.

### 8.3 gRPC Surface

The gRPC services mirror the core REST capabilities for internal or high-performance clients:

- inference
- models
- health
- power
- registry
- audit
- standard gRPC health checking

gRPC matters for load balancer compatibility, internal service integration, and strongly typed client generation.

### 8.4 Authentication and Authorization

MAI uses API key authentication with role-based permissions. API keys are generated from OS randomness, stored as hashes, and supplied through `X-IM-Auth-Token`.

Roles include admin, adult, teen, child, and guest. Permissions govern:

- inference
- list models
- manage models
- power control
- profile management
- audit access
- registry writes

There is also model access filtering for safer profile-specific access.

### 8.5 Rate Limiting

Rate limiting is per key, sliding-window based, and returns standard retry information. It protects the local appliance from accidental or hostile overuse.

### 8.6 API Audit

The API audit middleware records request metadata, profile context, route category, response status, timing, and chain hashes. The Ship Hardening Plan requires replacing the current in-memory startup default with a persistent WAL-backed writer in production.

### 8.7 Error Handling

API errors are normalized into stable error responses. Sensitive backend details should be stripped from public error messages. This is important because backend engines and hardware may reveal implementation details that should not leave the appliance boundary.

---

## 9. Router Layer

The router layer lives in `mai-router/`. Its job is to classify and prepare routing decisions before compliance policy and scheduler placement complete the path.

Primary responsibilities:

- sensitivity classification
- entity detection
- route rule evaluation
- fallback handling
- cost modeling
- latency budget enforcement
- programmable routing modules

The router is the first major Lamprey L1 layer:

- Is this request local-only?
- Is frontier/cloud routing even allowed?
- Does the text contain sensitive entities?
- Does policy need to evaluate HIPAA, ITAR/EAR, OCAP, or conflict rules?

The router does not replace the scheduler. It constrains and informs it. The scheduler decides where a permitted request lands; the router and policy stack decide what routes are eligible.

---

## 10. Lamprey Compliance Runtime

The Lamprey compliance runtime lives in `mai-compliance/`. It is the governance layer and the most distinctive part of the stack.

### 10.1 Purpose

Lamprey makes compliance pre-inference. It is not response filtering. It is not logging after the fact. It evaluates policy before the model receives the request, records the decision, and constrains routing.

### 10.2 HIPAA Module

The HIPAA module detects protected health information and related regulated healthcare context. It supports:

- PHI detection
- medical entity classification
- BAA-related logic
- de-identification helpers
- report templates

HIPAA decisions can force local-only routing, trigger redaction/de-identification flows, or produce audit evidence for compliance review.

### 10.3 ITAR/EAR Module

The ITAR/EAR modules evaluate export-controlled or defense-sensitive technical data. Their purpose is to prevent controlled technical content from reaching ineligible routes or backends.

This is route governance, not output cleanup. The strongest posture is refusing or restricting before inference.

### 10.4 OCAP Module

The OCAP module is a unique differentiator. OCAP stands for Ownership, Control, Access, and Possession, and is used in tribal data sovereignty contexts.

The OCAP pipeline evaluates:

- tribal data markers
- treaty context
- cultural sensitivity
- consent
- possession status
- access role
- governance metadata

The policy significance is large: most AI compliance stacks talk about HIPAA or generic privacy. Very few can represent tribal data sovereignty as a first-class routing and audit concern.

### 10.5 Policy Composer

The policy composer resolves multi-domain decisions. If HIPAA, ITAR, and OCAP all trigger, the composer must produce one deterministic outcome.

Important rules:

- deny wins
- most restrictive route wins
- conflicts are recorded, not hidden
- every contributing policy should be visible in the decision payload

This is what prevents "undefined compliance behavior" when real-world data crosses categories.

### 10.6 Trust Context

Lamprey decisions include trust context:

- tenant ID
- subject hash
- claim ID
- trust bundle version
- service identity
- offline mode
- revocation status

This lets the audit layer connect identity, policy, route, and inference without storing sensitive payloads.

### 10.7 Local Trust Cache

The local trust cache lets a node continue operating when disconnected from the cloud trust core, as long as signed bundles remain valid under policy.

Connectivity states include concepts such as:

- connected
- degraded
- stale but not expired
- expired
- air-gapped

The important behavior is not "keep running forever offline." The important behavior is "keep running only within the restrictions justified by locally verifiable trust material."

### 10.8 Compliance Audit

The compliance audit chain records policy decisions, trust events, route outcomes, report events, and correlation fields. It is designed to be tamper-evident and suitable for report generation.

Production must persist and seal this log. The no-op sealer is acceptable for tests and bring-up, not for the ship profile.

### 10.9 Compliance Reports

The report engine generates compliance artifacts for:

- HIPAA
- ITAR/EAR
- OCAP
- activity digest
- system/monthly reporting

Reports are meant to include trust sections and certification signatures so they can be verified off-host.

---

## 11. Trust Manifold and OpenBao Boundary

The Trust Manifold is the broader identity and trust architecture. In the target design, OpenBao owns the enterprise trust functions:

- identity authentication
- secrets
- PKI
- Transit signing
- revocation
- audit-device functions
- service identities

MAI owns local enforcement and inference governance:

- verify signed claims and bundles
- cache trust material
- enforce policy offline
- route locally
- write local audit evidence

### 11.1 Boundary Rule

The trust plane receives claims and metadata. It must not receive prompts, completions, embeddings, PHI payloads, export-controlled payloads, or OCAP-governed payloads.

### 11.2 Service Identity

The architecture defines distinct service identities so no component relies on one broad shared token. Examples include identities for:

- MAI API
- scheduler
- adapter manager
- Lamprey router
- Lamprey policy
- Lamprey audit
- dashboard
- local trust cache
- audit correlation service

The production bridge should use least-privilege identity and signed claims.

### 11.3 Production Gap and Hardening

The local-dev token exchange path exists to prove the wire contract. Production must swap synthetic local-dev exchange for a real OpenBao-backed bridge or disable exchange entirely. The production profile should refuse to start with local-dev synthetic exchange enabled.

---

## 12. SDK Layer

The SDKs make MAI usable by developers and applications.

### 12.1 Python SDK

The Python SDK lives in `mai-sdk-python/`. It exposes typed methods for:

- chat/completions
- streaming
- embeddings
- models
- power
- scheduler metrics
- system status
- updates
- auth
- trust
- compliance

It also handles:

- config
- retries
- error mapping
- API key headers
- async client flows
- CLI helpers

The Python SDK is the primary integration path for the reference apps and dashboard.

### 12.2 Rust SDK

The Rust SDK lives in `mai-sdk-rs/`. It provides Rust-native configuration, auth headers, error handling, and client boundary pieces.

The Rust SDK matters for internal tools, future system components, and strongly typed Rust integrations.

### 12.3 SDK Product Role

The SDKs turn the API into something application developers can use without understanding every internal component. They also stabilize the product boundary: apps should change less often than internal scheduler or compliance implementation details.

---

## 13. Applications and Reference Demos

The `apps/` directory contains reference applications and acquisition demos. These are not all meant to be final end-user products. Many are proof surfaces that demonstrate a stack behavior.

### 13.1 Local Secure Inference

This is the simplest authenticated inference path. It proves:

- API key auth works
- local inference can be called through the SDK
- streaming or completion behavior works
- the stack can be used without compliance demo overhead

### 13.2 RAG Reference

The RAG reference app proves:

- local document ingestion
- chunking
- embeddings
- cosine retrieval
- grounded answer generation

It intentionally avoids requiring an external vector database, which keeps the local-first appliance posture intact.

### 13.3 Compliance-Routed Demo

This demo shows the shape of policy-driven routing. It is useful for explaining HIPAA/ITAR/OCAP routing decisions and how policy output affects the route before inference.

As the stack hardens, this demo should be updated to avoid stale "placeholder" language where real endpoints now exist.

### 13.4 Tribal Sovereignty Demo

This demo is focused on OCAP and tribal data governance. It is one of the most strategically important demos because it illustrates a market-differentiating policy category rather than a generic AI feature.

### 13.5 OpenBao Trust Demo

This demo shows the Trust Manifold sequence:

1. bridge authenticates identity
2. claim is minted
3. local trust status is checked
4. token exchange occurs
5. inference request is made
6. Lamprey metadata is attached
7. audit summary is produced

In local-dev, some bridge behavior is simulated. In production, that simulation must be replaced by real OpenBao integration or blocked.

### 13.6 Operator App

The operator app provides a command-line or simple panel-style view of:

- models
- scheduler
- power
- trust
- system

It is useful for operations and demos but is not the full compliance dashboard.

---

## 14. Compliance Dashboard Front-End

The compliance dashboard lives in `compliance-dashboard/`. It is the main operator-facing front-end for Lamprey.

### 14.1 Dashboard Role

The dashboard is not the system of record. It is a window into `mai-api`.

It displays and controls:

- trust state
- compliance module state
- audit entries
- report generation
- policy toggles
- live alerts
- health checks

Every important value should come from API/SDK calls, not local dashboard-only state.

### 14.2 Dashboard Pages

The dashboard includes:

- Overview
- Audit
- Reports
- Policy
- Alerts
- Health

### 14.3 Overview

The overview page should answer:

- Is the node connected, degraded, stale, expired, or air-gapped?
- Which trust bundle is active?
- Is the bundle signature verified?
- Are compliance modules enabled?
- Is audit integrity clean?

### 14.4 Audit Page

The audit page lets an operator or regulator search by:

- tenant
- module
- route decision
- date range
- correlation ID

The critical product moment is correlation. A reviewer should be able to trace:

> credential event -> policy decision -> MAI request -> report evidence

### 14.5 Reports Page

The reports page lets operators generate and download compliance reports. Reports should include trust sections and certification signatures.

Report types include:

- HIPAA
- ITAR/EAR
- OCAP
- activity digest
- monthly/system reports

### 14.6 Policy Page

The policy page lets authorized operators enable/disable modules, reload policies, and apply templates.

Production hardening must ensure that these controls are permissioned, audited, and fail safely.

### 14.7 Alerts Page

The alerts page consumes live compliance feed events and displays relevant policy/trust/audit alerts.

Production alerting should include audit write failures, trust expiration, bundle verification failure, air-gap violation, and adapter health problems.

### 14.8 Dashboard Authentication

The dashboard has two gates:

- dashboard access token
- SDK/API token used to call `mai-api`

The local-dev default `dashboard-dev` is convenient for bring-up but must be rejected by the production ship profile.

---

## 15. Deployment and Profiles

Deployment profiles live under `deployment/`. They define the runtime posture of a node.

Current profile categories include:

- local development
- cloud trust core
- local MAI node
- air-gap demo
- future ship/production profile from the hardening plan

### 15.1 Local Development

Local-dev should be easy:

- accept-all verifier permitted
- local token stub permitted
- relaxed paths
- developer-friendly startup

But local-dev behavior must be impossible to confuse with production.

### 15.2 Air-Gap Demo

Air-gap demo proves disconnected operation and restricted routing. It is a demo posture, not automatically a full production posture.

### 15.3 Local MAI Node

This is closer to field topology. It represents a node with local inference and trust-cache behavior, potentially connected to an external trust core.

### 15.4 Ship Profile

The Ship Hardening Plan calls for a strict profile that refuses:

- stubs
- in-memory critical state
- accept-all verifiers
- null sealers
- dev dashboard tokens
- synthetic trust exchange
- missing trust anchors
- missing persistent paths
- unverifiable audit state

The ship profile is the difference between "the stack can demonstrate the product" and "the product can be shipped."

---

## 16. Observability and Operations

MAI has health and telemetry surfaces today. The hardening lane turns these into production operations.

### 16.1 Health

Health should distinguish:

- process alive
- API ready
- degraded but safe
- unsafe for production

This is more precise than one `/health` boolean.

### 16.2 Metrics

Important metrics include:

- request count
- request latency
- scheduler decision latency
- queue depth
- adapter health
- adapter restart count
- audit write failures
- trust bundle age
- trust state
- policy decisions by module
- rate limit events
- GPU memory pressure
- KV cache pressure
- report generation count

### 16.3 Logs

Production logs must be structured, rotated, and redacted. They must not contain:

- raw API keys
- prompts
- completions
- embeddings
- raw PHI
- OpenBao tokens
- secret key material

### 16.4 Alerts

Operational alerts should fire for:

- audit chain break
- audit write failure
- trust bundle expired
- verifier missing
- vault unavailable
- no healthy backend
- adapter crash loop
- air-gap violation
- disk near full
- policy reload failure
- production guard violation

---

## 17. Backup, Restore, and Recovery

The product is not truly shippable until a field operator can recover it.

Critical state includes:

- vault storage
- model registry
- API audit WAL
- compliance audit WAL
- trust bundle cache
- revocation snapshots
- auth key hashes
- reports
- config checksums

Recovery must verify:

- backup manifest signature
- file checksums
- audit chain continuity
- trust bundle signatures
- vault seal state
- config compatibility

A restored node should pass the same ship validation command as a fresh production node.

---

## 18. CI, Burn-In, and Release Gates

The current repo has strong local tests and CI structure. The production hardening lane turns this into release discipline.

Required release gates:

- Rust compile
- clippy
- rustfmt
- cargo test workspace
- Python lint
- Python strict typing
- Python tests
- API integration tests
- compliance integration tests
- ship validator
- package build
- backup/restore drill
- GPU integration on self-hosted runner
- benchmark regression
- 72-hour burn-in

The release process should produce evidence artifacts:

- build metadata
- commit hash
- test reports
- benchmark report
- burn-in report
- ship validation output
- known deferrals

---

## 19. Request Path Walkthrough

This is the end-to-end path for a regulated inference request.

1. A client calls the Python SDK or REST/gRPC endpoint with an API key.
2. `mai-api` authenticates the key and attaches profile context.
3. Rate limiting checks the key's request budget.
4. The request body is parsed without logging sensitive payload content.
5. Router/classifier determines whether sensitive entities or route constraints may apply.
6. Lamprey policy modules evaluate HIPAA, ITAR/EAR, OCAP, and trust context.
7. Policy composer resolves conflicts and produces a route decision.
8. Trust cache contributes bundle version, claim, revocation, offline, and service identity context.
9. Audit correlation fields are prepared.
10. Scheduler receives only eligible placement options.
11. Scheduler evaluates topology, KV cache, batching, load, health, and power state.
12. Adapter manager sends the request to the selected backend.
13. Backend returns tokens or embeddings through adapter IPC.
14. Streaming response flows back through SSE/WebSocket or normal JSON response.
15. API audit records request metadata and status without storing prompts/completions.
16. Compliance audit records policy decision and correlation evidence.
17. Dashboard/reporting can later retrieve and verify the decision trail.

The core guarantee is that policy and trust decide before inference, not after.

---

## 20. What Makes the Stack Defensible

The stack's defensibility comes from the combination, not any single module.

### 20.1 Local-First Inference

The model runs near the data. Regulated payloads do not need to leave the appliance.

### 20.2 Governance Before Inference

Policy decisions happen before the backend sees the request.

### 20.3 Multi-Domain Compliance

HIPAA, ITAR/EAR, and OCAP can be evaluated together, with deterministic conflict resolution.

### 20.4 OCAP as First-Class Policy

Tribal data sovereignty is not a footnote or tag. It is represented as a policy pipeline and audit concern.

### 20.5 Offline Trust

The node can operate from locally verified trust bundles under defined degraded modes.

### 20.6 Tamper-Evident Audit

Decisions, claims, routes, and reports are tied to verifiable audit evidence.

### 20.7 Backend Neutrality

Adapters make inference engines replaceable. The governance boundary remains stable.

### 20.8 Hardware Abstraction

The HIL and scheduler keep the stack ready for heterogeneous and future hardware.

---

## 21. Remaining Hardening Boundary

The Ship Hardening Plan identifies the main transition still needed:

> Convert demo-safe defaults into production-failing invariants.

The stack already has many serious parts. The final product step is making sure production cannot accidentally run with:

- stub vault
- memory audit
- accept-all verifier
- null sealer
- dev dashboard token
- local-dev token exchange
- missing persistent state
- missing trust anchors
- missing backup/restore plan

Once those are impossible under the ship profile, Lamprey MAI becomes not only architecturally substantial, but operationally sea-worthy.

---

## 22. Final Framing

Lamprey MAI is best understood as a sovereign AI control plane that happens to include local inference, not as an inference server that happens to include compliance.

The back-end foundation protects local state, schedules compute, manages adapters, and exposes stable APIs. The Lamprey middle layer turns every request into a governed event with trust context, policy decisions, and audit proof. The SDKs and applications make that boundary usable. The dashboard makes it inspectable by operators, compliance officers, regulators, and buyers.

The result is a stack designed for organizations that cannot treat AI as a stateless web app. They need locality, proof, policy, recovery, and trust. Lamprey MAI is the architecture that binds those pieces into one product.
