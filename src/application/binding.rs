//! Project ↔ delivery-target binding store and file import (Phase A).
//!
//! Projects are owned by an external system; aiclaw only caches the project key
//! and stores the many-to-many binding between a project and cross-platform
//! delivery targets. See docs/2026-06-19-project-binding-and-multi-push-design.md.

use serde::{Deserialize, Serialize};

use crate::infrastructure::db::DbPool;

/// A cross-platform delivery endpoint (one row in `delivery_targets`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryTarget {
    pub channel: String,
    pub peer_id: String,
    pub conversation_id: String,
    pub conversation_type: String,
    #[serde(default)]
    pub account_scope: Option<String>,
}

impl DeliveryTarget {
    /// Deterministic id derived from the UNIQUE tuple, so re-importing the same
    /// endpoint is idempotent without a read-back round trip.
    pub fn target_id(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}",
            self.channel,
            self.peer_id,
            self.conversation_id,
            self.conversation_type,
            self.account_scope.as_deref().unwrap_or(""),
        )
    }

    fn account_scope_value(&self) -> String {
        self.account_scope.clone().unwrap_or_default()
    }

    fn validate(&self) -> Result<(), String> {
        if self.channel.trim().is_empty() {
            return Err("channel is empty".to_string());
        }
        if self.peer_id.trim().is_empty() {
            return Err("peer_id is empty".to_string());
        }
        if self.conversation_id.trim().is_empty() {
            return Err("conversation_id is empty".to_string());
        }
        if self.conversation_type.trim().is_empty() {
            return Err("conversation_type is empty".to_string());
        }
        Ok(())
    }
}

/// One binding import record (JSONL line or CSV row).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindingImportRecord {
    pub project_key: String,
    pub channel: String,
    pub peer_id: String,
    pub conversation_id: String,
    pub conversation_type: String,
    #[serde(default = "default_bind_source")]
    pub bind_source: String,
    #[serde(default)]
    pub project_name: Option<String>,
    #[serde(default)]
    pub account_scope: Option<String>,
}

fn default_bind_source() -> String {
    "import".to_string()
}

impl BindingImportRecord {
    fn target(&self) -> DeliveryTarget {
        DeliveryTarget {
            channel: self.channel.trim().to_string(),
            peer_id: self.peer_id.trim().to_string(),
            conversation_id: self.conversation_id.trim().to_string(),
            conversation_type: self.conversation_type.trim().to_string(),
            account_scope: self.account_scope.clone(),
        }
    }
}

/// Summary of a file import operation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportSummary {
    pub total: usize,
    pub success: usize,
    pub failed: usize,
    pub errors: Vec<String>,
}

/// A project row for listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectRow {
    pub project_key: String,
    pub project_name: String,
    pub binding_count: i64,
}

/// A binding row for listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindingRow {
    pub target_id: String,
    pub channel: String,
    pub peer_id: String,
    pub conversation_id: String,
    pub conversation_type: String,
    pub bind_source: String,
    pub status: String,
}

fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Ensure the project exists (cache of the external project key).
///
/// A missing/blank `project_name` never overwrites an existing name: for a brand
/// new project it defaults to the key, but for an existing project it is left
/// untouched so later records without a name don't clobber an earlier one.
pub fn ensure_project(db: &DbPool, project_key: &str, project_name: Option<&str>) -> Result<(), String> {
    let key = project_key.trim().to_string();
    if key.is_empty() {
        return Err("project_key is empty".to_string());
    }
    let explicit_name = project_name
        .map(|n| n.trim())
        .filter(|n| !n.is_empty())
        .map(|n| n.to_string());
    let has_explicit = explicit_name.is_some();
    let insert_name = explicit_name.unwrap_or_else(|| key.clone());
    let ts = now_ts();
    db.execute(move |conn| {
        conn.execute(
            "INSERT INTO projects (project_key, project_name, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT(project_key) DO UPDATE SET
                 project_name = CASE WHEN ?4 THEN excluded.project_name ELSE projects.project_name END,
                 updated_at = excluded.updated_at",
            rusqlite::params![key, insert_name, ts, has_explicit],
        )?;
        Ok(())
    })
    .map_err(|e| format!("ensure_project failed: {}", e))
}

