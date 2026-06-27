//! Push job import and execution (Phase A).
//!
//! Import writes push intents into `push_jobs`/`push_job_items`; `run_push_job`
//! resolves each item's targets (broadcast over a project's active bindings, or
//! a validated targeted set) and enqueues one `outbox` row per delivery target,
//! preserving the existing recoverable-delivery semantics.
//! See docs/2026-06-19-project-binding-and-multi-push-design.md.

use serde::{Deserialize, Serialize};

#[allow(deprecated)] // port-based migration pending for push.rs
use crate::application::audit::write_audit;
use crate::application::binding::{DeliveryTarget, ImportSummary};
use crate::domain::entities::message::MessageContent;
use crate::domain::value_objects::route_key::{ChannelId, ConversationType, RouteKey};
use crate::infrastructure::db::DbPool;

/// One push import record (JSONL line or CSV row).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushImportRecord {
    pub project_key: String,
    pub mode: String,
    pub message_text: String,
    #[serde(default)]
    pub target_targets: Vec<DeliveryTarget>,
}

/// Outcome of running a push job.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSummary {
    pub job_id: String,
    pub total_items: usize,
    pub queued_items: usize,
    pub failed_items: usize,
    /// Total number of outbox rows enqueued across all items.
    pub enqueued_messages: usize,
}

fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

fn new_id(prefix: &str) -> String {
    format!("{}-{}", prefix, uuid::Uuid::new_v4())
}

fn parse_conversation_type(value: &str) -> ConversationType {
    match value.trim().to_ascii_lowercase().as_str() {
        "group" => ConversationType::Group,
        "thread" => ConversationType::Thread,
        "bot_session" => ConversationType::BotSession,
        _ => ConversationType::Direct,
    }
}

fn validate_mode(mode: &str) -> Result<(), String> {
    match mode {
        "broadcast" | "targeted" => Ok(()),
        other => Err(format!("invalid mode: {}", other)),
    }
}

/// Create a push job and insert its items (status `pending`).
#[allow(deprecated)] // TODO: port-based migration in follow-up
pub fn import_pushes(db: &DbPool, source_format: &str, path: &str, records: &[PushImportRecord]) -> Result<(String, ImportSummary), String> {
    let job_id = new_id("job");
    let ts = now_ts();
    let format = source_format.to_string();
    let path_owned = path.to_string();
    let job_for_db = job_id.clone();
    db.execute(move |conn| {
        conn.execute(
            "INSERT INTO push_jobs (job_id, source_format, source_path, status, total_items, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'pending', 0, ?4, ?4)",
            rusqlite::params![job_for_db, format, path_owned, ts],
        )?;
        Ok(())
    })
    .map_err(|e| format!("create push job failed: {}", e))?;

    let mut summary = ImportSummary::default();
    for (idx, record) in records.iter().enumerate() {
        summary.total += 1;
        if let Err(e) = insert_push_item(db, &job_id, record) {
            summary.failed += 1;
            summary.errors.push(format!("record {}: {}", idx + 1, e));
        } else {
            summary.success += 1;
        }
    }

    let total = summary.total as i64;
    let job_for_update = job_id.clone();
    db.execute(move |conn| {
        conn.execute(
            "UPDATE push_jobs SET total_items = ?1, updated_at = unixepoch() WHERE job_id = ?2",
            rusqlite::params![total, job_for_update],
        )?;
        Ok(())
    })
    .map_err(|e| format!("update push job totals failed: {}", e))?;

    Ok((job_id, summary))
}

