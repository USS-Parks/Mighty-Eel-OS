# GitDoctor 75 Public Error Redaction (GD75-06)

**Date:** 2026-05-25  
**Goal:** prevent API error responses from leaking local filesystem paths (Windows or POSIX) or backend-specific identifiers.

## What Changed

- Hardened `mai-api` safe messages so user-facing strings are sanitized consistently:
  - `mai-api/src/errors.rs` now sanitizes details for `BadRequest`, `ValidationFailed`, `PermissionDenied`, `ModelIncompatible`, and `EndpointDisabled`.
- Added path redaction in `sanitize_error_detail(...)`:
  - Any token that looks like a Windows path (`C:\...`) or common POSIX path (`/etc/...`, `/home/...`, `/Users/...`) becomes `<redacted_path>`.

## Test Evidence

- `mai-api/src/errors.rs` includes a unit test:
  - `test_sanitize_redacts_paths`

