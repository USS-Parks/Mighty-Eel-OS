# MAI Appliance TLS Architecture

## Current State

The mai-api server binds plain TCP (`axum::serve(listener, router)`) and
relies on the ship profile's `tls_mode` field to declare its posture:

| tls_mode             | Meaning                                        |
|----------------------|------------------------------------------------|
| `direct`             | No TLS; direct plain-text HTTP (staging)       |
| `reverse-proxy-required` | TLS terminated by an external reverse proxy (production) |

The `TlsConfig` struct in `ship_profile.rs` (`cert_path`, `key_path`) is
parsed from TOML but never consumed to configure `axum::serve()`. It
exists as a placeholder for a future session that wires `rustls` or
`native-tls` into the axum stack.

## Ring-1 → Ring-3 (mai-api → OpenBao)

The bridge to OpenBao supports two modes:

| Mode    | Transport       | Auth                       |
|---------|-----------------|----------------------------|
| Plain   | HTTP            | AppRole bearer token       |
| Server TLS | HTTPS        | AppRole token + server cert verification |
| mTLS    | HTTPS + client cert | AppRole token + TLS identity (pending) |

**Server TLS** is enabled via `start-openbao.ps1 -TlsEnabled`, which uses
OpenBao's built-in `-dev-tls` flag. This generates a CA, server certificate,
and key inside the container and exposes them in the `openbao-tls/` host
directory. The mai-api bridge client connects to `https://localhost:8200`.

**mTLS** (mutual TLS) is built via `OpenBaoBridgeClient::new_with_mtls()`.
The client presents a certificate issued by OpenBao's PKI engine and trusts
the staging CA. This is implemented but not yet end-to-end tested — it
requires the bridge client to issue its own cert from the PKI engine and
configure the reqwest client with the resulting PEM identity.

### Enabling Server TLS (staging)

```powershell
# Start OpenBao with TLS
.\start-openbao.ps1 -TlsEnabled

# Set environment for mai-api
$env:MAI_OPENBAO_ADDR = "https://localhost:8200"
$env:MAI_OPENBAO_SECRET_ID = "<secret from start-openbao output>"
```

### Production considerations

1. **Certificate rotation**: OpenBao PKI issues certs with 24h TTL.
   A background task should refresh the client certificate before expiry.
2. **-dev-tls is dev only**: The `-dev-tls` flag generates self-signed
   certs and must never be used in production. Production must use a
   real CA (enterprise, Let's Encrypt) with properly configured listeners.
3. **Server TLS**: Application-level TLS termination (via `rustls` or
   `native-tls` in `axum_server`) remains deferred. Production deploys
   behind nginx/caddy or a cloud load balancer that terminates TLS.
4. **mTLS E2E**: The `new_with_mtls()` constructor and PKI cert issuance
   client are built but not yet tested end-to-end with the OpenBao PKI
   engine. A follow-up session should complete the full mTLS handshake
   flow: issue appliance cert → configure reqwest → verify mutual auth.

## Decision Record

- **Why -dev-tls instead of manual certs?** OpenBao's `-dev-tls` flag
  generates CA, server cert, and key inside the container with zero
  external dependencies (no OpenSSL required on the host). Manual cert
  generation with OpenSSL caused PS 5.1 here-string syntax issues and
  stderr suppression problems. `-dev-tls` handles everything internally.
- **Why rustls for the bridge?** `reqwest` 0.12 with `rustls-tls`
  provides `Identity::from_pem()` which is not available with
  `native-tls`. Pure-Rust, no system library dependencies.
- **Why PEM certs?** OpenBao's PKI engine emits PEM. PKCS12 would
  require an additional conversion step.
- **Why not TLS in the app?** Keeping TLS at the reverse proxy layer
  separates concerns: the app focuses on mTLS to OpenBao (east-west),
  the proxy handles client-facing TLS (north-south). This matches
  the three-layer manifold architecture where Ring-1's perimeter is
  the reverse proxy.
