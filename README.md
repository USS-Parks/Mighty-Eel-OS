# MAI: Model Abstraction Interface

MAI is the local inference and governance layer for IM-OS, Island
Mountain's sovereign data and identity operating system. It lets a
regulated organization run AI close to the data, prove who was allowed
to use it, route each request through policy, and verify the audit
trail afterward.

The inference engine is a plugin. The data sovereignty layer is the
product.

---

## What This Proves

Each point below maps to landed code and passing tests. None is a
roadmap item or a design intention.

- **Local-first inference:** REST, gRPC, streaming, SDKs, and seven
  backend adapters route through one stable MAI boundary. Verifiable:
  `cargo test --workspace` (1196+ tests).
- **Hardware-aware scheduling:** placement considers GPU topology, KV
  cache residency, batching opportunity, memory pressure, and power
  state on every request: not round-robin, not memory watermark. Every
  placement produces a `ScoreBreakdown` in the audit log. Verifiable:
  `cargo test -p mai-scheduler --lib` (324+ tests).
- **Trust without payload leakage:** OpenBao-backed claims and signed
  policy bundles cross the trust boundary; prompts, completions,
  embeddings, PHI, ITAR/EAR-controlled content, and OCAP-governed data
  do not. Verifiable: read `mai-compliance::bundle::canonical_bytes`;
  the signing payload contains identity and policy metadata only, no
  content.
- **Compliance before inference:** Lamprey policy modules compose HIPAA,
  ITAR/EAR, and OCAP decisions before the request is placed. The
  composer is fail-closed: deny-wins, most-restrictive-route, no silent
  allows. Verifiable: `mai-compliance/src/policy/composer.rs`.
- **Audit as proof:** policy decisions, credential events, and route
  outcomes link into a tamper-evident BLAKE3 hash chain with ML-DSA-87
  periodic signatures. The signed compliance report is verifiable
  off-host with the public key only. Verifiable:
  `POST /v1/compliance/audit/verify`.

**The trust boundary in one sentence:** identity, claims, signatures,
and audit correlation IDs cross between cloud and local; regulated
payloads never do. This separation is enforced architecturally in the
signing payload contract, not by configuration.

---

## Quick Start

Three commands from a fresh clone to a working system:

```powershell
# 1. Start the API server. The API key prints to stdout on first boot.
cargo run --bin mai-api

# 2. Run the Trust Manifold demo dry-run. No inference call needed.
$env:MAI_API_KEY = "im-..."
python apps/openbao-trust-demo/main.py --dry-run

# 3. Confirm the trust state of the local node.
curl -H "X-IM-Auth-Token: $env:MAI_API_KEY" `
  http://localhost:8420/v1/trust/status
```

A successful dry-run prints an audit-ready JSON summary with
`bundle_signature_verified: true`. The trust status call returns
`{"mode": "connected", ...}`. If either fails, see
`docs/KNOWN-ISSUES.md` and `mai-sdk-python/docs/quickstart.md`.

For the full interactive demo including compliance routing, degraded
bundle behavior, and signed report generation, see `docs/DEMO-SUITE.md`.

---

## Start Here

| If you are... | Start with |
|---|---|
| Reviewing the product thesis | [docs/ACQUISITION-PACKAGE.md](docs/ACQUISITION-PACKAGE.md) |
| Running the Trust Manifold demo | `python apps/openbao-trust-demo/main.py --dry-run` |
| Running the full demo suite | [docs/DEMO-SUITE.md](docs/DEMO-SUITE.md) |
| Integrating trust and identity | [docs/BUYER-INTEGRATION-GUIDE.md](docs/BUYER-INTEGRATION-GUIDE.md) |
| Understanding the architecture | [docs/MAI-MASTER-ARCHITECTURE.md](docs/MAI-MASTER-ARCHITECTURE.md) |
| Building against the SDK | [mai-sdk-python/docs/quickstart.md](mai-sdk-python/docs/quickstart.md) |
| Operating a local node | [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md) |

---

## Architecture

MAI adapts the Tock microcontroller kernel's layered trust model for AI
inference: a design where untrusted components cannot corrupt the
trusted core regardless of what they do. Applied here: adapters are
untrusted, the core kernel and compliance engine are trusted, and the
API boundary is the enforced separation between them.

- **Trusted core kernel (Rust):** scheduler, registry, power state
  machine, health monitor.
- **Untrusted adapters (Python via PyO3):** Ollama, vLLM, llama.cpp,
  TGI, TensorRT-LLM, ExLlamaV2, SGLang.
- **Stable API boundary:** REST, gRPC, Server-Sent Events, and WebSocket
  streaming.
- **Hardware Interface Layer:** typed traits that abstract GPU,
  memristor, and future compute targets.
- **Lamprey governance layer:** router, policy runtime, audit log,
  compliance reports, and dashboard.

See `docs/MAI-MASTER-ARCHITECTURE.md` for the full specification.

---

## Project Structure

```text
mai/
  mai-core/       Trusted core kernel (Rust)
  mai-hil/        Hardware Interface Layer (Rust)
  mai-adapters/   Adapter framework + PyO3 bridge (Rust)
  mai-api/        REST + gRPC API server (Rust)
  mai-sdk-rs/     Rust SDK
  mai-sdk-python/ Python SDK
  adapters/       Backend adapter implementations (Python)
  apps/           Demo and integration applications
  configs/        Product tier configurations
  tests/          Integration tests and benchmarks
  docs/           Architecture and specification documents
```

---

## Build And Test

```powershell
# Rust components
cargo check --workspace
cargo clippy --workspace
cargo test --workspace

# Python components
cd mai-sdk-python
pip install -e ".[dev]"
ruff check adapters/
mypy --strict adapters/
pytest adapters/
```

For hardware-dependent deferrals and open questions, see
`docs/KNOWN-ISSUES.md`.

---

## License

Proprietary. Island Mountain AI. All rights reserved.
