# review_wsf_front file review

Scan: `361f70b_20260715T130144Z`

Target: `C:\Users\17076\Documents\Claude\Mighty Eel OS\mai`

## Coverage

Five assigned files were read in full and closed with `reviewed` receipts in `work_ledger.jsonl`:

- `crates/wsf-api/src/auth.rs` (449 lines)
- `crates/wsf-api/src/lib.rs` (937 lines)
- `crates/wsf-api/src/policy.rs` (247 lines)
- `crates/wsf-bridge/src/lib.rs` (519 lines)
- `crates/wsf-broker/src/lib.rs` (765 lines)

Minimum supporting source was read only for concrete proof: `crates/wsf-api/src/main.rs`, `crates/fabric-token/src/lib.rs`, `crates/fabric-revocation/src/lib.rs`, and `crates/wsf-seal/src/lib.rs`.

## Candidate dispositions

- `review_wsf_front-001` - reportable: omitted `requested_models` yields an unrestricted signed model list despite a restricted tenant policy.
- `review_wsf_front-002` - reportable: the attenuation route never enforces the tenant's finite `max_delegation_depth`.
- `review_wsf_front-003` - reportable: a token revoked by token id can be attenuated into a newly signed child id because no current revocation snapshot is supplied.
- `review_wsf_front-004` - deferred: `/v1/tokens/verify` returns `valid=true` for stale-revoked tokens, but no concrete in-repository authorization consumer of `VerifyResp.valid` was found.
- `review_wsf_front-005` - reportable: shipped seal construction omits revocation state, so later-revoked tokens retain seal authority.
- `review_wsf_front-006` - reportable: shipped unseal construction omits revocation state, so later-revoked tokens retain decrypt authority.
- `review_wsf_front-007` - reportable: shipped AWS broker construction omits revocation state, so later-revoked tokens can mint fresh STS credentials.
- `review_wsf_front-008` - reportable: the AWS duration clamp widens 1-899 seconds of remaining token life to 900-second credentials.
- `review_wsf_front-009` - deferred: `TenantIssuancePolicy.max_classification` is unused, but no concrete deployed lower-policy/higher-OpenBao classification mismatch was found; shipped `single_dev` is already `Restricted`.

Every candidate has a unique `05_findings/<candidate_id>/candidate_ledger.jsonl` containing separate discovery, candidate-local validation, and candidate-local attack-path receipts.

## Exact deferred proof gaps

1. `review_wsf_front-004`: establish a deployed consumer that treats the REST `VerifyResp.valid` result as authorization; the stale-valid response behavior itself is proven.
2. `review_wsf_front-009`: establish a production `TenantPolicyStore` value whose `max_classification` is below the matching OpenBao `TenantAttributes.max_data_classification`; the missing enforcement itself is proven.

No repository target file was modified.
