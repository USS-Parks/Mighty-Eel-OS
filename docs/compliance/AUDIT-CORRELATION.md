# Audit Correlation (BF-5)

Status: landed with Session 42 (2026-05-22 late evening).

This document defines the metadata-only bridge between an OpenBao
credential event and the Lamprey policy decision it authorised. The
bridge is what makes it possible to answer "which claim issued this
decision?" from either side of the air gap without ever sending the
prompt, completion, or any regulated payload to the cloud trust
system.

The schema and acceptance criteria below match Appendix A §A.9
verbatim — when the cloud audit store ingests these events, no field
translation is required.

## Why this exists

Lamprey policy decisions are recorded on the local appliance in a
tamper-evident chain ([`audit::HashChainManager`]). OpenBao records
credential events in its own cloud-side audit log. Without an
explicit correlation, the two logs cannot be joined: forensics on a
revoked claim cannot find the inferences it authorised, and the
acquirer's due-diligence questionnaire ("show me every decision tied
to credential X") has no answer.

The BF-5 correlation event carries just enough metadata to join the
two logs:

- enough identity to link the two events (`credential_event_id` ↔
  `lamprey_decision_id`)
- enough provenance to make the link explainable
  (`tenant`, `service_identity`, `policy_version`,
  `trust_bundle_version`)
- nothing that could leak the request (no prompt, no completion, no
  raw `subject_id`)

## Schema

[`crate::audit::CorrelationFields`] serialises to:

```json
{
  "credential_event_id": "cred_evt_123",
  "lamprey_decision_id": "dec_456",
  "mai_request_id": "req_789",
  "tenant": "tribal-health-demo",
  "subject_hash": "hmac:6c7d3a...",
  "service_identity": "lamprey-router",
  "policy_version": "2026.05.22.001",
  "trust_bundle_version": "2026.05.22.001",
  "decision": "local_only_allowed"
}
```

### Field provenance

| Field | Source | Notes |
|-------|--------|-------|
| `credential_event_id` | OpenBao credential event | `None` for local-only / offline flows that never exchanged a claim. |
| `lamprey_decision_id` | Assigned by the audit chain | Formatted as `"dec_<id>"` where `<id>` is the chain's monotonic per-shard id. |
| `mai_request_id` | [`policy::bundle::RequestMetadata`] | Stable across retries; same value the inference response carries. |
| `tenant` | [`trust::TrustContext::tenant_id`] | Tenant the subject's claim was issued under. |
| `subject_hash` | [`subject_hash::hmac_subject`] | HMAC-SHA256 pseudonym of the subject id; always begins with `"hmac:"`. |
| `service_identity` | [`trust::TrustContext::service_identity`] | `None` for human subjects; `Some(...)` for service-to-service claims. |
| `policy_version` | Active policy bundle version | Recorded so a decision can be replayed against the exact policy that produced it. |
| `trust_bundle_version` | [`trust::TrustContext::trust_bundle_version`] | Version of the trust bundle the claim was issued against. |
| `decision` | [`audit::RoutingDecision`] | Wire vocabulary: `allow`, `local_only_allowed`, `quarantine`, `deny`. |

## Subject hashing

Raw subject ids never appear in correlation events. The HMAC
construction is documented in [`crate::subject_hash`] and uses
SHA-256 with a per-tenant key held in the local vault. Two
properties matter for the bridge:

- **Deterministic per tenant.** The same subject id hashes to the
  same value across calls, so cross-claim correlation within a
  tenant is possible.
- **Not deterministic across tenants.** A different tenant key
  produces a different hash for the same subject id. Cross-tenant
  joins are intentionally impossible.

The `"hmac:"` prefix is mandatory and is what audit consumers use to
distinguish a pseudonymised identifier from a raw one. Code that
constructs a correlation event must use [`hmac_subject`] — manually
building the string is a forbidden pattern.

## Metadata-only sync

Correlation events are the *only* compliance-side payload that may
leave the local appliance by default. They contain no prompt, no
completion, no embedding, no PHI, no ITAR/EAR content, and no OCAP
payload. The §A.2 hard rule is enforced by construction: the
correlation type holds metadata fields only, and the audit log's
`enqueue_correlation` API takes a [`CorrelationFields`] — there is
no way to attach a payload.

## Offline queue and replay

Connectivity to the cloud audit destination is not assumed. The
audit store maintains an in-memory FIFO queue of correlation events:

- Producers call [`AuditStore::enqueue_correlation`] for every
  recorded decision, regardless of connectivity state.
- A separate cloud-sync worker is responsible for calling
  [`AuditStore::drain_offline_queue`] and pushing the drained
  events to the cloud audit destination.
- When the queue is full (default capacity 4096), the oldest events
  are dropped. The drop count is surfaced on
  [`StoreDropCounters::offline_events_dropped`] for dashboards.

When connectivity is restored, the worker drains the queue in
arrival order. Because every event is self-contained metadata, no
ordering guarantees are required for correctness — the cloud store
sorts on `lamprey_decision_id` for replay.

## Acceptance criteria (§A.9)

| Criterion | Where it lives |
|-----------|---------------|
| Credential event can be linked to Lamprey route decision. | [`CorrelationFields::credential_event_id`] ↔ [`CorrelationFields::lamprey_decision_id`]. |
| Subject identifier is hashed or pseudonymised. | [`subject_hash::hmac_subject`] enforces the construction; the field is named `subject_hash` and must start with `"hmac:"`. |
| No prompt or completion is included in the correlation event. | The type holds no payload fields; the only request-derived field on the parent [`AuditEntry`] is `request_hash`, a BLAKE3 over the *masked* request. |
| Offline audit queue can store correlation events. | [`AuditStore::enqueue_correlation`] / `offline_queue` (capacity 4096 by default). |
| Queued events can sync after reconnection. | [`AuditStore::drain_offline_queue`] returns the queued events in arrival order. |
| Audit correlation can be demonstrated during Session 46. | Verified end-to-end in `mai-compliance` test `policy::audit::api::tests::record_enqueues_bf5_correlation_event` and downstream demo scenarios. |

## Related modules

- [`crate::audit::AuditLog`] — the façade that builds correlation
  events from a [`PolicyBundle`] + [`AggregateDecision`].
- [`crate::audit::HashChainManager`] — the tamper-evident chain
  whose ids back the `lamprey_decision_id` field.
- [`crate::policy::AuditFeed`] — the in-process broadcast channel
  the audit log can subscribe to so policy decisions land in the
  chain without explicit `record` calls at every call site.
- [`crate::trust::TrustContext`] — the source of every BF-2 / BF-5
  identity field used in the correlation event.