fn insert_push_item(db: &DbPool, job_id: &str, record: &PushImportRecord) -> Result<(), String> {
    if record.project_key.trim().is_empty() {
        return Err("project_key is empty".to_string());
    }
    if record.message_text.trim().is_empty() {
        return Err("message_text is empty".to_string());
    }
    validate_mode(record.mode.trim())?;

    let item_id = new_id("item");
    let job_id = job_id.to_string();
    let project_key = record.project_key.trim().to_string();
    let message_text = record.message_text.clone();
    let mode = record.mode.trim().to_string();
    let targets_json = if record.target_targets.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&record.target_targets).map_err(|e| e.to_string())?)
    };
    let ts = now_ts();
    db.execute(move |conn| {
        conn.execute(
            "INSERT INTO push_job_items
                (item_id, job_id, project_key, message_text, mode, target_targets_json, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7, ?7)",
            rusqlite::params![item_id, job_id, project_key, message_text, mode, targets_json, ts],
        )?;
        Ok(())
    })
    .map_err(|e| format!("insert push item failed: {}", e))
}

/// Parse JSONL push records.
pub fn parse_pushes_jsonl(path: &str) -> Result<Vec<PushImportRecord>, String> {
    let content = std::fs::read_to_string(path).map_err(|e| format!("read {} failed: {}", path, e))?;
    let mut records = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let record: PushImportRecord =
            serde_json::from_str(line).map_err(|e| format!("line {}: parse error: {}", idx + 1, e))?;
        records.push(record);
    }
    Ok(records)
}

/// Parse CSV push records.
///
/// Header: `project_key,mode,target_targets,message_text`.
/// `target_targets` uses `channel:peer_id:conversation_id:conversation_type`
/// segments joined by `|` (empty for broadcast).
pub fn parse_pushes_csv(path: &str) -> Result<Vec<PushImportRecord>, String> {
    let content = std::fs::read_to_string(path).map_err(|e| format!("read {} failed: {}", path, e))?;
    let mut records = Vec::new();
    let mut lines = content.lines();
    let _ = lines.next(); // header
    for (idx, line) in lines.enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let fields = crate::application::binding::parse_csv_line(line);
        if fields.len() < 4 {
            return Err(format!("line {}: expected 4 columns, got {}", idx + 2, fields.len()));
        }
        let target_targets = parse_target_segments(&fields[2])
            .map_err(|e| format!("line {}: {}", idx + 2, e))?;
        records.push(PushImportRecord {
            project_key: fields[0].clone(),
            mode: fields[1].clone(),
            target_targets,
            message_text: fields[3].clone(),
        });
    }
    Ok(records)
}

fn parse_target_segments(value: &str) -> Result<Vec<DeliveryTarget>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(Vec::new());
    }
    let mut targets = Vec::new();
    for seg in value.split('|') {
        let seg = seg.trim();
        if seg.is_empty() {
            continue;
        }
        let parts: Vec<&str> = seg.split(':').collect();
        if parts.len() != 4 {
            return Err(format!("invalid target segment '{}': expected channel:peer:conv:type", seg));
        }
        targets.push(DeliveryTarget {
            channel: parts[0].trim().to_string(),
            peer_id: parts[1].trim().to_string(),
            conversation_id: parts[2].trim().to_string(),
            conversation_type: parts[3].trim().to_string(),
            account_scope: None,
        });
    }
    Ok(targets)
}

/// Resolve the active delivery targets bound to a project (broadcast mode).
fn resolve_broadcast_targets(db: &DbPool, project_key: &str) -> Result<Vec<DeliveryTarget>, String> {
    let project_key = project_key.to_string();
    db.query(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT t.channel, t.peer_id, t.conversation_id, t.conversation_type, t.account_scope
             FROM project_bindings b
             JOIN delivery_targets t ON t.target_id = b.target_id
             WHERE b.project_key = ?1 AND b.status = 'active' AND t.status = 'active'",
        )?;
        let rows: Result<Vec<_>, _> = stmt
            .query_map(rusqlite::params![project_key], |row| {
                let account_scope: String = row.get(4)?;
                Ok(DeliveryTarget {
                    channel: row.get(0)?,
                    peer_id: row.get(1)?,
                    conversation_id: row.get(2)?,
                    conversation_type: row.get(3)?,
                    account_scope: if account_scope.is_empty() { None } else { Some(account_scope) },
                })
            })?
            .collect();
        rows
    })
    .map_err(|e| format!("resolve broadcast targets failed: {}", e))
}

