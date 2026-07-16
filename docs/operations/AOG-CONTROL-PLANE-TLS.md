# AOG Control-Plane Node TLS Contract

Status: LSH-C1 identity and provisioning contract. The C2 transport prompt owns listener/client integration; production remains contained until that gate passes.

## Identity invariant

Every AOG Raft node uses one estate-CA-signed leaf certificate for both server and client authentication. Before any socket bind, `aogd` validates all of the following:

- the advertised membership address is a credential-free HTTPS origin;
- the leaf chains to a configured estate CA and is currently valid for both `serverAuth` and `clientAuth`;
- the DNS/IP SAN matches the advertised membership host;
- the URI SAN is exactly `spiffe://loom/node/<AOGD_NODE_ID>`;
- the PKCS#8 private key matches the leaf; and
- the leaf remains valid beyond the configured rotation safety window.

No private-key bytes are formatted, logged, placed in errors, or stored in a printable configuration type.

## Provisioning sources

Choose exactly one source.

### Mounted DER files

Set all three variables:

```text
AOGD_RAFT_CA_DER_PATH=/run/mai/raft/estate-ca.der
AOGD_RAFT_CERT_DER_PATH=/run/mai/raft/node.der
AOGD_RAFT_KEY_DER_PATH=/run/mai/raft/node.key.der
```

The key must be unencrypted PKCS#8 DER and readable only by the `aogd` service account. Partial file configuration is rejected.

### OpenBao KV-v2

Configure the existing `AOGD_OPENBAO_*` AppRole coordinates and set:

```text
AOGD_RAFT_TLS_OPENBAO_PATH=kv/data/loom/nodes/<node-id>/raft-tls
```

The per-node record is separate from `kv/data/loom/trust` and contains these base64-encoded DER fields:

```json
{
  "raft_ca_der": "<base64 DER estate CA>",
  "raft_cert_der": "<base64 DER node leaf>",
  "raft_key_der": "<base64 DER PKCS#8 private key>"
}
```

The node AppRole needs `read` only on its own node record plus the shared application-trust record. It must not read another node's key record.

## Rotation contract

`AOGD_RAFT_TLS_ROTATION_MIN_SECS` defaults to `3600`. Startup refuses a leaf whose remaining lifetime is less than this safety window.

Rotation is a coordinated rolling restart until the C5 live gate proves any stronger reload behavior:

1. Issue a replacement leaf under the same estate CA with the same node-ID URI SAN and advertised-host SAN.
2. Atomically replace the three mounted files or write a new version of the node's OpenBao record.
3. Restart one follower and wait for health and catch-up.
4. Repeat for remaining followers, then transfer leadership and restart the former leader.
5. Revoke or destroy the superseded private key only after every node uses the replacement.

If validation or restart fails, keep production contained and restore the last still-valid identity. Never fall back to plaintext Raft.
