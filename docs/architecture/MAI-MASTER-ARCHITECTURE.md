# MAI Master Architecture Specification

**Project:** Island Mountain Model Abstraction Interface (MAI)
**Version:** 1.0.0-spec
**Architecture Model:** Tock Kernel (adapted for AI inference abstraction)
**Source:** IM 48 Month Plan, May 2026
**Session:** 01 (Root specification)
**Classification:** Island Mountain AI / Confidential

---

## Table of Contents

1. Executive Summary
2. System Context and Scope
3. Tock-to-MAI Architecture Map
4. Trust Model
5. Six-Layer IM-OS Stack
6. Component Catalog
7. Data Flow Diagrams
8. Interface Contract Index
9. Error Taxonomy and Failure Propagation
10. Security Model
11. Hardware Capability Negotiation Protocol
12. Power State Machine
13. Model Registry Schema
14. Technology Choices and Rationale
15. Product Tier Hardware Profiles
16. Air-Gap Architecture
17. Telemetry and Observability
18. Configuration Architecture
19. Deployment Model
20. Session Dependency Graph
21. Glossary

---

## 1. Executive Summary

The Model Abstraction Interface (MAI) is the core inference abstraction layer for IM-OS, Island Mountain's sovereign data and identity operating system. It occupies Layer 3 of the six-layer IM-OS stack, sitting between all application logic above it (L4-L6) and all inference hardware below it (L1-L2).

The MAI's single purpose: make the inference engine a replaceable plugin while the data sovereignty layer remains the product. When a Scout unit ships with an RTX 5090 running Ollama in 2026, the MAI abstracts that hardware. When that same unit receives a TetraMem MX100 classical memristor card in 2028, the MAI abstracts that too. When quantum memristor SoCs arrive in 2030+, the MAI abstracts those. Nothing above Layer 3 changes. Not one line of application code. Not one API endpoint. Not one user-facing feature.

