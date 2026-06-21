# Dynamic API Auth Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the single daemon-wide API token with project-level bearer tokens that can expire, rotate, and carry endpoint scopes.

**Architecture:** Store API clients in SQLite, validate bearer tokens against the registry in the HTTP auth middleware, and add local CLI commands for issuing, listing, and revoking tokens. Keep `/api/health` open, scope protected routes explicitly, and record auth failures for auditing.

**Tech Stack:** Rust, axum, rusqlite, serde, existing CLI/runtime layout.

---

### Task 1: Add API client registry storage

**Files:**
- Modify: `src/infrastructure/db.rs`
- Test: `src/infrastructure/db.rs` unit tests

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn init_db_creates_api_clients_table() {
    let conn = init_db(":memory:").unwrap();
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    assert!(tables.contains(&"api_clients".to_string()));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test init_db_creates_api_clients_table -v`
Expected: FAIL because `api_clients` does not exist yet.

- [ ] **Step 3: Write minimal implementation**

Add the table and supporting indexes:

```sql
CREATE TABLE IF NOT EXISTS api_clients (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    client_name TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    scopes TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    revoked_at INTEGER,
    rotated_from TEXT
);

CREATE INDEX IF NOT EXISTS idx_api_clients_project ON api_clients (project_id, revoked_at, expires_at);
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test init_db_creates_api_clients_table -v`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/infrastructure/db.rs
git commit -m "feat: add api client registry table"
```

### Task 2: Implement token registry and bearer auth lookup

**Files:**
- Create: `src/adapters/api_client_registry.rs`
- Modify: `src/adapters/http_auth.rs`
- Modify: `src/infrastructure/runtime.rs`
- Modify: `src/adapters/mod.rs`
- Test: `src/adapters/http_auth.rs` unit tests

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn bearer_auth_requires_a_registry_match() {
    let registry = ApiClientRegistry::new_in_memory();
    assert!(!registry.is_authorized("send", "Bearer abc"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test bearer_auth_requires_a_registry_match -v`
Expected: FAIL because the registry type does not exist yet.

- [ ] **Step 3: Write minimal implementation**

Create a registry service that:

```rust
pub struct ApiClientRegistry {
    db: Arc<DbPool>,
}

pub struct ApiClientRecord {
    pub project_id: String,
    pub client_name: String,
    pub scopes: Vec<String>,
    pub expires_at: i64,
    pub revoked_at: Option<i64>,
}

impl ApiClientRegistry {
    pub fn lookup_token(&self, token: &str) -> Result<Option<ApiClientRecord>, String> { /* hash + SELECT */ }
    pub fn is_authorized(&self, token: &str, required_scope: &str) -> bool { /* validate record + scope */ }
}
```

Update the auth middleware to call the registry and return `401` for missing/invalid/expired/revoked tokens and `403` for missing scope.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test bearer_auth_requires_a_registry_match -v`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/adapters/api_client_registry.rs src/adapters/http_auth.rs src/adapters/mod.rs src/infrastructure/runtime.rs
git commit -m "feat: add dynamic api token registry"
```

### Task 3: Add CLI token management commands

**Files:**
- Modify: `src/main.rs`
- Test: existing CLI tests or new command tests under `tests/`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn auth_issue_command_is_parsed() {
    let args = vec![
        "magiclaw".to_string(),
        "auth".to_string(),
        "issue".to_string(),
        "--project".to_string(),
        "proj-a".to_string(),
        "--scope".to_string(),
        "send".to_string(),
    ];
    assert!(matches!(parse_cli_args(&args), Ok(CliCommand::AuthIssue(_))));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test auth_issue_command_is_parsed -v`
Expected: FAIL because `auth` commands are not parsed yet.

- [ ] **Step 3: Write minimal implementation**

Add CLI subcommands for:

```text
magiclaw auth issue --project <project_id> --name <client_name> --scopes send,window_status --ttl-secs 86400
magiclaw auth list --project <project_id>
magiclaw auth revoke --token <raw_token>
```

`auth issue` should print the raw token once and insert the hashed record into SQLite.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test auth_issue_command_is_parsed -v`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/main.rs tests/
git commit -m "feat: add api token management cli"
```

### Task 4: Add end-to-end auth coverage for issue/expire/revoke/scopes

**Files:**
- Modify: `tests/daemon_api_auth_closed_loop.rs`
- Create: `tests/daemon_api_dynamic_auth_closed_loop.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn project_token_can_call_allowed_endpoint_and_is_rejected_after_revoke() {
    // spawn daemon, issue token via CLI, call allowed endpoint, revoke token, verify 401
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test project_token_can_call_allowed_endpoint_and_is_rejected_after_revoke -v`
Expected: FAIL until the dynamic auth path exists.

- [ ] **Step 3: Write minimal implementation**

Cover:

- `GET /api/health` remains open.
- valid project token returns `200` on allowed endpoint.
- wrong / expired / revoked token returns `401`.
- missing scope returns `403`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test project_token_can_call_allowed_endpoint_and_is_rejected_after_revoke -v`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add tests/daemon_api_dynamic_auth_closed_loop.rs tests/daemon_api_auth_closed_loop.rs
git commit -m "test: cover dynamic api token auth flow"
```
