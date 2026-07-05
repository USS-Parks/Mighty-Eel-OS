# Loom Control-Plane — Disaster-Recovery Runbook (H4)

Cold-restore the Loom control-plane **estate** (desired state) from an encrypted
backup after a total loss of the control-plane hosts. The estate is `aog-store`;
the tamper-evident **receipt** ledger (`wsf-ledger`) is a physically separate
store and is recovered from its own append-only segments (H3) — this runbook
covers the estate.

The machinery this runbook drives is `aog_apiserver::backup`
(`backup_estate` / `restore_estate`); the DR drill in
`crates/aog-apiserver/tests/dr_drill.rs` executes the restore steps below and is
the gate for "a full DR drill from cold backup succeeds by the runbook alone."

## What a backup is

A backup is the estate's committed `key → value` content, serialized and
**envelope-sealed** with AES-256-GCM under a 32-byte **DR data key**
(`fabric-envelope`). At rest it is ciphertext, safe to place on removable media
or off-site object storage. The seal binds a fixed AAD, so a blob sealed for any
other purpose will not unseal as an estate backup; a wrong data key fails closed.

The **DR data key** is escrowed, never stored beside the backup:

- **Production:** the data key is wrapped by an OpenBao **Transit** key
  (`transit/loom-dr-backup`); the wrapped reference travels in the seal's
  `data_key_wrapped` field. Recovering it requires an authenticated Transit
  `decrypt` — i.e. an operator with the DR role, not the backup media alone.
- **Air-gapped estates:** the data key is held in operator escrow (split-knowledge
  / HSM), reconstructed at restore time.

## Take a backup (periodic)

1. Read the committed estate from the current leader: `RaftNode::range("")`.
2. Seal it: `backup_estate(&entries, &data_key)` → an encrypted blob.
3. Write the blob to the backup medium (removable media / off-site). Record the
   estate revision and entry count in the backup index for the drill's manifest
   check.

## Restore from a cold backup (DR)

Preconditions: a clean host, the latest sealed backup blob recovered from the
medium, and the DR data key recovered from escrow (Transit unwrap or operator
reconstruction). Then:

1. **Recover the data key** from escrow. Do not proceed without it — the backup is
   inert ciphertext otherwise.
2. **Read** the sealed backup blob from the medium.
3. **Unseal** it: `restore_estate(&blob, &data_key)` → the estate entries. A wrong
   key or tampered blob errors here — stop and escalate.
4. **Bootstrap** a fresh single-node control plane on the clean host:
   `RaftNode::bootstrap(node_id, dir)`.
5. **Re-apply** every entry as a `Put` (`Precondition::Any`). Revisions are
   re-established by the fresh estate; the authoritative `key → value` content is
   what the backup carries.
6. **Verify** the restored content against the backup manifest (entry count +
   spot-checked values). This is the drill's pass condition.
7. **Re-form HA:** once the single node is serving, admit the replacement peers as
   learners and promote them (`add_learner` → `change_membership`, H1), then resume
   normal operation. Rotate the DR data key if the old one may be compromised.

## Post-restore

- Confirm the receipt ledger recovered its chain (H3) and verifies off-host.
- Confirm workloads reconcile only on the new leader (the `SharedGate` follows
  quorum-confirmed leadership, H1/H2).
- File the drill result (date, estate revision restored, time-to-serve) in the
  operations log.
