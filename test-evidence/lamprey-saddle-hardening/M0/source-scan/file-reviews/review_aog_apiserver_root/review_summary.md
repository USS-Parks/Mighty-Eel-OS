# AOG API Server Review

Five assigned files were read in full. Targeted `aog-apiserver` auth, CRUD, policy, and receipt tests passed (22 tests), confirming the tested behavior while leaving the identified authorization, global-object isolation, read isolation, and durability gaps reachable in the code.

Seven candidate instances were preserved. The strongest is the absence of verb/kind/resource authorization after signature verification: the standard test token has empty roles and route grants, yet non-compliance kinds are intentionally admitted. A caller can create a `RevocationIntent` naming another tenant/token/subject/ring, which the controllers consume as an estate-wide kill instruction.

The GET and LIST routes are authenticated but do not receive a principal and do not filter by `metadata.tenant`. Update and delete reject only when both principal and object tenants are present and unequal, allowing a tenant principal to mutate system-owned global objects; the teardown controller deliberately creates global deprovision revocation records.

Admission commits desired state before appending to a process-local receipt vector, ignores the ingest result, and regenerates its signing key on construction. Restart/crash therefore breaks the claimed durable 1:1 mutation-to-proof invariant.

The sealing placeholder candidate is deferred pending a deployed unseal consumer: it is a real validation bypass for desired-state integrity, but current checked-out code does not turn the injected annotation into plaintext or key use.