/// The set of active target_ids bound to a project (for targeted validation).
fn active_target_ids(db: &DbPool, project_key: &str) -> Result<std::collections::HashSet<String>, String> {
    let project_key = project_key.to_string();
    db.query(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT b.target_id FROM project_bindings b
             WHERE b.project_key = ?1 AND b.status = 'active'",
        )?;
        let rows: Result<Vec<String>, _> =
            stmt.query_map(rusqlite::params![project_key], |row| row.get(0))?.collect();
        rows.map(|v| v.into_iter().collect())
    })
    .map_err(|e| format!("load active target ids failed: {}", e))
}

fn enqueue_outbox(db: &DbPool, target: &DeliveryTarget, message_text: &str) -> Result<(), String> {
    let route_key = RouteKey::new(
        ChannelId::new(target.channel.clone()),
        target.conversation_id.clone(),
        target.peer_id.clone(),
        parse_conversation_type(&target.conversation_type),
    );
    let route_key_json = serde_json::to_string(&route_key).map_err(|e| e.to_string())?;
    let payload_json = serde_json::to_string(&MessageContent::Text(message_text.to_string())).map_err(|e| e.to_string())?;
    let id = new_id("push");
    let ts = now_ts();
    db.execute(move |conn| {
        conn.execute(
            "INSERT INTO outbox (id, route_key, payload, status, retry_count, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'pending', 0, ?4, ?4)",
            rusqlite::params![id, route_key_json, payload_json, ts],
        )?;
        Ok(())
    })
    .map_err(|e| format!("enqueue outbox failed: {}", e))
}

#[derive(Debug, Clone)]
struct PendingItem {
    item_id: String,
    project_key: String,
    message_text: String,
    mode: String,
    target_targets_json: Option<String>,
}

fn fetch_pending_items(db: &DbPool, job_id: &str) -> Result<Vec<PendingItem>, String> {
    let job_id = job_id.to_string();
    db.query(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT item_id, project_key, message_text, mode, target_targets_json
             FROM push_job_items WHERE job_id = ?1 AND status = 'pending' ORDER BY created_at ASC",
        )?;
        let rows: Result<Vec<_>, _> = stmt
            .query_map(rusqlite::params![job_id], |row| {
                Ok(PendingItem {
                    item_id: row.get(0)?,
                    project_key: row.get(1)?,
                    message_text: row.get(2)?,
                    mode: row.get(3)?,
                    target_targets_json: row.get(4)?,
                })
            })?
            .collect();
        rows
    })
    .map_err(|e| format!("fetch pending items failed: {}", e))
}

fn mark_item(db: &DbPool, item_id: &str, status: &str, error: Option<&str>) -> Result<(), String> {
    let item_id = item_id.to_string();
    let status = status.to_string();
    let error = error.map(|e| e.to_string());
    db.execute(move |conn| {
        conn.execute(
            "UPDATE push_job_items SET status = ?1, error = ?2, updated_at = unixepoch() WHERE item_id = ?3",
            rusqlite::params![status, error, item_id],
        )?;
        Ok(())
    })
    .map_err(|e| format!("mark item failed: {}", e))
}

/// Resolve a single item's delivery targets.
fn resolve_item_targets(db: &DbPool, item: &PendingItem) -> Result<Vec<DeliveryTarget>, String> {
    match item.mode.as_str() {
        "broadcast" => {
            let targets = resolve_broadcast_targets(db, &item.project_key)?;
            if targets.is_empty() {
                return Err("no active bindings for project".to_string());
            }
            Ok(targets)
        }
        "targeted" => {
            let raw = item
                .target_targets_json
                .as_deref()
                .ok_or_else(|| "targeted item has no target_targets".to_string())?;
            let requested: Vec<DeliveryTarget> = serde_json::from_str(raw).map_err(|e| e.to_string())?;
            if requested.is_empty() {
                return Err("targeted item has empty target set".to_string());
            }
            let allowed = active_target_ids(db, &item.project_key)?;
            let (valid, invalid): (Vec<_>, Vec<_>) =
                requested.into_iter().partition(|t| allowed.contains(&t.target_id()));
            if !invalid.is_empty() {
                let names: Vec<String> = invalid.iter().map(|t| t.target_id()).collect();
                return Err(format!("targets not bound to project: {}", names.join(", ")));
            }
            Ok(valid)
        }
        other => Err(format!("invalid mode: {}", other)),
    }
}

