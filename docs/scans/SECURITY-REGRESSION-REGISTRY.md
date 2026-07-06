# Security Regression Registry

A deterministic regression identifier per audit finding. Adversarial fixtures that
assert the CURRENT vulnerable behavior live in a quarantined, feature-gated harness
(no `#[ignore]`, per plan §0.5). When a fix lands, the matching fixture flips to
assert the repaired behavior and joins the product suite.

Quarantined harness (implemented now):
`crates/fabric-token/tests/security_regression.rs` —
run `cargo test -p fabric-token --features security-regression`.

| Reg ID | Finding | Fixture asserts (current, vulnerable) | Status | Location |
|---|---|---|---|---|
| REG-AF-001-unsigned-parent | AF-001 | attenuate signs a child of an unsigned parent (signer oracle) | IMPLEMENTED | fabric-token harness |
| REG-AF-001-wrong-key-parent | AF-001 | attenuate accepts an attacker-key parent | IMPLEMENTED | fabric-token harness |
| REG-AF-001-role-widening | AF-001 | child gains a role the parent never held | IMPLEMENTED | fabric-token harness |
| REG-AF-001-tenant-swap | AF-001 | child claims a different tenant | IMPLEMENTED | fabric-token harness |
| REG-AF-006-revoked-parent | AF-006 | a revoked parent still mints fresh children | IMPLEMENTED | fabric-token harness |
| REG-AF-002-caller-subject | AF-002 | `/v1/tokens/issue` mints from caller-supplied tenant/subject/roles | PHASE A | wsf-bridge / wsf-api |
| REG-AF-003-cross-tenant-unseal | AF-003 | unseal opens another tenant's envelope | PHASE E | wsf-seal |
| REG-AF-004-arbitrary-role | AF-004 | exchange assumes a caller-chosen `role_arn` | PHASE B | wsf-broker |
| REG-AF-007-unfiltered-receipts | AF-007 | receipt query returns cross-tenant entries | PHASE L | wsf-ledger / wsf-api |

Every finding has a deterministic identifier (gate §0.4). AF-001 and AF-006 are
implemented and proven against the current code; AF-002/003/004/007 identifiers
are reserved and land with their phase, where the same fixture flips to assert the
repaired behavior. The `caveat widening` and `stale token` cases named in §0.4 are
covered by the AF-001 role/tenant-widening and AF-006 revoked-parent fixtures
respectively; `wrong-key parent` and `unsigned parent` are both implemented.