/// Upsert a delivery target, returning its deterministic target_id.
pub fn upsert_delivery_target(db: &DbPool, target: &DeliveryTarget) -> Result<String, String> {
    target.validate()?;
    let target_id = target.target_id();
    let channel = target.channel.clone();
    let peer_id = target.peer_id.clone();
    let conversation_id = target.conversation_id.clone();
    let conversation_type = target.conversation_type.clone();
    let account_scope = target.account_scope_value();
    let id_for_db = target_id.clone();
    let ts = now_ts();
    db.execute(move |conn| {
        conn.execute(
            "INSERT INTO delivery_targets
                (target_id, channel, peer_id, conversation_id, conversation_type, account_scope, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', ?7, ?7)
             ON CONFLICT(channel, peer_id, conversation_id, conversation_type, account_scope)
             DO UPDATE SET status = 'active', updated_at = excluded.updated_at",
            rusqlite::params![id_for_db, channel, peer_id, conversation_id, conversation_type, account_scope, ts],
        )?;
        Ok(())
    })
    .map_err(|e| format!("upsert_delivery_target failed: {}", e))?;
    Ok(target_id)
}

/// Upsert an active binding between a project and a delivery target.
pub fn upsert_binding(db: &DbPool, project_key: &str, target_id: &str, bind_source: &str) -> Result<(), String> {
    let project_key = project_key.trim().to_string();
    let target_id = target_id.to_string();
    let bind_source = bind_source.trim().to_string();
    let id = format!("{}|{}", project_key, target_id);
    let ts = now_ts();
    db.execute(move |conn| {
        conn.execute(
            "INSERT INTO project_bindings
                (id, project_key, target_id, bind_source, status, bound_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 'active', ?5, ?5, ?5)
             ON CONFLICT(project_key, target_id)
             DO UPDATE SET status = 'active', bind_source = excluded.bind_source, unbound_at = NULL, updated_at = excluded.updated_at",
            rusqlite::params![id, project_key, target_id, bind_source, ts],
        )?;
        Ok(())
    })
    .map_err(|e| format!("upsert_binding failed: {}", e))
}

/// Apply a single binding record (project + target + binding) idempotently.
pub fn apply_binding_record(db: &DbPool, record: &BindingImportRecord) -> Result<(), String> {
    if record.project_key.trim().is_empty() {
        return Err("project_key is empty".to_string());
    }
    ensure_project(db, &record.project_key, record.project_name.as_deref())?;
    let target = record.target();
    let target_id = upsert_delivery_target(db, &target)?;
    let bind_source = if record.bind_source.trim().is_empty() {
        "import"
    } else {
        record.bind_source.trim()
    };
    upsert_binding(db, record.project_key.trim(), &target_id, bind_source)
}

/// Import bindings from a JSONL file (one JSON object per line).
pub fn import_bindings_jsonl(db: &DbPool, path: &str) -> Result<ImportSummary, String> {
    let content = std::fs::read_to_string(path).map_err(|e| format!("read {} failed: {}", path, e))?;
    let mut summary = ImportSummary::default();
    for (idx, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        summary.total += 1;
        match serde_json::from_str::<BindingImportRecord>(line) {
            Ok(record) => match apply_binding_record(db, &record) {
                Ok(()) => summary.success += 1,
                Err(e) => {
                    summary.failed += 1;
                    summary.errors.push(format!("line {}: {}", idx + 1, e));
                }
            },
            Err(e) => {
                summary.failed += 1;
                summary.errors.push(format!("line {}: parse error: {}", idx + 1, e));
            }
        }
    }
    Ok(summary)
}

/// Import bindings from a CSV file.
///
/// Header: `project_key,channel,peer_id,conversation_id,conversation_type,bind_source`
/// (`bind_source` optional; extra trailing columns ignored).
pub fn import_bindings_csv(db: &DbPool, path: &str) -> Result<ImportSummary, String> {
    let content = std::fs::read_to_string(path).map_err(|e| format!("read {} failed: {}", path, e))?;
    let mut summary = ImportSummary::default();
    let mut lines = content.lines();
    // Skip header row.
    let _ = lines.next();
    for (idx, line) in lines.enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        summary.total += 1;
        let fields = parse_csv_line(line);
        if fields.len() < 5 {
            summary.failed += 1;
            summary.errors.push(format!("line {}: expected >=5 columns, got {}", idx + 2, fields.len()));
            continue;
        }
        let record = BindingImportRecord {
            project_key: fields[0].clone(),
            channel: fields[1].clone(),
            peer_id: fields[2].clone(),
            conversation_id: fields[3].clone(),
            conversation_type: fields[4].clone(),
            bind_source: fields.get(5).cloned().filter(|s| !s.trim().is_empty()).unwrap_or_else(default_bind_source),
            project_name: None,
            account_scope: None,
        };
        match apply_binding_record(db, &record) {
            Ok(()) => summary.success += 1,
            Err(e) => {
                summary.failed += 1;
                summary.errors.push(format!("line {}: {}", idx + 2, e));
            }
        }
    }
    Ok(summary)
}

