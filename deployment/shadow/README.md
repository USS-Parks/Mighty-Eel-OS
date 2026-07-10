# Shadow-Mode ROI / Risk Read (D2)

Run this on your own machine, point your OpenAI traffic at it, and get a cost +
risk read on your LLM usage in ~15 minutes — **with zero enforcement**. Nothing
is blocked; no data leaves your box except the OpenAI calls you already make.

It's the AOG gateway in **shadow mode** in front of your OpenAI key: every
request is classified (PHI / PII / secrets), routed (what could stay local),
metered (what it costs), and receipted (a verifiable audit chain) — then passed
straight through to OpenAI, unchanged.

## 1. Set your key + demo secrets

```bash
cd deployment/shadow
cp .env.example .env
# edit .env: set OPENAI_API_KEY, and give the two demo trust-plane
# secrets any random values (e.g. openssl rand -hex 16). They stay on
# this machine — they exist so nothing is baked into the compose file.
```

## 2. Bring it up

```bash
docker compose --profile shadow up --build
```

The first build compiles the services in release (~10–30 min).

## 3. Point your traffic at the gateway

The gateway speaks the OpenAI API. Repoint your app with a base-URL + key change:

```bash
OPENAI_BASE_URL=http://localhost:8080/v1
OPENAI_API_KEY=vk_demo          # the gateway's virtual key — NOT your real key
```

or a raw call:

```bash
curl -s http://localhost:8080/v1/chat/completions \
  -H "Authorization: Bearer vk_demo" -H "Content-Type: application/json" \
  -d '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hello"}]}'
```

Your real OpenAI key stays inside the gateway (from `.env`); your app only ever
presents `vk_demo`. Demo models mapped to your OpenAI: `gpt-4o-mini`, `gpt-4o`.

## 4. Read the dashboard

Open **http://localhost:8088**:

- **Overview** — trust mode `shadow`, connectivity, audit-chain integrity.
- **Routing & Spend** — cost per task / model / provider, local-vs-cloud, and the
  sovereignty dividend (what could have run on-prem for $0).
- **Audit** — the unified receipt ledger, searchable by correlation id.

Nothing here blocks a request — shadow mode only **decides + logs**. Flip
`AOG_MODE=enforce` later (M2) to actually stop classified data from leaving.

## What leaves your machine

Only your existing OpenAI calls. OpenBao is local (dev mode), the gateway adds
no telemetry / phone-home, and the console is served locally. Tear down with
`docker compose --profile shadow down -v`.
