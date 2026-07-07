# Security Regression Registry

A deterministic regression identifier per audit finding. Adversarial fixtures start
as a quarantined, feature-gated harness asserting the CURRENT vulnerable behavior
(no `#[ignore]`, per plan §0.5). When a fix lands, the matching fixture flips to
assert the repaired behavior and joins the default product suite.

AF-001 / AF-006 fixtures (fix landed): `crates/fabric-token/tests/security_regression.rs`
has been flipped to assert the repaired behavior and de-quarantined — the
`security-regression` feature is retired; run `cargo test -p fabric-token`.

| Reg ID | Finding | Fixture asserts | Status | Location |
|---|---|---|---|---|
| REG-AF-001-unsigned-parent | AF-001 | unsigned parent is rejected (signer oracle closed) | FIXED | fabric-token suite |
| REG-AF-001-wrong-key-parent | AF-001 | attacker-key parent is rejected | FIXED | fabric-token suite |
| REG-AF-001-role-widening | AF-001 | child gaining a role the parent lacks is rejected | FIXED | fabric-token suite |
| REG-AF-001-tenant-swap | AF-001 | child claiming a different tenant is rejected | FIXED | fabric-token suite |
| REG-AF-001-service-identity-swap | AF-001 | child assuming a different service identity is rejected | FIXED | fabric-token suite |
| REG-AF-001-scope-widening | AF-001 | child gaining a compliance scope the parent lacks is rejected | FIXED | fabric-token suite |
| REG-AF-001-subject-swap | AF-001 | child claiming a different subject is rejected | FIXED | fabric-token suite |
| REG-AF-006-revoked-parent | AF-006 | a revoked parent mints no children | FIXED | fabric-token suite |
| REG-AF-002-caller-subject | AF-002 | `/v1/tokens/issue` mints from caller-supplied tenant/subject/roles | PHASE A | wsf-bridge / wsf-api |
| REG-AF-003-cross-tenant-unseal | AF-003 | unseal opens another tenant's envelope | PHASE E | wsf-seal |
| REG-AF-004-arbitrary-role | AF-004 | exchange assumes a caller-chosen `role_arn` | PHASE B | wsf-broker |
| REG-AF-007-unfiltered-receipts | AF-007 | receipt query returns cross-tenant entries | PHASE L | wsf-ledger / wsf-api |

Every finding has a deterministic identifier (gate §0.4). AF-001 and AF-006 are
FIXED: the fixtures now assert the repaired behavior in the default product suite
(parent signature + revocation verified, every identity/authority axis monotonic).
The broader AF-006 finding (all privileged consumers honouring signed revocation
snapshots) remains CONTAINED pending the R phase — this closes only the attenuation
leg. AF-002/003/004/007 identifiers are reserved and land with their phase, where
the same fixture flips to assert the repaired behavior. The `caveat widening` and
`stale token` cases named in §0.4 are covered by the AF-001 role/tenant/scope
widening and AF-006 revoked-parent fixtures respectively.