/// Run a push job: resolve each pending item's targets and enqueue outbox rows.
#[allow(deprecated)] // TODO: port-based migration in follow-up
pub fn run_push_job(db: &DbPool, job_id: &str) -> Result<RunSummary, String> {
    // Mark job running.
    {
        let job_id = job_id.to_string();
        db.execute(move |conn| {
            conn.execute(
                "UPDATE push_jobs SET status = 'running', updated_at = unixepoch() WHERE job_id = ?1",
                rusqlite::params![job_id],
            )?;
            Ok(())
        })
        .map_err(|e| format!("mark job running failed: {}", e))?;
    }

    let items = fetch_pending_items(db, job_id)?;
    let mut summary = RunSummary {
        job_id: job_id.to_string(),
        total_items: items.len(),
        ..Default::default()
    };

    for item in &items {
        match resolve_item_targets(db, item) {
            Ok(targets) => {
                let mut enqueued = 0usize;
                let mut first_error: Option<String> = None;
                for target in &targets {
                    match enqueue_outbox(db, target, &item.message_text) {
                        Ok(()) => enqueued += 1,
                        Err(e) => {
                            if first_error.is_none() {
                                first_error = Some(e);
                            }
                        }
                    }
                }
                if enqueued > 0 && first_error.is_none() {
                    mark_item(db, &item.item_id, "queued", None)?;
                    summary.queued_items += 1;
                    summary.enqueued_messages += enqueued;
                    write_audit(
                        db,
                        Some(&item.project_key),
                        "project_push",
                        &format!("queued {} ({} targets)", item.mode, enqueued),
                    )
                    .ok();
                } else if enqueued > 0 {
                    // Partial: some targets enqueued, some failed.
                    let err = first_error.unwrap_or_else(|| "partial enqueue".to_string());
                    mark_item(db, &item.item_id, "queued", Some(&format!("partial: {}", err)))?;
                    summary.queued_items += 1;
                    summary.enqueued_messages += enqueued;
                    write_audit(db, Some(&item.project_key), "project_push", &format!("partial: {}", err)).ok();
                } else {
                    let err = first_error.unwrap_or_else(|| "no targets enqueued".to_string());
                    mark_item(db, &item.item_id, "failed", Some(&err))?;
                    summary.failed_items += 1;
                    write_audit(db, Some(&item.project_key), "project_push", &format!("failed: {}", err)).ok();
                }
            }
            Err(e) => {
                mark_item(db, &item.item_id, "failed", Some(&e))?;
                summary.failed_items += 1;
                write_audit(db, Some(&item.project_key), "project_push", &format!("failed: {}", e)).ok();
            }
        }
    }

    // Finalize job status.
    let status = if summary.failed_items == 0 {
        "done"
    } else if summary.queued_items == 0 {
        "failed"
    } else {
        "partial"
    };
    let job_id_owned = job_id.to_string();
    let success = summary.queued_items as i64;
    let failed = summary.failed_items as i64;
    let status_owned = status.to_string();
    db.execute(move |conn| {
        conn.execute(
            "UPDATE push_jobs SET status = ?1, success_items = ?2, failed_items = ?3, updated_at = unixepoch() WHERE job_id = ?4",
            rusqlite::params![status_owned, success, failed, job_id_owned],
        )?;
        Ok(())
    })
    .map_err(|e| format!("finalize job failed: {}", e))?;

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::binding::{apply_binding_record, BindingImportRecord};
    use crate::infrastructure::db::init_db;

    fn pool() -> DbPool {
        DbPool::new(init_db(":memory:").unwrap())
    }

    fn bind(db: &DbPool, project: &str, peer: &str) {
        apply_binding_record(
            db,
            &BindingImportRecord {
                project_key: project.into(),
                channel: "wechat".into(),
                peer_id: peer.into(),
                conversation_id: format!("conv_{}", peer),
                conversation_type: "direct".into(),
                bind_source: "import".into(),
                project_name: None,
                account_scope: None,
            },
        )
        .unwrap();
    }

    fn count_outbox(db: &DbPool) -> i64 {
        db.query(|conn| conn.query_row("SELECT COUNT(*) FROM outbox", [], |r| r.get(0)))
            .unwrap()
    }

    #[test]
    fn broadcast_enqueues_one_per_active_binding() {
        let db = pool();
        bind(&db, "proj_a", "u1");
        bind(&db, "proj_a", "u2");
        let records = vec![PushImportRecord {
            project_key: "proj_a".into(),
            mode: "broadcast".into(),
            message_text: "hello all".into(),
            target_targets: vec![],
        }];
        let (job_id, summary) = import_pushes(&db, "jsonl", "mem", &records).unwrap();
        assert_eq!(summary.success, 1);

        let run = run_push_job(&db, &job_id).unwrap();
        assert_eq!(run.queued_items, 1);
        assert_eq!(run.enqueued_messages, 2);
        assert_eq!(count_outbox(&db), 2);
    }

    #[test]
    fn targeted_validates_against_bindings() {
        let db = pool();
        bind(&db, "proj_a", "u1");
        // u2 NOT bound to proj_a -> targeted should fail the item.
        let records = vec![PushImportRecord {
            project_key: "proj_a".into(),
            mode: "targeted".into(),
            message_text: "core only".into(),
            target_targets: vec![DeliveryTarget {
                channel: "wechat".into(),
                peer_id: "u2".into(),
                conversation_id: "conv_u2".into(),
                conversation_type: "direct".into(),
                account_scope: None,
            }],
        }];
        let (job_id, _) = import_pushes(&db, "jsonl", "mem", &records).unwrap();
        let run = run_push_job(&db, &job_id).unwrap();
        assert_eq!(run.failed_items, 1);
        assert_eq!(count_outbox(&db), 0);
    }

    #[test]
    fn cross_platform_broadcast_fans_out_by_channel() {
        let db = pool();
        bind(&db, "proj_x", "wx_u1");
        apply_binding_record(
            &db,
            &BindingImportRecord {
                project_key: "proj_x".into(),
                channel: "dingtalk".into(),
                peer_id: "dt_u1".into(),
                conversation_id: "dt_conv".into(),
                conversation_type: "direct".into(),
                bind_source: "import".into(),
                project_name: None,
                account_scope: None,
            },
        )
        .unwrap();
        let records = vec![PushImportRecord {
            project_key: "proj_x".into(),
            mode: "broadcast".into(),
            message_text: "multi platform".into(),
            target_targets: vec![],
        }];
        let (job_id, _) = import_pushes(&db, "jsonl", "mem", &records).unwrap();
        let run = run_push_job(&db, &job_id).unwrap();
        assert_eq!(run.enqueued_messages, 2);

        let channels: Vec<String> = db
            .query(|conn| {
                let mut stmt = conn.prepare("SELECT route_key FROM outbox").unwrap();
                let rows: Result<Vec<String>, _> = stmt.query_map([], |r| r.get(0)).unwrap().collect();
                rows
            })
            .unwrap();
        let joined = channels.join(" ");
        assert!(joined.contains("wechat"));
        assert!(joined.contains("dingtalk"));
    }

    #[test]
    fn invalid_mode_record_is_rejected_on_import() {
        let db = pool();
        let records = vec![PushImportRecord {
            project_key: "proj_a".into(),
            mode: "shout".into(),
            message_text: "bad".into(),
            target_targets: vec![],
        }];
        let (_, summary) = import_pushes(&db, "jsonl", "mem", &records).unwrap();
        assert_eq!(summary.failed, 1);
    }
}
