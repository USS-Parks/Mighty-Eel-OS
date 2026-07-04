# Sovereignty Console

One React UI over the **WSF trust plane** (`wsf-api`) and the **AOG gateway**
(`aog-gateway`). Replaces the retired Jinja2 `compliance-dashboard/`. Inherits
the Lamprey Harness "panels" aesthetic (pattern, not code).

Product areas (built across C1–C4):

- **Overview** — trust mode, bundle version, connectivity, audit-chain integrity, tenant summary (C2).
- **Routing & Spend** — local vs cloud routing, per-provider metering, ROI / break-even (C3).
- **Audit** — search the unified receipt ledger (WSF + AOG) by correlation id / tenant / decision / date (C4).

## Develop

```bash
npm install
npm run dev        # http://localhost:5173
npm run typecheck  # tsc --noEmit (app + node configs)
npm run test       # vitest run
npm run build      # typecheck + production bundle → dist/
```

## Configuration

Base URLs come from `VITE_WSF_API_BASE` / `VITE_AOG_BASE` at build time (see
`.env.example`), and are overridable at runtime from the login screen — so the
shadow-mode artifact (D2) can point at a prospect's own gateway without a rebuild.

## Auth

The console authenticates against WSF identity: an operator presents a WSF
**trust token** (JSON), which is verified via `POST /v1/tokens/verify` before the
session is established. Gateway metering endpoints additionally take the caller's
virtual key as a bearer. There is no separate credential store — the trust token
*is* the session.
