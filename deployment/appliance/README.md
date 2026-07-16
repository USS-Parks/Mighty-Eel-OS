# AOG/WSF Appliance (D1)

One-command bring-up of the M1 **sovereign shadow** stack: the OpenBao trust
core, the WSF trust plane (`wsf-api`), the AOG gateway (shadow mode), and the
console — plus Postgres + MinIO (reserved for service state / evidence) and a
mock on-prem model so a governed request completes end to end.

**Dev-mode OpenBao — not production.** The production HA topology is `../wsf-ha/`.
The whole stack is gated behind the `demo` compose profile, binds only to the
loopback interface, and requires demo secrets to be injected from `.env`. A bare
`docker compose up` starts nothing. Validate before use:
`python validate_profile.py --profile demo docker-compose.yml`.

## Bring-up

```bash
cd deployment/appliance
cp .env.example .env            # then set OPENBAO_DEV_ROOT_TOKEN + WSF_OPENBAO_SECRET_ID
docker compose --profile demo up --build
```

The first build compiles the Rust workspace in release inside the image
(~10–30 min). Startup order is enforced by `depends_on`:
**openbao → seed → (wsf-api, aog-gateway) → console**. The one-shot `seed`
provisions OpenBao (AppRole + KV + Transit + policy), mints the persistent
ML-DSA trust anchor, issues a demo trust token, seeds the `vk_demo` virtual key,
and writes a shared env file the services source before starting.

## Endpoints

| Service | URL |
|---|---|
| Console | http://localhost:8088 |
| AOG gateway (OpenAI/Anthropic-compatible) | http://localhost:8080 |
| WSF API | http://localhost:8081 |
| OpenBao (dev, loopback only; root token from `$OPENBAO_DEV_ROOT_TOKEN`) | http://127.0.0.1:8200 |

## The gate — *a governed request succeeds*

With the stack healthy, drive a governed chat completion through the gateway
with the seeded virtual key. In shadow mode the request is classified, routed,
metered, and receipted — but never blocked:

```bash
curl -s -i http://localhost:8080/v1/chat/completions \
  -H "Authorization: Bearer vk_demo" \
  -H "Content-Type: application/json" \
  -d '{"model":"demo","messages":[{"role":"user","content":"hello"}]}'
```

Expect `200`, an OpenAI `chat.completion` body, and `x-aog-*` governance headers
(`x-aog-route`, `x-aog-classification`, `x-aog-policy-mode: shadow`). Then:

```bash
curl -s http://localhost:8080/v1/status                                   # mode + providers + chain integrity
curl -s http://localhost:8080/v1/usage -H "Authorization: Bearer vk_demo" # metered spend
```

An unknown key returns `401`; an over-budget key `402` — before any model is touched.

## Console login

The console verifies a WSF trust token. Retrieve the seeded demo token:

```bash
docker compose exec wsf-api cat /seed/demo-token.json
```

Open http://localhost:8088, paste it into **Trust token**, set **AOG virtual
key** to `vk_demo`, and **Verify & enter**. Base URLs default to the same-origin
`/api/wsf` + `/api/aog` nginx proxies (no CORS).

## Route to real cloud spend (still governed)

Set provider keys on `aog-gateway` (see `.env.example`) to register real
OpenAI/Anthropic providers; requests stay shadow-governed. This is the basis of
the D2 shadow-mode lead artifact.

Provider destinations are approved before OpenBao access, credential attachment,
or listener bind. Credentialed providers require HTTPS. DNS answers are checked
and pinned for the life of the client, redirects are not followed, and metadata,
link-local, multicast, unspecified, and unapproved private addresses are denied.
`AOG_LOCAL_ALLOWED_ORIGINS` is an exact-origin allowlist for the security-significant
`local` route; `AOG_PRIVATE_PROVIDER_ALLOWED_ORIGINS` separately approves a private
HTTPS origin when a deployment deliberately uses one. The demo's non-loopback HTTP
mock is allowed only because the Compose file declares both its exact local origin
and `AOG_ALLOW_INSECURE_PROVIDER_FIXTURES=1` under the development profile. Never
set that fixture override in production.

## Teardown

```bash
docker compose down -v   # -v also clears the openbao/seed/pg/minio volumes
```