This is the engineering moat. It is not clever. It is not novel. It is a direct adaptation of a proven architecture (the Tock microcontroller kernel's layered trust model) applied to AI inference abstraction. The Tock kernel has survived six years of production deployment on embedded hardware by enforcing rigid trust boundaries between kernel code, driver capsules, and user processes. The MAI applies the same discipline at a different scale.

### What the MAI Is

The MAI is a thin, stable API surface with a trusted core kernel that schedules inference requests across backend adapters, manages hardware through a typed Hardware Interface Layer (HIL), controls power states for appliance-grade energy behavior, and maintains a model registry for zero-downtime model management. All of this runs air-gapped by default, with post-quantum cryptography protecting all data at rest.

### What the MAI Is Not

The MAI is not an inference engine. It does not run models. It does not train models. It does not serve tokens. It dispatches requests to inference engines (Ollama, vLLM, llama.cpp, TGI, TensorRT-LLM, ExLlamaV2, SGLang) through sandboxed adapter capsules and returns results through a stable API. If every inference engine on the planet disappeared tomorrow, the MAI would still compile, still boot, and still report "no adapters available" through its health endpoint.

The MAI is not a UI framework. Layer 6 (the React dashboard) is explicitly out of scope for this build. The MAI exposes REST, gRPC, and streaming endpoints. What renders on screen is someone else's problem.

The MAI is not a cloud service. It runs on hardware the customer physically possesses. It never phones home. It never transmits telemetry. It never requires an internet connection. Air-gap is not a mode. It is the default.

---

## 2. System Context and Scope

### Build Scope

This build covers L3 (MAI core) and defines the interface contracts into L1/L2 (hardware and vault) and L4/L5 (agent and applications). It does NOT build the UI (L6), full agent logic (L4 internals), or full application logic (L5 internals, only scaffolds).

Specifically:

**In scope:**
- MAI core kernel (scheduler, registry, power state machine, health monitor, hot-swap manager)
- Hardware Interface Layer (HIL) with traits and drivers for NVIDIA, AMD, CPU, TetraMem (stub)
- Backend adapter framework with PyO3 FFI bridge
- Seven backend adapters (Ollama, vLLM, llama.cpp, TGI, TensorRT-LLM, ExLlamaV2, SGLang)
- API server (REST via axum, gRPC via tonic, SSE and WebSocket streaming)
- Vault integration interface (ZFS, PQC encryption, TPM key management, audit trail, profile store)
- Agent/RAG interface (context management, RAG pipeline hooks, tool calling, speech-to-text handoff)
- Sleep mode and power state machine (Deep Vault Sleep, Sentinel, Full Inference)
- Model management and OTA update pipeline (air-gap safe)
- Seven L4-L5 application integration scaffolds
- Integration test suite and system validation
- Deployment packaging (Debian, systemd, Docker compose, first-boot automation)

**Out of scope:**
- L6 React dashboard and onboarding wizard
- Remote support tunnel (network service, not MAI concern)
- Landfall Council collaborative workspace (deferred to post-v1)
- Full L4 agent internals (RAG pipeline implementation, tool implementations)
- Full L5 application logic (only integration scaffolds with MAI API)
- TetraMem adapter implementation (stub interface only, hardware unavailable until 2028)
- Photonic adapter implementation (stub interface only)

### External Dependencies

The MAI depends on the following external systems at runtime:

| Dependency | Required | Purpose |
|---|---|---|
| Linux kernel (Debian/Ubuntu) | Yes | Base OS, cgroups, systemd |
| ZFS | Yes | Encrypted vault storage |
| TPM 2.0 | Yes (production) | Key management, attestation |
| At least one inference backend | Yes | Actual model execution |
| NVIDIA drivers (NVML) | Conditional | NVIDIA GPU hardware support |
| ROCm | Conditional | AMD GPU hardware support |
| Qdrant | Yes | Local vector database for RAG |
| SQLite | Yes | Family profile store |

Note: "Required" means the MAI will not pass its own health check without it. "Conditional" means the MAI functions without it (with reduced capability).

---

## 3. Tock-to-MAI Architecture Map

### The Tock Kernel Model

Tock is a secure embedded operating system for microcontrollers. Its architecture enforces three trust tiers through language-level guarantees (Rust's type system and ownership model) and OS-level isolation (processes, capsules, and the kernel):

1. **Kernel (trusted):** Written in safe Rust (no unsafe in the core). Manages scheduling, memory, and system calls. The smallest possible trusted computing base.

2. **Capsules (semi-trusted):** Kernel extensions that implement device drivers and system services. Written in Rust, cannot use unsafe, cannot allocate heap memory. If a capsule panics, the kernel catches it. Capsules communicate with hardware through typed HIL traits.

3. **Processes (untrusted):** User-space applications compiled against a stable system call interface. Cannot address kernel memory. Cannot bypass the syscall layer. Crash-isolated from each other and from the kernel.

### The MAI Adaptation

The MAI maps Tock's three-tier trust model to AI inference abstraction:

| Tock Concept | MAI Equivalent | Trust Level | Language |
|---|---|---|---|
| Kernel | MAI Core (scheduler, registry, power, health, hotswap) | Trusted | Rust (no unsafe) |
| Peripheral Drivers | HIL hardware drivers (NVIDIA, AMD, CPU, TetraMem) | Trusted | Rust (unsafe in drivers only) |
| HIL Traits | HIL traits (HardwareProbe, PowerStateController, MemoryManager, SecureLoadContext) | Trusted | Rust |
| Capsules | Backend adapters (Ollama, vLLM, llama.cpp, etc.) | Untrusted | Python via PyO3 |
| Syscall Interface | MAI API (REST + gRPC + Streaming) | Boundary | Rust (axum + tonic) |
| Processes | IM-OS Applications (Summit Chat, Scribe, etc.) | Untrusted | Python |

### System Context Diagram

```
+---------------------------------------------------------------------+
|  L5-L6  APPLICATIONS (Tock: Processes)                    UNTRUSTED |
|  Legacy Engine, Scribe, FamilyVault AI, MedRecord,                  |
|  Summit Chat, HomeBase, Agent Orchestrator, Estate AI               |
|  Isolated by API contract. Cannot touch inference directly.         |
+--------------------------- MAI API ---------------------------------+
|  L3-L4  SYSCALL INTERFACE (Tock: Syscall)                 BOUNDARY  |
|  REST + gRPC + Streaming SSE/WebSocket + Python/Rust SDK            |
|  Stable contract. Apps compiled against this. Never breaks.         |
+---------------------------------------------------------------------+
|  L3     BACKEND ADAPTERS (Tock: Capsules)                 UNTRUSTED |
|  Ollama, vLLM, llama.cpp, TGI, TensorRT-LLM, ExLlamaV2,          |
|  SGLang, TetraMem SDK (future), Photonic adapter (future)          |
|  Sandboxed. No direct hardware access. Crash-isolated.              |
|  Talk to hardware ONLY through the HIL.                             |
+-------------------- HIL (Capability Layer) -------------------------+
|  L3     MAI CORE KERNEL (Tock: Core Kernel)               TRUSTED   |
|  Model Scheduler, Power State Machine, Model Registry,              |
|  Health Monitor, Sleep Mode Controller, Hot-Swap Manager            |
+-------------------- HIL (Hardware Interface) -----------------------+
|  L1-L2  HARDWARE DRIVERS (Tock: Peripheral Drivers)       TRUSTED   |
|  CUDA/ROCm detection, GPU memory manager,                          |
|  TPM 2.0 key ops, Air-gap daemon, Thermal monitor                  |
+---------------------------------------------------------------------+
|  L1     HARDWARE (Tock: Hardware)                         PHYSICAL  |
|  NVIDIA H100/H200/RTX 5090, AMD MI300X/RX 9090 XT,                |
|  TetraMem MX100 (2028+), Quantum memristor SoC (2030+)            |
|  Air-gap switch, TPM 2.0, ZFS vault                                |
+---------------------------------------------------------------------+
```

### Why Tock and Not Something Else

The choice of Tock as the structural model is deliberate. Other architectural patterns were considered and rejected:

**Microservices (rejected):** Microservices assume network communication between components. The MAI runs air-gapped. Network between components introduces latency, failure modes, and attack surface that are unnecessary when everything runs on the same physical machine.

**Plugin architectures (e.g., VSCode extensions) (rejected):** Plugin architectures typically share a process with the host. A crashing plugin takes down the host. The MAI needs crash isolation between adapters so that a segfault in vLLM's CUDA code does not bring down the scheduler.

**Hypervisor model (rejected):** Full VM isolation per adapter would provide the strongest isolation but at enormous overhead. A 350W GPU inference system cannot afford hypervisor overhead on the inference hot path.

**Tock (selected):** Tock gives us typed capability-based isolation (HIL traits), crash boundaries (capsule panic catching), a minimal trusted computing base (safe Rust kernel with no unsafe), and process isolation (API syscall interface), all within a single-machine runtime with microsecond-level overhead. It is the right structural model.

---

## 4. Trust Model

### Trust Zones

The MAI defines three trust zones with clear, enforced boundaries:

#### Zone 1: Trusted (MAI Core + HIL Drivers)

**Language:** Rust
**Unsafe policy:** Zero unsafe in mai-core. Unsafe permitted in mai-hil driver implementations only (direct hardware access requires it).
**Crash policy:** If trusted code panics, the system is in an undefined state. This must never happen. All error paths are explicit via `Result<T, E>` with `thiserror` derive macros. No `unwrap()` in production paths.
**Access:** Can read/write hardware registers, manage GPU memory, seal/unseal TPM keys, read vault data, control power states.

Components in this zone:
- mai-core: scheduler, registry, power state machine, health monitor, hot-swap manager, vault interface
- mai-hil: HIL trait definitions, NVIDIA driver, AMD driver, CPU driver, TetraMem stub
- mai-api: REST server, gRPC server, streaming endpoints, auth middleware, audit middleware

#### Zone 2: Untrusted Adapters (Backend Capsules)

**Language:** Python (via PyO3 FFI bridge from Rust)
**Unsafe policy:** Cannot use unsafe. The PyO3 bridge is the only communication channel.
**Crash policy:** If an adapter crashes, the AdapterManager (Rust, trusted) catches the failure, logs it, and restarts the adapter with exponential backoff. No request reaches hardware without passing through the HIL. A crashing adapter cannot corrupt the scheduler, registry, or any other core component.
**Access:** Can call inference backends via HTTP/IPC. Cannot access hardware directly. Cannot read vault data directly. Cannot modify the registry. Cannot bypass the scheduler.
**Isolation mechanism:** Each adapter runs in its own process with cgroups resource limits (CPU, memory, IO). The AdapterManager communicates with adapter processes through the PyO3 FFI bridge.

Components in this zone:
- adapters/ollama/
- adapters/vllm/
- adapters/llamacpp/
- adapters/tgi/
- adapters/tensorrt/
- adapters/exllamav2/
- adapters/sglang/

#### Zone 3: Untrusted Applications (IM-OS Processes)

**Language:** Python (using mai-sdk-python) or Rust (using mai-sdk-rs)
**Unsafe policy:** N/A (applications are external processes)
**Crash policy:** If an application crashes, only that application is affected. The MAI continues serving other applications. The API server's connection handling ensures a crashed client's TCP connection is cleaned up without affecting the server.
**Access:** Can call MAI API endpoints (REST, gRPC, streaming). Cannot bypass the API. Cannot address GPU memory. Cannot read model weights. Cannot modify the registry directly. Cannot access hardware.

Components in this zone:
- apps/summit-chat/
- apps/familyvault/
- apps/scribe/
- apps/legacy-engine/
- apps/medrecord/
- apps/homebase/
- apps/estate-ai/

### Trust Boundary Enforcement Rules

These rules are non-negotiable. A violation of any rule fails the session's acceptance criteria.

1. **No adapter may access hardware directly.** All hardware access goes through HIL traits. There are no exceptions. An adapter that needs GPU VRAM information asks the scheduler, which queries the MemoryManager HIL trait, which reads NVML. The adapter never sees NVML.

2. **No application may bypass the API.** The syscall interface (REST/gRPC) is the only path to inference. No application can link against mai-core, import mai-hil, or call adapter code directly.

3. **Backend errors never leak to applications.** All backend-specific errors (Ollama HTTP 500, vLLM CUDA OOM, llama.cpp file not found) are wrapped in MAI error types before crossing the adapter boundary. Applications see `AdapterError::OutOfMemory`, never `cudaErrorMemoryAllocation`.

4. **No unsafe in mai-core.** All unsafe Rust is confined to mai-hil driver implementations. The HIL trait definitions themselves contain no unsafe. This is verified by CI.

5. **The API contract never breaks backward compatibility.** Applications are compiled against it. Breaking it breaks every application above L3. If an endpoint must change, add a versioned replacement (e.g., /v2/) and deprecate the old one with a sunset header. Never remove.

---

## 5. Six-Layer IM-OS Stack

### Layer 1: Hardware (PHYSICAL)

The physical compute substrate. In the GPU era (2026-2028), this is NVIDIA and AMD GPUs. In the memristor transition era (2028-2030), this adds TetraMem MX100 classical memristors alongside GPUs. In the quantum era (2030+), this adds quantum memristor SoCs.

**Components:** GPU cards, air-gap hardware switch, TPM 2.0 chip, ZFS storage arrays, thermal sensors, power supply units, Wake-on-LAN NIC.

**MAI interaction:** The MAI accesses L1 exclusively through HIL driver implementations in mai-hil. No other MAI component touches hardware.

### Layer 2: Vault (INTERFACE ONLY for this build)

Encrypted storage for all persistent data: model weights, family profiles, audit logs, vector embeddings, medical records, documents, photos, memories.

**Components:** ZFS encrypted datasets, ML-KEM (Kyber-1024) encryption at rest, ML-DSA (Dilithium-87) digital signatures, TPM 2.0 sealed master key, SQLite family profile database, Qdrant vector database, append-only hash-chained audit log.

**MAI interaction:** The MAI defines a vault interface in mai-core (Session 12) that abstracts ZFS operations, PQC encryption, profile queries, audit writes, and vector DB operations. The MAI does not implement the vault itself. It implements the interface that applications use to request vault operations through the MAI API.

### Layer 3: Inference / MAI (THIS BUILD)

The complete inference abstraction layer. This is what the 18 sessions build.

**Components:** MAI core kernel (scheduler, registry, power, health, hotswap), HIL (traits + drivers), backend adapters (7 GPU-era + 2 future stubs), API server (REST + gRPC + streaming), vault interface, agent/RAG interface.

### Layer 4: Agent (INTERFACE ONLY for this build)

The agent layer sits between the MAI API and the application layer. It provides context management, RAG pipeline orchestration, tool calling, speech-to-text, and agentic task management.

**MAI interaction:** The MAI defines agent/RAG interface endpoints in Session 13. Applications call these endpoints to interact with the RAG pipeline, invoke tools, and manage context windows. The actual RAG pipeline internals and tool implementations are out of scope.

### Layer 5: Applications (SCAFFOLDS ONLY for this build)

End-user applications that consume the MAI API (directly or through the agent layer).

**Seven application scaffolds built in Session 16:**
- Summit Chat: streaming multi-turn conversation with family profile context
- FamilyVault AI: CLIP embedding, semantic photo/document search, face recognition hooks
- Landfall Scribe: RAG-augmented document drafting and editing
- Legacy Engine: oral history capture, speech-to-text, knowledge graph construction
- MedRecord Vault: HIPAA-aware medical document parsing and secure storage
- HomeBase: Matter/Thread smart home device control via Sentinel model
- Estate AI: digital asset cataloging and beneficiary access management

### Layer 6: UI (OUT OF SCOPE)

React dashboard, onboarding wizard, mobile companion. Not part of this build.

---

## 6. Component Catalog

### 6.1 Trusted Components (Rust)

#### mai-core: Model Scheduler (`scheduler.rs`)

**Purpose:** Routes inference requests to the correct adapter and model based on request requirements, available hardware, loaded models, and scheduling strategy.

**Responsibilities:**
- Accept inference requests from the API server
- Evaluate request against loaded models and adapter capabilities
- Select optimal adapter + model combination
- Distribute requests across multiple GPUs when available
- Implement scheduling strategies: round-robin, least-loaded, model-affinity, priority-based
- Trigger Sentinel-to-Full Inference promotion when request exceeds Sentinel capability
- Queue management with priority levels derived from family profiles
- Rate limiting per profile
- Request timeout enforcement

**Key types:**
- `InferenceRequest`: incoming request with model preference, parameters, priority
- `SchedulingDecision`: selected adapter, model, GPU assignment
- `SchedulingStrategy`: enum of available routing strategies
- `RequestQueue`: priority queue with per-profile rate limiting

**Interfaces consumed:** Registry (model state), HealthMonitor (adapter liveness), PowerStateMachine (current power state), AdapterManager (dispatch)
**Interfaces provided to:** API Server (request intake), Core kernel internal bus

#### mai-core: Model Registry (`registry.rs`)

**Purpose:** Tracks all known models, their current state (downloaded, loading, loaded, active, unloading, error), and their requirements (VRAM, compute type, quantization format).

**Responsibilities:**
- Parse model manifests (TOML format)
- Track model lifecycle state machine: `Unknown -> Downloaded -> Loading -> Loaded -> Active -> Unloading -> Downloaded`
- Version management for model packages
- Dependency resolution (model X requires adapter Y at version >= Z)
- Air-gap aware update checking (USB model packages, no network)
- Model compatibility matrix (which models run on which hardware tiers)
- VRAM budget tracking across loaded models

**Key types:**
- `ModelManifest`: parsed TOML manifest with model metadata
- `ModelState`: enum of lifecycle states
- `ModelRequirements`: VRAM, compute type, quantization, adapter compatibility
- `ModelRegistry`: the registry itself, thread-safe with interior mutability

**Interfaces consumed:** MemoryManager HIL (VRAM availability), Vault interface (model storage)
**Interfaces provided to:** Scheduler (model selection), HotSwap (model replacement), API Server (model listing)

#### mai-core: Power State Machine (`power.rs`)

**Purpose:** Controls the device's power state transitions between Off, Deep Vault Sleep, Sentinel, Full Inference, and Thermal Throttle.

**Responsibilities:**
- Enforce valid state transitions (see state machine diagram in Section 12)
- Manage auto-demotion timers (Full Inference to Sentinel after 12 minutes idle, configurable)
- Manage extended inactivity demotion (Sentinel to Deep Vault Sleep after 2 hours, configurable)
- Coordinate GPU power state changes through HIL PowerStateController
- Handle thermal throttle events from hardware
- Support schedule-based power profiles (e.g., always Sentinel during business hours)
- Signal wake events to trigger state transitions
- Coordinate with Scheduler for Sentinel-to-Full promotion

**Key types:**
- `PowerState`: enum {Off, DeepVaultSleep, Sentinel, FullInference, ThermalThrottle}
- `PowerTransition`: validated transition request with source, target, reason
- `PowerProfile`: per-tier defaults + user overrides
- `DemotionTimer`: configurable inactivity timer

**Interfaces consumed:** PowerStateController HIL (hardware power control), HealthMonitor (thermal data)
**Interfaces provided to:** Scheduler (promotion requests), API Server (power state queries), Health Monitor (state reporting)

#### mai-core: Health Monitor (`health.rs`)

**Purpose:** Continuous monitoring of adapter health, hardware telemetry, and system integrity.

**Responsibilities:**
- Adapter heartbeat monitoring (periodic health check calls through AdapterManager)
- Hardware telemetry collection via HIL (GPU temperature, VRAM usage, power draw, fan speed)
- Alert escalation: healthy -> degraded -> critical -> failed
- Air-gap verification (periodic check that no network interfaces are active when air-gap expected)
- Telemetry storage (local-only, never transmitted)
- Health endpoint data for API server
- Adapter restart recommendations to AdapterManager

**Key types:**
- `HealthStatus`: enum {Healthy, Degraded, Critical, Failed}
- `AdapterHealth`: per-adapter health state with last heartbeat timestamp
- `HardwareTelemetry`: GPU temp, VRAM used/total, power draw, fan RPM
- `SystemHealth`: aggregate of all adapter and hardware health

**Interfaces consumed:** AdapterManager (adapter heartbeat), HIL HardwareProbe (hardware telemetry)
**Interfaces provided to:** API Server (/health endpoint), PowerStateMachine (thermal alerts), Scheduler (adapter availability)

#### mai-core: Hot-Swap Manager (`hotswap.rs`)

**Purpose:** Zero-downtime replacement of models and adapters with automatic rollback on failure.

**Responsibilities:**
- Model hot-swap: load new model version, verify it serves correctly, swap routing, unload old version
- Adapter hot-swap: start new adapter process, verify health, swap routing, stop old process
- Rollback protocol: if new version fails health check within grace period, revert to previous
- Coordination with Scheduler to drain in-flight requests before swap
- Coordination with Registry to update model state
- Swap audit logging

**Key types:**
- `SwapRequest`: what to swap (model or adapter), old version, new version
- `SwapState`: enum {Preparing, Loading, Verifying, Swapping, RollingBack, Complete, Failed}
- `RollbackTrigger`: conditions that trigger automatic rollback

**Interfaces consumed:** Registry (model state), Scheduler (request draining), AdapterManager (adapter lifecycle)
**Interfaces provided to:** API Server (swap initiation), Model Management (OTA updates)

#### mai-core: Vault Interface (`vault.rs`)

**Purpose:** Abstraction over L2 vault operations. The MAI does not implement the vault. It provides a typed interface that the API server and agent layer use to request vault operations.

**Responsibilities:**
- Model weight storage and retrieval (ZFS datasets)
- PQC encryption and decryption interface (ML-KEM key encapsulation, ML-DSA signatures)
- TPM 2.0 key seal/unseal operations
- Family profile CRUD operations (SQLite)
- Audit trail append (hash-chained, tamper-evident)
- Qdrant vector database operations (embedding storage, similarity search)
- Compliance audit data export

**Key types:**
- `VaultOperation`: enum of vault operations
- `EncryptedPayload`: PQC-encrypted data with key ID and algorithm
- `AuditEntry`: timestamped, signed, hash-chained log entry
- `FamilyProfile`: profile data with access permissions

#### mai-hil: HIL Trait Definitions

**Purpose:** Typed interface between the MAI core and hardware-specific driver implementations. This is the hardware abstraction boundary.

**Four core traits (specified in Session 02):**

1. `HardwareProbe`: GPU/accelerator detection, enumeration, capability reporting
2. `PowerStateController`: power state transitions, current draw reporting, wake latency, thermal signals
3. `MemoryManager`: VRAM allocation, model memory mapping, OOM signaling, shared memory
4. `SecureLoadContext`: TPM-attested model loading, encrypted weight transfer, integrity verification

**Design principle:** Every trait method documents its contract, error conditions, and latency guarantees. Methods are async where hardware latency is expected (e.g., VRAM allocation, power state transitions). Methods are sync where the answer is cached (e.g., capability reporting after initial probe).

#### mai-hil: Hardware Drivers

**NVIDIA CUDA Driver (`nvidia.rs`):**
- Uses NVML (NVIDIA Management Library) for GPU detection and management
- Supports H100 PCIe, H100 SXM5, H200, RTX 5090
- Implements all four HIL traits
- VRAM tracking with per-model allocation accounting
- Power management via nvidia-smi persistence mode and power limit controls
- Thermal monitoring with throttle event detection
- Integration tests feature-gated behind `--features nvidia`

**AMD ROCm Driver (`amd.rs`):**
- Uses rocm-smi bindings for GPU detection and management
- Supports MI300X, RX 9090 XT
- Implements all four HIL traits
- Integration tests feature-gated behind `--features amd`

**CPU Fallback Driver (`cpu.rs`):**
- Detects AVX-512 and other SIMD capabilities
- Provides degraded-mode compute target
- Always available (no feature gate)
- Reports reduced capability through HardwareProbe

**TetraMem Stub Driver (`tetramem_stub.rs`):**
- Compiles successfully
- All trait methods return `Err(HilError::NotImplemented)`
- Interface is designed for future memristor hardware
- Placeholder for 2028+ integration

#### mai-adapters: Adapter Framework (Rust side)

**AdapterManager:** The Rust-side manager that spawns, monitors, and communicates with Python adapter processes.

**Responsibilities:**
- Spawn adapter processes with cgroups resource limits
- Communicate through PyO3 FFI bridge
- Monitor adapter health via heartbeat
- Restart failed adapters with exponential backoff
- Route inference requests from Scheduler to correct adapter process
- Collect streaming tokens from adapter and forward to Scheduler

**PyO3 FFI Bridge:** Typed interface between Rust AdapterManager and Python AdapterBase. All data crossing this boundary is serialized through well-defined types. No raw pointers. No shared mutable state.

#### mai-api: API Server

**REST Server (axum):**
- OpenAPI 3.1 compliant endpoints
- /v1/chat/completions (streaming and non-streaming)
- /v1/completions (text completion)
- /v1/embeddings (vector embedding)
- /v1/models (model listing and management)
- /v1/health (system health)
- /v1/power (power state queries and transitions)
- /v1/admin/* (configuration, registry management)

**gRPC Server (tonic):**
- Proto3 service definitions mirroring REST endpoints
- Bidirectional streaming for chat
- Server-side streaming for completions

**Streaming:**
- SSE (Server-Sent Events) for REST streaming endpoints
- WebSocket for bidirectional communication (agent layer, speech-to-text)

**Middleware:**
- Authentication: X-IM-Profile header, local-only family profile system
- Audit: append-only logging of all API calls to vault
- Air-gap verification: startup check that no unexpected network interfaces are active
- Rate limiting: per-profile configurable limits

### 6.2 Untrusted Components (Python)

#### AdapterBase (Python abstract class)

All backend adapters inherit from `AdapterBase` and implement its abstract methods:

```python
class AdapterBase(ABC):
    @abstractmethod
    async def chat(self, request: ChatRequest) -> AsyncGenerator[TokenEvent, None]: ...

    @abstractmethod
    async def complete(self, request: CompletionRequest) -> AsyncGenerator[TokenEvent, None]: ...

    @abstractmethod
    async def embed(self, request: EmbeddingRequest) -> EmbeddingResponse: ...

    @abstractmethod
    async def load_model(self, manifest: ModelManifest) -> LoadResult: ...

    @abstractmethod
    async def unload_model(self, model_id: str) -> UnloadResult: ...

    @abstractmethod
    async def health_check(self) -> HealthStatus: ...

    @abstractmethod
    def capabilities(self) -> AdapterCapabilities: ...
```

Adapters self-register using the `@mai_adapter` decorator:

```python
@mai_adapter(name="ollama", version="1.0")
class OllamaAdapter(AdapterBase):
    ...
```

#### Individual Adapters

Each adapter is specified in detail in Session 03 and implemented in Sessions 08-09. Summary:

| Adapter | Backend | Primary Use Case | Key Feature |
|---|---|---|---|
| Ollama | Ollama REST API | Scout/Summit Base default | Simplest deployment, GPU layer assignment |
| vLLM | vLLM OpenAI-compatible API | Ranger/Pack Leader | Tensor parallelism across GPUs, LoRA |
| llama.cpp | llama-cpp-python | Lightweight fallback | GGUF format, grammar constraints |
| TGI | HuggingFace TGI API | Quantized models | Speculative decoding, AWQ/GPTQ |
| TensorRT-LLM | TensorRT-LLM API | H100/H200 maximum throughput | Engine build caching, inflight batching |
| ExLlamaV2 | ExLlamaV2 Python API | Quantized multi-model | EXL2/GPTQ, multi-model multiplexing |
| SGLang | SGLang API | Structured output | RadixAttention, constrained decoding |

### 6.3 Application Scaffolds (Python)

Seven application scaffolds are built in Session 16. Each scaffold demonstrates the MAI API integration pattern for its use case. Scaffolds include configuration templates, smoke tests, and integration tests. They are starting points, not finished applications.

---

## 7. Data Flow Diagrams

### 7.1 Inference Request Lifecycle

```
Application                API Server            Scheduler           AdapterMgr          Adapter         Backend
    |                          |                     |                   |                  |               |
    |-- POST /v1/chat -------->|                     |                   |                  |               |
    |                          |-- authenticate ----->|                   |                  |               |
    |                          |-- validate --------->|                   |                  |               |
    |                          |                     |-- check models -->|                   |               |
    |                          |                     |   (registry)      |                   |               |
    |                          |                     |                   |                   |               |
    |                          |                     |-- check power --->|                   |               |
    |                          |                     |   (if Sentinel,   |                   |               |
    |                          |                     |    promote?)      |                   |               |
    |                          |                     |                   |                   |               |
    |                          |                     |-- select adapter->|                   |               |
    |                          |                     |   + model + GPU   |                   |               |
    |                          |                     |                   |-- dispatch ------>|               |
    |                          |                     |                   |                  |-- HTTP/IPC --->|
    |                          |                     |                   |                  |               |
    |                          |                     |                   |                  |<-- tokens ----|
    |                          |                     |                   |<-- tokens -------|               |
    |                          |                     |<-- tokens --------|                  |               |
    |                          |<-- tokens ----------|                   |                  |               |
    |<-- SSE deltas -----------|                     |                   |                  |               |
    |                          |                     |                   |                  |               |
    |                          |-- audit log ------->| (vault)           |                  |               |
```

**Step-by-step:**
1. Application sends POST /v1/chat/completions with X-IM-Profile header
2. API Server authenticates the request against the family profile store
3. API Server validates request parameters (model preference, max tokens, temperature, etc.)
4. Request enters the Scheduler's priority queue (priority derived from family profile)
5. Scheduler checks the Registry for compatible loaded models
6. If in Sentinel mode and request exceeds Sentinel capability, Scheduler requests Full Inference promotion from Power State Machine (target: <8 seconds to first token)
7. Scheduler selects the best adapter + model + GPU combination based on current strategy
8. Scheduler dispatches to AdapterManager
9. AdapterManager forwards to the selected adapter process via PyO3 FFI bridge
10. Adapter calls its inference backend (Ollama HTTP API, vLLM API, etc.)
11. Backend streams tokens back to adapter
12. Tokens flow back: Backend -> Adapter -> AdapterManager -> Scheduler -> API Server
13. API Server streams SSE deltas to the application
14. On completion: audit log entry written to vault (profile ID, model used, token count, latency, timestamp)

### 7.2 Model Load/Unload Lifecycle

```
Trigger             Registry         MemoryMgr(HIL)      SecureLoad(HIL)     Vault           Adapter
   |                    |                 |                    |                |                |
   |-- load request --->|                 |                    |                |                |
   |                    |-- check VRAM -->|                    |                |                |
   |                    |                 |-- available? ----->|                |                |
   |                    |                 |   (if not: evict   |                |                |
   |                    |                 |    LRU model)      |                |                |
   |                    |                 |                    |                |                |
   |                    |-- verify ------>|                    |                |                |
   |                    |   integrity     |-- TPM attest ----->|                |                |
   |                    |                 |                    |-- unseal key ->|                |
   |                    |                 |                    |                |-- read model ->|
   |                    |                 |                    |                |   (encrypted)  |
   |                    |                 |                    |-- decrypt ---->|                |
   |                    |                 |                    |   (ML-KEM)     |                |
   |                    |                 |<-- load to VRAM ---|                |                |
   |                    |                 |                    |                |                |
   |                    |-- health check -|------------------------------------+--------------->|
   |                    |                 |                    |                |                |
   |                    |-- state: active |                    |                |                |
   |                    |-- audit log --->| (vault)            |                |                |
```

**Model state machine:**
```
Unknown -> Downloaded -> Loading -> Loaded -> Active -> Unloading -> Downloaded
                            |                              ^
                            +-- Error (rollback) ----------+
```

### 7.3 Sleep Mode Transition: Deep Vault Sleep to Full Inference

This is the most complex transition, triggered when a user request arrives while the system is in Deep Vault Sleep.

```
Wake Trigger      PowerState        Scheduler        HIL(Power)      HIL(Memory)      Registry       Adapter
    |                 |                 |                |                |                |              |
    |-- wake -------->|                 |                |                |                |              |
    |  (API request,  |                 |                |                |                |              |
    |   WoL, schedule)|                 |                |                |                |              |
    |                 |                 |                |                |                |              |
    |                 |-- transition -->|                |                |                |              |
    |                 |  DeepVault ->   |                |                |                |              |
    |                 |  Sentinel       |                |                |                |              |
    |                 |                 |-- GPU wake --->|                |                |              |
    |                 |                 |   (low power)  |                |                |              |
    |                 |                 |                |-- init VRAM -->|                |              |
    |                 |                 |                |                |                |              |
    |                 |                 |-- load ------->|                |                |              |
    |                 |                 |  Sentinel model|                |-- Phi-4-mini ->|              |
    |                 |                 |                |                |   (from vault)  |              |
    |                 |                 |                |                |                |-- start ---->|
    |                 |                 |                |                |                |  (Ollama)    |
    |                 |-- Sentinel ---->|                |                |                |              |
    |                 |   ready         |                |                |                |              |
    |                 |                 |                |                |                |              |
    |                 |  [if request    |                |                |                |              |
    |                 |   exceeds       |                |                |                |              |
    |                 |   Sentinel]     |                |                |                |              |
    |                 |                 |                |                |                |              |
    |                 |-- transition -->|                |                |                |              |
    |                 |  Sentinel ->    |                |                |                |              |
    |                 |  FullInference  |                |                |                |              |
    |                 |                 |-- GPU full --->|                |                |              |
    |                 |                 |   power        |                |                |              |
    |                 |                 |                |-- alloc VRAM ->|                |              |
    |                 |                 |-- load full -->|                |-- Qwen3 14B -->|              |
    |                 |                 |   model        |                |   (from vault)  |              |
    |                 |                 |                |                |                |-- load ----->|
    |                 |-- Full ready -->|                |                |                |              |
    |                 |                 |-- serve ------>|                |                |              |
    |                 |                 |   request      |                |                |              |
```

**Target latencies:**
- Deep Vault Sleep to Sentinel: <2 seconds (Phi-4-mini is small, stays partially cached)
- Sentinel to Full Inference: <8 seconds to first token (Qwen3 14B load + warm-up)

### 7.4 Auto-Demotion Flow

```
Last Request      Scheduler       DemotionTimer     PowerState       HIL(Power)       Registry
    |                |                 |                 |                |                |
    | (12 min idle)  |                 |                 |                |                |
    |                |-- no activity ->|                 |                |                |
    |                |                 |-- timer fires ->|                |                |
    |                |                 |                 |                |                |
    |                |                 |                 |-- demote ----->|                |
    |                |                 |                 |  Full ->       |                |
    |                |                 |                 |  Sentinel      |                |
    |                |                 |                 |                |-- reduce power |
    |                |                 |                 |                |                |
    |                |<-- unload ------|                 |                |-- unload ----->|
    |                |   full models   |                 |                |  (keep Sentinel|
    |                |                 |                 |                |   model loaded)|
    |                |                 |                 |                |                |
    | (2 hour idle)  |                 |                 |                |                |
    |                |                 |-- timer fires ->|                |                |
    |                |                 |                 |-- demote ----->|                |
    |                |                 |                 |  Sentinel ->   |                |
    |                |                 |                 |  DeepVaultSleep|                |
    |                |                 |                 |                |-- GPU sleep -->|
    |                |                 |                 |                |                |-- unload all |
```

---

## 8. Interface Contract Index

Every interface contract in the MAI is specified in one session and implemented in another. This table maps contracts to their specification and implementation sessions.

| Contract | Description | Specified In | Implemented In |
|---|---|---|---|
| HardwareProbe trait | GPU/accelerator detection and capability reporting | Session 02 | Session 06 |
| PowerStateController trait | Power state transitions, current draw, thermal signals | Session 02 | Session 06 |
| MemoryManager trait | VRAM allocation, OOM signaling, model memory mapping | Session 02 | Session 06 |
| SecureLoadContext trait | TPM-attested model loading, encrypted weight transfer | Session 02 | Session 06 |
| InferenceAdapter trait (Rust) | Typed adapter interface for Rust-side AdapterManager | Session 03 | Session 08 |
| AdapterBase class (Python) | Abstract base class for Python adapter implementations | Session 03 | Session 08 |
| PyO3 FFI bridge | Rust-to-Python typed communication channel | Session 03 | Session 08 |
| Scheduler internal API | Request routing, multi-GPU distribution, priority queues | Session 04 | Session 07 |
| Registry internal API | Model manifest parsing, state machine, versioning | Session 04 | Session 07 |
| PowerStateMachine internal API | State transitions, demotion timers, promotion protocol | Session 04 | Session 07 |
| HealthMonitor internal API | Heartbeat, telemetry collection, alert escalation | Session 04 | Session 07 |
| HotSwap internal API | Model/adapter replacement with rollback | Session 04 | Session 07 |
| REST API (OpenAPI 3.1) | All /v1/* endpoints | Session 05 | Session 11 |
| gRPC API (Proto3) | Inference and management services | Session 05 | Session 11 |
| SSE streaming protocol | Server-sent events for streaming completions | Session 05 | Session 11 |
| WebSocket protocol | Bidirectional streaming for agent/STT | Session 05 | Session 11 |
| Python SDK | mai-sdk-python package | Session 05 | Session 11 |
| Rust SDK | mai-sdk-rs crate | Session 05 | Session 11 |
| Auth/authz contract | X-IM-Profile header, family profile permissions | Session 05 | Session 11 |
| Vault interface | ZFS, PQC encryption, profiles, audit, vector DB | Session 12 | Session 12 |
| Agent/RAG interface | Context management, RAG pipeline, tool calling, STT | Session 13 | Session 13 |
| Model package format | .mai-pkg with PQC signatures | Session 15 | Session 15 |

---

## 9. Error Taxonomy and Failure Propagation

### 9.1 Error Categories

The MAI defines a strict error taxonomy organized by trust boundary. Errors never leak implementation details across trust boundaries.

#### Hardware Errors (L1, from HIL drivers)

```rust
pub enum HilError {
    /// Hardware not detected during probe
    DeviceNotFound { device_type: DeviceType },
    /// Driver version incompatible
    DriverVersionMismatch { required: String, found: String },
    /// GPU/accelerator out of memory
    OutOfMemory { requested_bytes: u64, available_bytes: u64 },
    /// Thermal limit exceeded, hardware throttling
    ThermalThrottle { current_temp_c: u32, limit_temp_c: u32 },
    /// Hardware fault detected (ECC error, driver crash)
    HardwareFault { description: String },
    /// Power state transition failed
    PowerTransitionFailed { from: PowerState, to: PowerState, reason: String },
    /// TPM attestation failed during secure load
    AttestationFailed { reason: String },
    /// Feature not implemented (future hardware stubs)
    NotImplemented,
    /// Communication with hardware timed out
    Timeout { operation: String, duration_ms: u64 },
}
```

#### Adapter Errors (L3, from backend adapters)

```rust
pub enum AdapterError {
    /// Adapter process crashed
    BackendCrashed { adapter_name: String, exit_code: Option<i32> },
    /// Backend is not responding
    BackendUnavailable { adapter_name: String },
    /// Inference request timed out
    Timeout { adapter_name: String, duration_ms: u64 },
    /// Backend ran out of GPU memory during inference
    OutOfMemory { adapter_name: String },
    /// Requested model not found in backend
    ModelNotFound { model_id: String },
    /// Request exceeds model's context window
    ContextExceeded { requested_tokens: u32, max_tokens: u32 },
    /// Backend rate limited the request
    RateLimited { adapter_name: String, retry_after_ms: u64 },
    /// Hardware fault reported by backend
    HardwareFault { adapter_name: String, description: String },
    /// Model loading failed
    ModelLoadFailed { model_id: String, reason: String },
    /// Configuration error in adapter
    ConfigError { adapter_name: String, detail: String },
}
```

#### Core Errors (L3, from MAI kernel)

```rust
pub enum CoreError {
    /// No adapter available for the requested model
    NoAdapterAvailable { model_id: String },
    /// No model loaded that can serve the request
    NoModelAvailable { requested_capability: String },
    /// Scheduler queue is full
    QueueFull { queue_depth: usize },
    /// Power state does not allow this operation
    InvalidPowerState { current: PowerState, required: PowerState },
    /// Model registry operation failed
    RegistryError { operation: String, reason: String },
    /// Hot-swap operation failed
    HotSwapFailed { reason: String },
    /// Internal timeout
    InternalTimeout { component: String, duration_ms: u64 },
}
```

#### API Errors (L3-L4 boundary, returned to applications)

```rust
pub enum ApiError {
    /// Authentication failed
    Unauthorized { reason: String },
    /// Profile does not have permission for this operation
    Forbidden { profile_id: String, operation: String },
    /// Requested resource not found
    NotFound { resource: String },
    /// Request validation failed
    BadRequest { details: Vec<ValidationError> },
    /// Rate limit exceeded for this profile
    RateLimited { profile_id: String, retry_after_seconds: u32 },
    /// Service temporarily unavailable (promoting power state, loading model)
    ServiceUnavailable { reason: String, retry_after_seconds: Option<u32> },
    /// Internal error (wrapped from CoreError or AdapterError)
    InternalError { request_id: String, message: String },
    /// Request timed out
    GatewayTimeout { request_id: String },
}
```

### 9.2 Failure Propagation Rules

**Rule 1: Errors wrap at every trust boundary.**

When an error crosses a trust boundary, it is wrapped in the receiving zone's error type. The original error details are logged but not forwarded. This prevents information leakage.

```
NVIDIA NVML error (HilError::HardwareFault)
  -> wrapped in CoreError::InternalTimeout or AdapterError::HardwareFault
    -> wrapped in ApiError::InternalError (only request_id + generic message visible to app)
```

**Rule 2: Backend-specific errors never reach applications.**

An application that receives `ApiError::InternalError` knows something went wrong. It does not know whether the error was a CUDA OOM, an Ollama HTTP 500, or a vLLM tensor mismatch. The application retries or fails gracefully. The details are in the audit log.

**Rule 3: Adapter crashes are caught and restarted.**

The AdapterManager runs in trusted Rust code. When an adapter process exits unexpectedly:
1. AdapterManager detects process exit
2. In-flight requests to that adapter receive `AdapterError::BackendCrashed`
3. Scheduler re-routes subsequent requests to alternative adapters (if available)
4. AdapterManager restarts the crashed adapter with exponential backoff (1s, 2s, 4s, 8s, max 60s)
5. After 5 consecutive crashes, adapter is marked as `Failed` and removed from routing

**Rule 4: Hardware faults trigger power state changes.**

If a HIL driver reports a hardware fault:
1. HealthMonitor marks the affected hardware as degraded
2. Scheduler stops routing to adapters on the affected hardware
3. If thermal: PowerStateMachine transitions to ThermalThrottle
4. If fatal (ECC uncorrectable): HealthMonitor marks hardware as Failed, Scheduler routes to remaining hardware

**Rule 5: Vault errors are critical.**

If the vault interface cannot decrypt model weights (TPM failure, corrupted data):
1. The model load fails with `CoreError::RegistryError`
2. Registry marks the model as `Error` state
3. HealthMonitor reports degraded system health
4. The system continues operating with already-loaded models
5. Audit log records the vault error (if audit trail itself is working)

### 9.3 Retry Semantics

| Error Type | Retryable | Strategy |
|---|---|---|
| AdapterError::Timeout | Yes | Retry with same adapter, max 2 attempts |
| AdapterError::BackendCrashed | Yes | Retry with different adapter if available |
| AdapterError::OutOfMemory | Conditional | Retry after model eviction if possible |
| AdapterError::RateLimited | Yes | Wait retry_after_ms, then retry |
| CoreError::QueueFull | Yes | Backpressure to API, return 503 with Retry-After |
| CoreError::InvalidPowerState | Yes | Wait for state transition, auto-retry |
| ApiError::ServiceUnavailable | Yes | Client retries after retry_after_seconds |
| HilError::ThermalThrottle | No | Wait for thermal recovery, no retry |
| HilError::HardwareFault | No | Route to different hardware, no retry on same |

---

## 10. Security Model

### 10.1 Threat Model

The MAI assumes the following threat landscape:

**Trusted threats (attacks from within the trusted zone):**
- Supply chain compromise of Rust dependencies: mitigated by cargo audit, vendored dependencies, reproducible builds
- Buggy HIL driver code: mitigated by Rust's type system, careful unsafe auditing, integration testing

**Untrusted adapter threats:**
- Malicious adapter attempts to access hardware directly: blocked by process isolation (separate PID, no GPU device file access)
- Adapter attempts to read other adapter's memory: blocked by cgroups memory isolation
- Adapter attempts to exhaust system resources: blocked by cgroups CPU/memory/IO limits
- Adapter sends malformed data through FFI bridge: caught by PyO3 type validation

**Untrusted application threats:**
- Application attempts to bypass API: blocked by API being the only interface (no shared memory, no direct function calls)
- Application sends malformed requests: caught by API validation middleware
- Application attempts to access other profile's data: blocked by authentication middleware

**Physical threats:**
- Device theft: mitigated by PQC encryption at rest, TPM-sealed keys
- Hardware tampering: detected by TPM attestation during secure model load
- Cold boot attack: mitigated by encrypted VRAM (future, not in v1.0)

### 10.2 Cascade Failure Prevention

The Tock trust model prevents cascade failures through five mechanisms:

**1. Process isolation for adapters.**
Each adapter runs in its own Linux process with cgroups resource limits. A segfault in vLLM's CUDA code kills only the vLLM adapter process. The AdapterManager (Rust, trusted) detects the exit, logs it, and restarts the adapter. The scheduler re-routes requests. No other adapter, no core component, and no application is affected.

**2. Typed HIL traits for hardware access.**
No code outside of mai-hil can access hardware. The HIL traits define the complete hardware interface. The Rust type system enforces this at compile time. There is no runtime "permission check" because there is no way to call hardware functions without implementing the HIL trait, and only mai-hil crates implement HIL traits.

**3. API boundary for applications.**
Applications communicate with the MAI exclusively through HTTP (REST) or gRPC. There is no shared memory, no direct function call path, no IPC pipe. A crashed application drops its TCP connection. The API server cleans up the connection state. The MAI continues serving other applications.

**4. Error wrapping at trust boundaries.**
Every error that crosses a trust boundary is wrapped in the receiving zone's error type. A CUDA driver error becomes an HilError, which becomes a CoreError, which becomes an ApiError. At each wrapping, backend-specific details are logged but not forwarded. An application cannot determine which GPU vendor the system uses by analyzing error messages.

**5. Audit trail for accountability.**
Every API call, every model load, every power state transition, every adapter restart is logged to the audit trail. The audit trail is append-only, hash-chained, and PQC-signed. If something goes wrong, the audit trail provides a complete, tamper-evident record of what happened.

### 10.3 Post-Quantum Cryptography

The MAI deploys NIST PQC standards four years ahead of the 2030 recommended transition deadline:

**ML-KEM (Kyber-1024):** Key Encapsulation Mechanism for encrypting data at rest in the vault. All model weights, family profile data, audit logs, and vector embeddings are encrypted with ML-KEM.

**ML-DSA (Dilithium-87):** Digital Signature Algorithm for signing model packages, audit entries, and configuration files. Every .mai-pkg model package carries an ML-DSA signature verified before installation.

**TPM 2.0 key management:** The master encryption key is sealed to the TPM. Unsealing requires the TPM's Platform Configuration Registers (PCRs) to match the expected boot state. A modified boot chain cannot unseal the vault.

### 10.4 Air-Gap as Architecture

Air-gap is not a mode. It is the architectural default. Every component must function with zero network access. There are no `if air_gap_mode:` conditionals in the codebase. If network access is available, it is an optional enhancement (e.g., network model updates as an alternative to USB), not a requirement.

The air-gap daemon (specified in Session 02, implemented in Session 06) monitors network interfaces. If an unexpected network interface becomes active while the system is supposed to be air-gapped, the daemon logs a security event and optionally disables the interface.

Telemetry is NEVER transmitted off-device. Health data, usage metrics, performance telemetry, and audit logs are local-only. Even when the device is connected to a network for model updates, telemetry remains local. No exceptions.

---

## 11. Hardware Capability Negotiation Protocol

### 11.1 Overview

When the MAI boots, it must discover what hardware is present, what that hardware can do, and how to schedule inference across it. This is the hardware capability negotiation protocol.

### 11.2 Discovery Phase

On boot (or on hardware hotplug event):

1. **Enumerate:** HIL HardwareProbe trait calls `enumerate_devices()` to discover all compute targets
2. **Probe:** For each discovered device, call `probe_capabilities()` to get a CapabilityDescriptor
3. **Validate:** Compare capabilities against minimum requirements for the configured product tier
4. **Register:** Store capabilities in the Scheduler's hardware capability map

### 11.3 CapabilityDescriptor

```rust
pub struct CapabilityDescriptor {
    /// Unique device identifier
    pub device_id: DeviceId,
    /// Device type (GPU, Memristor, CPU, etc.)
    pub device_type: DeviceType,
    /// Human-readable device name (e.g., "NVIDIA RTX 5090")
    pub device_name: String,
    /// Driver/SDK version
    pub driver_version: String,

    // Memory
    /// Total VRAM/compute memory in bytes
    pub total_memory_bytes: u64,
    /// Available (unallocated) memory in bytes
    pub available_memory_bytes: u64,
    /// Memory bandwidth in GB/s
    pub memory_bandwidth_gbps: f64,

    // Compute
    /// Supported compute precisions
    pub compute_types: Vec<ComputeType>,  // FP32, FP16, BF16, INT8, INT4, etc.
    /// Supported quantization formats
    pub quantization_formats: Vec<QuantizationFormat>,  // GGUF, GPTQ, AWQ, EXL2, etc.
    /// Compute throughput estimate (TFLOPS at best precision)
    pub peak_tflops: f64,

    // Power
    /// Thermal Design Power in watts
    pub tdp_watts: u32,
    /// Current power draw in watts
    pub current_power_watts: u32,
    /// Current temperature in Celsius
    pub current_temp_c: u32,
    /// Thermal throttle threshold in Celsius
    pub throttle_temp_c: u32,

    // Features
    /// Whether this device supports tensor parallelism
    pub tensor_parallel: bool,
    /// Whether this device supports multi-model multiplexing
    pub multi_model: bool,
    /// Maximum concurrent inference streams
    pub max_concurrent_streams: u32,
    /// Wake latency from sleep to active (milliseconds)
    pub wake_latency_ms: u32,
}
```

### 11.4 Capability Matching

When the Scheduler receives an inference request, it matches the request's requirements against available capabilities:

```
Request: "I need a model that requires 14GB VRAM, FP16 compute, GGUF format"
Scheduler checks:
  1. Which devices have >= 14GB available VRAM?
  2. Which of those support FP16?
  3. Which of those have an adapter that supports GGUF?
  4. Of the matches, which has the lowest current load?
  -> Route to that device + adapter combination
```

### 11.5 Multi-GPU Scheduling

For Ranger and Pack Leader tiers with multiple GPUs:

**Tensor Parallelism (vLLM):** A single model is split across GPUs. The scheduler treats the GPU group as a single logical device with combined VRAM.

**Model Parallelism:** Different models on different GPUs. The scheduler tracks per-GPU model assignments and routes requests to the GPU with the correct model loaded.

**Hybrid:** Some GPUs run tensor-parallel large models while others run independent smaller models. The scheduler maintains a capability map per GPU and routes accordingly.

### 11.6 QM-Era Capability Extension

The CapabilityDescriptor is designed for extensibility. When TetraMem MX100 hardware arrives in 2028, its CapabilityDescriptor will include:

- `device_type: DeviceType::Memristor`
- `compute_types: [ComputeType::INT4, ComputeType::INT8]` (native analog compute)
- `tdp_watts: 8` (vs 350W for GPU)
- Additional fields for memristor-specific capabilities (analog precision, crossbar dimensions)

The Scheduler's capability matching algorithm works identically. It does not know or care whether the device is a GPU or a memristor. It sees capabilities and routes accordingly.

---

## 12. Power State Machine

### 12.1 States

| State | Power Draw (GPU era) | Power Draw (QM era) | Description |
|---|---|---|---|
| Off | 0W | 0W | System powered down |
| DeepVaultSleep | ~2W | ~1W | CPU standby, GPU off, vault encrypted, WoL listening |
| Sentinel | ~8W | ~3W | Small model loaded (Phi-4-mini), handles simple queries, home automation |
| FullInference | ~350W | ~15W | Full model(s) loaded, maximum capability |
| ThermalThrottle | Variable | N/A | GPU thermal limit exceeded, reduced performance |

### 12.2 Transition Matrix

| From \ To | Off | DeepVault | Sentinel | FullInference | ThermalThrottle |
|---|---|---|---|---|---|
| **Off** | -- | Boot | -- | -- | -- |
| **DeepVault** | Shutdown | -- | Wake trigger | -- | -- |
| **Sentinel** | Shutdown | Idle 2hr | -- | Capability exceeded | -- |
| **FullInference** | Shutdown | -- | Idle 12min | -- | Thermal limit |
| **ThermalThrottle** | -- | -- | -- | Temp recovered | -- |

### 12.3 Wake Triggers

Events that transition from DeepVaultSleep to Sentinel:
- API request received (TCP connection on MAI port)
- Wake-on-LAN packet received
- Scheduled task timer fires
- HomeBase Matter/Thread event (device command)

### 12.4 Sentinel Model

The Sentinel model is a small language model that stays loaded during Sentinel mode. It handles:
- Simple conversational queries
- HomeBase device commands ("turn off the lights")
- Basic system queries ("what models are available?")
- Deciding whether a request requires Full Inference promotion

Product tier Sentinel models:
- Scout/Ranger: Phi-4-mini (~3.8B parameters, ~2.5GB VRAM)
- Pack Leader: Gemma 4 12B (~8GB VRAM)

### 12.5 Promotion Protocol

When a request exceeds Sentinel capability:
1. Sentinel model evaluates the request complexity
2. If beyond Sentinel capability, returns a "promotion required" signal
3. PowerStateMachine transitions Sentinel -> FullInference
4. Full model loads (target: <8 seconds to first token on Scout tier)
5. Request is re-routed to the full model
6. Auto-demotion timer starts (12 minutes, configurable)

### 12.6 Sovereignty Signal

The power state machine is not just an engineering optimization. The 48-month plan calls it "a sovereignty signal." A home AI that draws 2W in Deep Vault Sleep and wakes to Sentinel in under 2 seconds feels like an appliance. One that draws 350W continuously feels like a liability. Sleep mode is the difference between a product families leave plugged in and one they turn off.

---

## 13. Model Registry Schema

### 13.1 Model Manifest (TOML)

Every model in the MAI registry is described by a TOML manifest:

```toml
[model]
id = "qwen3-14b-q5_k_m"
name = "Qwen3 14B"
version = "3.0.1"
family = "qwen3"
parameter_count = 14_000_000_000
quantization = "Q5_K_M"

[requirements]
min_vram_bytes = 12_884_901_888  # 12GB
compute_types = ["FP16", "INT8"]
quantization_format = "GGUF"
compatible_adapters = ["ollama", "llamacpp"]

[capabilities]
context_window = 131072
languages = ["en", "zh", "ja", "ko", "fr", "de", "es"]
supports_function_calling = true
supports_vision = false
supports_streaming = true

[sentinel]
is_sentinel_candidate = false
estimated_load_time_ms = 6000

[storage]
vault_path = "models/qwen3/qwen3-14b-q5_k_m.gguf"
size_bytes = 10_737_418_240  # 10GB
sha256 = "a1b2c3d4..."

[metadata]
added_date = "2026-05-15"
source = "usb-install"
package_signature = "ml-dsa-sig:..."
```

### 13.2 Registry State Machine

```
Unknown          Model ID referenced but no manifest found
    |
    v
Downloaded       Manifest parsed, weights in vault, not loaded
    |
    v
Loading          Weights being transferred to VRAM
    |
    +-----> Error (rollback to Downloaded)
    |
    v
Loaded           Weights in VRAM, adapter health check pending
    |
    v
Active           Serving inference requests
    |
    v
Unloading        Weights being evicted from VRAM
    |
    v
Downloaded       Back to vault-only state
```

### 13.3 Version Management

Model packages use semantic versioning. The registry tracks:
- Currently active version per model family
- Previous version (for rollback)
- Available updates (from USB or network)
- Compatibility matrix: which model versions work with which adapter versions

---

## 14. Technology Choices and Rationale

| Choice | Selected | Alternatives Considered | Rationale |
|---|---|---|---|
| Core language | Rust | Go, C++ | Memory safety without GC, zero-cost abstractions, no unsafe in core. Go lacks the fine-grained control needed for HIL drivers. C++ lacks the safety guarantees. |
| Adapter language | Python | Rust, TypeScript | Every major inference backend (Ollama, vLLM, llama.cpp, TGI, TensorRT-LLM, ExLlamaV2, SGLang) has a Python client library. Writing adapters in Rust would require reimplementing these clients. |
| FFI bridge | PyO3 | gRPC, Unix sockets | PyO3 gives typed FFI with Rust's type system enforcing correctness. gRPC adds network overhead for an in-process call. Unix sockets require manual serialization. |
| HTTP framework | axum | actix-web, warp | axum is from the Tokio team, async-first, composable middleware, fastest growing Rust HTTP framework. |
| gRPC framework | tonic | grpc-rs | tonic is pure Rust, integrates with tokio, no C++ dependency. grpc-rs wraps C++ gRPC core. |
| Async runtime | tokio | async-std | De facto Rust async standard. axum and tonic both use it. |
| Serialization | serde + TOML | JSON, YAML | serde is the Rust serialization standard. TOML for configuration (human-readable, unambiguous). JSON for API responses. No YAML (ambiguous parsing, security issues with YAML bomb). |
| Error handling | thiserror | anyhow | thiserror for library-style typed errors. anyhow is for applications. The MAI is a library/framework. |
| PQC | liboqs-rust / pqcrypto | classical RSA/ECC | NIST PQC standards, 4 years ahead of 2030 deadline. Classical crypto will be breakable by quantum computers. |
| Vector DB | Qdrant | Weaviate, Milvus, Chroma | Qdrant has a Rust client, runs locally, supports air-gap deployment. Others require more infrastructure. |
| Profile store | SQLite | PostgreSQL | SQLite is embedded, no server process, works air-gapped. PostgreSQL is overkill for profile storage on a single machine. |
| Logging | tracing + structlog | log4rs, env_logger | tracing for Rust (structured, async-aware), structlog for Python (structured, compatible). |
| CI | GitHub Actions | GitLab CI, Jenkins | Repo hosted on GitHub, Actions is native. |

---

## 15. Product Tier Hardware Profiles

### Scout Tier

| Attribute | Value |
|---|---|
| GPU | 1x NVIDIA RTX 5090 |
| VRAM | 32GB GDDR7 |
| Primary Adapter | Ollama |
| Sentinel Model | Phi-4-mini (~3.8B) |
| Full Models | Qwen3 14B + Gemma 4 26B (quantized to fit 32GB) |
| TDP | 575W system |
| Use Case | Single-family home AI |

### Ranger Tier

| Attribute | Value |
|---|---|
| GPU | 2x GPU (RTX 5090 or MI300X) |
| VRAM | 48-80GB combined |
| Primary Adapter | vLLM (tensor parallel across GPUs) |
| Sentinel Model | Phi-4-mini (~3.8B) |
| Full Models | Qwen3 70B + DeepSeek V4 + buyer-selected |
| TDP | 800W system |
| Use Case | Power users, small professional offices |

### Pack Leader Tier

| Attribute | Value |
|---|---|
| GPU | 4+ GPU (H100/H200 or MI300X array) |
| VRAM | 160GB+ combined |
| Primary Adapter | Full adapter fleet |
| Sentinel Model | Gemma 4 12B |
| Full Models | Full model library, fine-tuning capable |
| TDP | 1200W+ system |
| Use Case | Enterprises, research, multi-family estates |

---

## 16. Air-Gap Architecture

### 16.1 Network Isolation

The IM-OS hardware includes a physical air-gap switch. When engaged:
- All network interfaces are hardware-disabled
- The air-gap daemon verifies no interfaces are active
- Model updates are via USB only
- Telemetry remains local (it always does, but the switch provides physical assurance)

### 16.2 Air-Gap Daemon

Specified in Session 02, implemented in Session 06. Responsibilities:
- Monitor network interface state
- Alert if unexpected interface appears while air-gap switch is engaged
- Provide API endpoint for air-gap verification (/v1/health includes air-gap status)
- Log all network interface state changes to audit trail

### 16.3 Model Updates in Air-Gap Mode

Models are delivered as .mai-pkg files on USB drives:
1. User inserts USB drive
2. MAI detects .mai-pkg files
3. ML-DSA signature verification
4. Compatibility check (model requirements vs. hardware capabilities)
5. Installation to encrypted vault
6. Registry update

No network required. No phone-home. No license server check.

---

## 17. Telemetry and Observability

### 17.1 Local-Only Telemetry

All telemetry is collected and stored locally. It is NEVER transmitted off-device.

**Metrics collected:**
- Tokens per second (per adapter, per model)
- Time to first token (per request)
- Request latency (end-to-end)
- GPU utilization, VRAM usage, temperature, power draw
- Adapter health status and restart count
- Power state transition frequency and timing
- Model load/unload timing
- Queue depth and wait time

**Storage:** Telemetry is written to local SQLite databases with automatic rotation. Retention is configurable (default: 90 days for detailed, 1 year for aggregated).

### 17.2 Structured Logging

Rust components use `tracing` with structured fields. Python components use `structlog`. Both produce JSON-formatted log entries with:
- Timestamp (UTC)
- Component name
- Log level
- Structured fields (request_id, adapter_name, model_id, etc.)

### 17.3 Health Endpoint

`GET /v1/health` returns:
- Overall system health (healthy/degraded/critical)
- Per-adapter health status
- Hardware telemetry snapshot
- Power state
- Air-gap status
- Loaded models
- Queue depth

---

## 18. Configuration Architecture

### 18.1 Configuration Hierarchy

All configuration is TOML. No YAML. No JSON for config. No environment variables for production settings.

```
1. Product tier defaults (configs/scout.toml, configs/ranger.toml, configs/pack-leader.toml)
2. System overrides (/etc/mai/config.toml)
3. Runtime API overrides (POST /v1/admin/config, persisted to /etc/mai/overrides.toml)
```

Lower numbers are defaults, higher numbers override. Environment variables are accepted for CI/test overrides only, prefixed with `MAI_` (e.g., `MAI_LOG_LEVEL=debug`).

### 18.2 Configuration Sections

```toml
[mai]
product_tier = "scout"  # scout | ranger | pack-leader
log_level = "info"

[mai.power]
sentinel_model = "phi-4-mini"
auto_demote_full_to_sentinel_minutes = 12
auto_demote_sentinel_to_sleep_minutes = 120
enable_schedule_profiles = false

[mai.scheduler]
strategy = "least-loaded"  # round-robin | least-loaded | model-affinity | priority
max_queue_depth = 100
request_timeout_seconds = 120

[mai.adapters]
enabled = ["ollama"]
process_memory_limit_mb = 8192
process_cpu_limit_percent = 80
restart_max_attempts = 5
restart_backoff_base_seconds = 1

[mai.api]
listen_address = "127.0.0.1"
rest_port = 8080
grpc_port = 50051
enable_streaming = true
enable_websocket = true

[mai.vault]
zfs_dataset = "vault/models"
profile_db_path = "/var/lib/mai/profiles.db"
audit_log_path = "/var/lib/mai/audit/"

[mai.health]
heartbeat_interval_seconds = 10
telemetry_retention_days = 90
```

---

## 19. Deployment Model

### 19.1 System Service Architecture

The MAI runs as a set of systemd services with dependency ordering:

```
mai-hil.service          (HIL drivers, hardware detection)
    |
    v
mai-core.service         (core kernel, scheduler, registry, power)
    |
    v
mai-adapters.service     (adapter manager, spawns adapter processes)
    |
    v
mai-api.service          (REST + gRPC API server)
    |
    v
mai-health.service       (health monitor, telemetry collection)
    |
    v
mai-airgap.service       (air-gap daemon, network monitoring)
    |
    v
mai-vault.service        (vault interface, PQC operations)
```

### 19.2 First Boot

Target: first-boot completes in under 3 minutes on Scout hardware.

First-boot sequence:
1. TPM key initialization (if first boot)
2. ZFS vault creation and encryption setup
3. Hardware detection and capability probing
4. Default Sentinel model installation from bundled package
5. Family profile creation wizard hook (L6 UI provides the wizard)
6. Health check and status report

### 19.3 Packaging

- **Debian package (.deb):** Primary distribution method. Includes all Rust binaries, Python packages, systemd service files, default configurations.
- **Python wheels:** Separate distribution for mai-sdk-python and adapter updates.
- **Docker Compose:** Development and testing alternative. Not the production deployment method.

---

## 20. Session Dependency Graph

```
SESSION 01 (Master Architecture) ---- ROOT
    |
    +-- SESSION 02 (HIL Spec)
    |   +-- SESSION 06 (HIL Implementation)
    |   |   +-- SESSION 07 (Core Kernel Implementation)
    |   |   |   +-- SESSION 11 (API Server)
    |   |   |   +-- SESSION 13 (Agent/RAG Interface)
    |   |   |   +-- SESSION 14 (Sleep Mode)
    |   |   +-- SESSION 08 (Adapter Framework + Ollama)
    |   |       +-- SESSION 09 (Remaining Adapters)
    |   |           +-- SESSION 10 (E2E Integration Testing)
    |   +-- SESSION 03 (Adapter Framework Spec)
    |       +-- SESSION 08
    |       +-- SESSION 09
    +-- SESSION 04 (Core Kernel Spec)
    |   +-- SESSION 07
    |   +-- SESSION 14
    +-- SESSION 05 (API Surface Spec)
        +-- SESSION 11
        +-- SESSION 12 (Vault Integration)
        |   +-- SESSION 14
        |   +-- SESSION 15 (Model Management)
        |   +-- SESSION 16 (L4-L5 Scaffolds)
        +-- SESSION 13
        +-- SESSION 16

SESSION 10 + 11 + 12 + 13 + 14 + 15 + 16 --> SESSION 17 (System Validation)
SESSION 17 --> SESSION 18 (Deployment Packaging)
```

Critical path: **01 -> 02 -> 06 -> 07 -> 11 -> 12 -> 15 -> 17 -> 18** (9 sessions sequential minimum)

---

## 21. Glossary

| Term | Definition |
|---|---|
| **Adapter** | A sandboxed Python process that wraps an inference backend (e.g., Ollama, vLLM). Adapters are untrusted capsules in the Tock trust model. |
| **AdapterBase** | The Python abstract base class all adapters inherit from. Defines the standard interface for chat, completion, embedding, model management, and health checking. |
| **AdapterManager** | The Rust component that spawns, monitors, and communicates with adapter processes. Runs in the trusted zone. |
| **Air-gap** | Physical network isolation. The IM-OS hardware includes a physical switch that disables all network interfaces. Air-gap is the default operating mode. |
| **Air-gap daemon** | A service that monitors network interface state and alerts if unexpected interfaces appear while air-gap is expected. |
| **Auto-demotion** | Automatic power state reduction after a period of inactivity. Full Inference demotes to Sentinel after 12 minutes. Sentinel demotes to Deep Vault Sleep after 2 hours. Both are configurable. |
| **Capsule** | Tock term for a kernel extension that implements a device driver. In the MAI, backend adapters are the capsules. |
| **CapabilityDescriptor** | A Rust struct that describes a hardware device's compute capabilities: VRAM, precision support, power draw, thermal state, etc. |
| **Deep Vault Sleep** | The lowest power state (~2W). CPU on standby, GPU powered off, vault encrypted, WoL listening. |
| **Family profile** | A user identity within IM-OS. Each family member has a profile with access permissions, priority level, and usage history. Authentication is local-only (X-IM-Profile header). |
| **Full Inference** | The highest power state (~350W). Full-size models loaded, maximum hardware capability available. |
| **HIL (Hardware Interface Layer)** | The typed Rust trait boundary between the MAI core kernel and hardware-specific drivers. Adapted from Tock's HIL. |
| **Hot-swap** | Zero-downtime replacement of a model or adapter. The new version is loaded and verified before routing switches over. Automatic rollback on failure. |
| **IM-OS** | Island Mountain Operating System. The sovereign data and identity operating system that the MAI is part of. |
| **MAI** | Model Abstraction Interface. The core inference abstraction layer (L3) of IM-OS. |
| **ML-DSA** | Module-Lattice Digital Signature Algorithm (formerly Dilithium). NIST PQC standard for digital signatures. Used for model package signing and audit trail integrity. |
| **ML-KEM** | Module-Lattice Key Encapsulation Mechanism (formerly Kyber). NIST PQC standard for encryption. Used for vault data at rest. |
| **Model manifest** | A TOML file describing a model's metadata, requirements, capabilities, and storage location. |
| **Pack Leader** | The highest product tier. 4+ GPUs, 160GB+ VRAM, full adapter fleet, enterprise use case. |
| **PQC** | Post-Quantum Cryptography. Cryptographic algorithms designed to resist quantum computer attacks. The MAI uses ML-KEM and ML-DSA, deploying 4 years ahead of NIST's 2030 recommendation. |
| **Promotion** | Automatic power state escalation when a request exceeds current capability. Typically Sentinel to Full Inference. Target: <8 seconds to first token. |
| **PyO3** | The Rust-Python FFI bridge library. Used to communicate between the trusted Rust AdapterManager and untrusted Python adapters. |
| **Ranger** | The mid-tier product. 2x GPUs, 48-80GB VRAM, vLLM tensor parallel, power user and small office use case. |
| **Scout** | The entry-level product tier. 1x RTX 5090, 32GB VRAM, Ollama adapter, single-family home use case. |
| **Sentinel** | A low-power operating mode (~8W) where a small model (Phi-4-mini or Gemma 4 12B) handles simple queries. If a request exceeds Sentinel capability, the system promotes to Full Inference. |
| **Sentinel model** | The small language model kept loaded during Sentinel mode. Phi-4-mini for Scout/Ranger, Gemma 4 12B for Pack Leader. |
| **Sovereignty signal** | The 48-month plan's term for the power state machine. A home AI that draws 2W in sleep feels like an appliance. One that draws 350W continuously feels like a liability. |
| **Syscall interface** | Tock term for the boundary between processes and the kernel. In the MAI, this is the REST/gRPC API that applications call. |
| **Thermal throttle** | A power state entered when GPU temperature exceeds its thermal limit. Performance is reduced until temperature recovers. |
| **Tock** | A secure embedded operating system for microcontrollers. The MAI adapts Tock's layered trust model for AI inference abstraction. |
| **TPM 2.0** | Trusted Platform Module. A hardware security chip used for key management and boot attestation. |
| **Trust boundary** | The interface between code at different trust levels. Errors wrap at trust boundaries. Data validates at trust boundaries. Backend details never leak across trust boundaries. |
| **Vault** | The encrypted storage layer (L2) for all persistent data in IM-OS. Uses ZFS with ML-KEM encryption and TPM-sealed keys. |
| **Wake trigger** | An event that transitions the system from Deep Vault Sleep to Sentinel: API request, WoL packet, scheduled task, or HomeBase event. |

---

*MAI Master Architecture Specification v1.0.0-spec*
*Session 01 Deliverable*
*Island Mountain AI | Confidential*
*2026-05-15*
