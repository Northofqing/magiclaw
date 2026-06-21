# Dynamic API Auth Design

## Problem

The current daemon HTTP API uses a single global bearer token from `MAGICLAW_API_TOKEN`. That works for one caller, but it is not a good fit when multiple projects need to call the service concurrently. A single shared token is hard to rotate safely, hard to revoke selectively, and too coarse for future per-endpoint permissions.

## Goals

- Support multiple projects using the same daemon at the same time.
- Give each project its own token identity.
- Support token expiration and token rotation.
- Support endpoint-level scopes so a client can be allowed to call only specific APIs.
- Keep `/api/health` unauthenticated.
- Preserve the existing `Authorization: Bearer ...` request shape for callers.

## Non-Goals

- Full OIDC / SSO integration.
- Browser login or interactive user sessions.
- High-complexity policy engines.

## Proposed Model

Replace the single global token with a project token registry.

### Core concepts

- `project_id`: stable identifier for a calling project.
- `client_name`: optional human-readable label.
- `token`: random opaque bearer token presented by the caller.
- `token_hash`: server-side stored hash of the token, never the raw token.
- `scopes`: a list of allowed capabilities, such as `send`, `window_status`.
- `created_at`: issue time.
- `expires_at`: hard expiration time.
- `revoked_at`: optional revocation time.
- `rotated_from`: optional pointer to the previous token record.

### Policy rules

- A request is authorized only if the token matches a non-revoked, non-expired record.
- The matched record must include the required scope for the requested endpoint.
- During rotation, the old token may remain valid for a short overlap window.
- Rotation should be atomic from the caller's point of view: issue new token first, then revoke old token after the overlap window.

## Data Storage

Add a small auth registry table in SQLite:

```text
api_clients
  id               TEXT PRIMARY KEY
  project_id       TEXT NOT NULL
  client_name      TEXT NOT NULL
  token_hash       TEXT NOT NULL UNIQUE
  scopes           TEXT NOT NULL  -- JSON array
  created_at       INTEGER NOT NULL
  expires_at       INTEGER NOT NULL
  revoked_at       INTEGER NULL
  rotated_from     TEXT NULL
```

Recommended supporting index:

- `token_hash` unique index for fast lookup.
- `project_id` index for management and audit queries.

Token storage policy:

- Store only a hash of the token, not the raw value.
- Use a deterministic, fast hash that is suitable for lookup and operational management.
- Treat the raw token as a secret and surface it only once at issuance.

## Authentication Flow

1. The caller sends `Authorization: Bearer <token>`.
2. The middleware extracts the bearer token.
3. The token is hashed and looked up in the registry.
4. The registry entry must exist, be unrevoked, and be unexpired.
5. The route asks for a scope, such as `send`.
6. If the token lacks the scope, return `403 Forbidden`.
7. If the token is missing or invalid, return `401 Unauthorized`.

## Endpoint Mapping

- `GET /api/health`: no auth.
- `POST /api/send`: requires `send` scope.
- `GET /api/window_status`: requires `window_status` scope.
- Any new protected endpoint must declare its required scope explicitly.

## Rotation Strategy

Token rotation should be supported without breaking existing callers.

Recommended flow:

1. Issue a new token for the same `project_id` and scopes.
2. Keep the old token valid for a short overlap period.
3. Update the caller to use the new token.
4. Revoke the old token after the overlap window or manually if needed.

This avoids a hard cutover and lets projects rotate one by one.

## Migration Path

The daemon should use the project token registry as the only auth source.

Suggested implementation phases:

1. Add registry storage and auth lookup.
2. Add token issuance and revocation commands or admin endpoints.
3. Add expiration and rotation support.
4. Migrate tests and docs to project-based auth.

## Failure Behavior

- Missing token: `401 Unauthorized`.
- Unknown token: `401 Unauthorized`.
- Expired token: `401 Unauthorized`.
- Revoked token: `401 Unauthorized`.
- Valid token but missing scope: `403 Forbidden`.
- `/api/health` stays open even when auth is enabled.

## Audit Requirements

Add audit records for:

- token issuance,
- token rotation,
- token revocation,
- auth failures on protected endpoints,
- scope denials.

Each record should include `project_id`, route, timestamp, and outcome.

## Implementation Notes

- Keep the HTTP shape unchanged so existing clients only need a token swap.
- Keep the auth middleware centralized; do not scatter token checks across handlers.
- Prefer a small registry service that can be reused by future CLI or admin tooling.
- Avoid making auth logic depend on the WeChat runtime state; auth should be independent from message delivery.

## Testing Plan

Add closed-loop tests for:

- `GET /api/health` remains open.
- A valid project token can call allowed endpoints.
- Missing, wrong, expired, and revoked tokens are rejected.
- Scope enforcement returns `403` for unauthorized endpoints.
- Rotation works: old token continues during overlap, then fails after revocation.

## Open Decisions

- Exact storage hash format for token lookup.
- Whether token issuance is exposed through admin CLI, admin HTTP endpoint, or both.
- Whether overlap window duration is fixed or configurable.
