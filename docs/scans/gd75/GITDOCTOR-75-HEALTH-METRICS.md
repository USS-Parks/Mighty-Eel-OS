# GitDoctor 75 Health + Metrics Evidence (GD75-11)

**Date:** 2026-05-25  
**Goal:** make health monitoring and observability controls obvious to reviewers and scanners.

## HTTP Endpoints (auth-exempt)

- Health routes are mounted in `mai-api/src/routes.rs`:
  - `GET /v1/health`
  - `GET /v1/health/adapters`
  - `GET /v1/health/hardware`
  - `GET /v1/health/resources`
  - `GET /v1/health/system`
  - `GET /v1/health/live`
  - `GET /v1/health/ready`
  - `GET /v1/health/production`
- Prometheus metrics endpoint:
  - `GET /v1/metrics` (`mai-api/src/handlers/metrics.rs`)

## Acceptance Test Coverage

SHIP-11 observability acceptance tests live in:
- `mai-api/tests/ship_11_observability.rs`

Those tests cover (at minimum):
- `/v1/metrics` does not expose secrets
- `/v1/health/ready` and `/v1/health/production` fail closed when critical dependencies fail
- request correlation ID handling