/// List projects with their active binding counts.
pub fn list_projects(db: &DbPool) -> Result<Vec<ProjectRow>, String> {
    db.query(|conn| {
        let mut stmt = conn.prepare(
            "SELECT p.project_key, p.project_name,
                    (SELECT COUNT(*) FROM project_bindings b WHERE b.project_key = p.project_key AND b.status = 'active')
             FROM projects p ORDER BY p.project_key ASC",
        )?;
        let rows: Result<Vec<_>, _> = stmt
            .query_map([], |row| {
                Ok(ProjectRow {
                    project_key: row.get(0)?,
                    project_name: row.get(1)?,
                    binding_count: row.get(2)?,
                })
            })?
            .collect();
        rows
    })
    .map_err(|e| format!("list_projects failed: {}", e))
}

/// List active bindings for a project.
pub fn list_bindings(db: &DbPool, project_key: &str) -> Result<Vec<BindingRow>, String> {
    let project_key = project_key.to_string();
    db.query(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT t.target_id, t.channel, t.peer_id, t.conversation_id, t.conversation_type, b.bind_source, b.status
             FROM project_bindings b
             JOIN delivery_targets t ON t.target_id = b.target_id
             WHERE b.project_key = ?1 AND b.status = 'active'
             ORDER BY t.channel, t.peer_id",
        )?;
        let rows: Result<Vec<_>, _> = stmt
            .query_map(rusqlite::params![project_key], |row| {
                Ok(BindingRow {
                    target_id: row.get(0)?,
                    channel: row.get(1)?,
                    peer_id: row.get(2)?,
                    conversation_id: row.get(3)?,
                    conversation_type: row.get(4)?,
                    bind_source: row.get(5)?,
                    status: row.get(6)?,
                })
            })?
            .collect();
        rows
    })
    .map_err(|e| format!("list_bindings failed: {}", e))
}

/// Minimal RFC4180-style single-line CSV parser.
///
/// Supports double-quoted fields with embedded commas and `""` escapes.
/// Records spanning multiple physical lines are not supported (Phase A constraint).
pub(crate) fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
        } else {
            match c {
                '"' => in_quotes = true,
                ',' => {
                    fields.push(std::mem::take(&mut field));
                }
                _ => field.push(c),
            }
        }
    }
    fields.push(field);
    fields.into_iter().map(|f| f.trim().to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::db::init_db;

    fn pool() -> DbPool {
        DbPool::new(init_db(":memory:").unwrap())
    }

    #[test]
    fn parse_csv_line_handles_quotes_and_commas() {
        let fields = parse_csv_line(r#"a,"b,c",d,"e""f""#);
        assert_eq!(fields, vec!["a", "b,c", "d", r#"e"f"#]);
    }

    #[test]
    fn apply_binding_record_is_idempotent() {
        let db = pool();
        let record = BindingImportRecord {
            project_key: "proj_a".into(),
            channel: "wechat".into(),
            peer_id: "u1".into(),
            conversation_id: "conv_u1".into(),
            conversation_type: "direct".into(),
            bind_source: "import".into(),
            project_name: Some("Project A".into()),
            account_scope: None,
        };
        apply_binding_record(&db, &record).unwrap();
        apply_binding_record(&db, &record).unwrap();

        let bindings = list_bindings(&db, "proj_a").unwrap();
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].channel, "wechat");

        let projects = list_projects(&db).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].binding_count, 1);
    }

    #[test]
    fn missing_name_does_not_clobber_existing_project_name() {
        let db = pool();
        ensure_project(&db, "proj_a", Some("Project A")).unwrap();
        ensure_project(&db, "proj_a", None).unwrap();
        let projects = list_projects(&db).unwrap();
        assert_eq!(projects[0].project_name, "Project A");
    }

    #[test]
    fn binding_record_with_empty_project_key_fails() {
        let db = pool();
        let record = BindingImportRecord {
            project_key: "  ".into(),
            channel: "wechat".into(),
            peer_id: "u1".into(),
            conversation_id: "conv_u1".into(),
            conversation_type: "direct".into(),
            bind_source: "import".into(),
            project_name: None,
            account_scope: None,
        };
        assert!(apply_binding_record(&db, &record).is_err());
    }

    #[test]
    fn many_to_many_binding_supported() {        let db = pool();
        // One project bound to two users (multi-push), one user in two projects.
        for (proj, peer) in [("proj_a", "u1"), ("proj_a", "u2"), ("proj_b", "u1")] {
            let record = BindingImportRecord {
                project_key: proj.into(),
                channel: "wechat".into(),
                peer_id: peer.into(),
                conversation_id: format!("conv_{}", peer),
                conversation_type: "direct".into(),
                bind_source: "import".into(),
                project_name: None,
                account_scope: None,
            };
            apply_binding_record(&db, &record).unwrap();
        }
        assert_eq!(list_bindings(&db, "proj_a").unwrap().len(), 2);
        assert_eq!(list_bindings(&db, "proj_b").unwrap().len(), 1);
    }
}
