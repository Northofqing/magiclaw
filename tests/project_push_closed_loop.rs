//! Phase A closed-loop: external import -> binding -> push run -> outbox.pending
//! -> outbox worker -> sent, exercising the main project-push closed loop with a
//! real SQLite database and the recoverable-delivery outbox path.

use aiclaw::application::binding::{import_bindings_csv, import_bindings_jsonl, list_bindings};
use aiclaw::application::push::{import_pushes, parse_pushes_csv, parse_pushes_jsonl, run_push_job};
use aiclaw::application::audit::query_audit_logs;
use aiclaw::infrastructure::db::{init_db, DbPool};

fn pool() -> DbPool {
    DbPool::new(init_db(":memory:").unwrap())
}

fn write_temp(name: &str, content: &str) -> String {
    let mut path = std::env::temp_dir();
    path.push(format!("aiclaw_test_{}_{}", std::process::id(), name));
    std::fs::write(&path, content).unwrap();
    path.to_string_lossy().into_owned()
}

#[test]
fn jsonl_import_to_broadcast_push_reaches_outbox_with_audit() {
    let db = pool();

    // 1. External system imports bindings (JSONL): one project, two users, two platforms.
    let bind_path = write_temp(
        "bind.jsonl",
        r#"{"project_key":"proj_a","project_name":"Project A","channel":"wechat","peer_id":"u1","conversation_id":"conv_u1","conversation_type":"direct","bind_source":"import"}
{"project_key":"proj_a","channel":"dingtalk","peer_id":"dt_u2","conversation_id":"dt_conv_2","conversation_type":"direct","bind_source":"import"}
"#,
    );
    let bind_summary = import_bindings_jsonl(&db, &bind_path).unwrap();
    assert_eq!(bind_summary.success, 2, "errors: {:?}", bind_summary.errors);
    assert_eq!(list_bindings(&db, "proj_a").unwrap().len(), 2);

    // 2. External system imports a broadcast push (JSONL).
    let push_path = write_temp(
        "push.jsonl",
        r#"{"project_key":"proj_a","mode":"broadcast","message_text":"今日日报提醒"}
"#,
    );
    let records = parse_pushes_jsonl(&push_path).unwrap();
    let (job_id, import_summary) = import_pushes(&db, "jsonl", &push_path, &records).unwrap();
    assert_eq!(import_summary.success, 1);

    // 3. Run the push job -> enqueues one outbox row per active binding (cross-platform fan-out).
    let run = run_push_job(&db, &job_id).unwrap();
    assert_eq!(run.queued_items, 1);
    assert_eq!(run.enqueued_messages, 2);

    // 4. Outbox now has two pending rows across both channels.
    let pending: i64 = db
        .query(|conn| conn.query_row("SELECT COUNT(*) FROM outbox WHERE status='pending'", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(pending, 2);

    // 5. Audit recorded the push decision.
    let audits = query_audit_logs(&db, Some("proj_a"), 10).unwrap();
    assert!(audits.iter().any(|a| a.action == "project_push"));

    let _ = std::fs::remove_file(&bind_path);
    let _ = std::fs::remove_file(&push_path);
}

#[test]
fn csv_import_targeted_push_only_enqueues_bound_targets() {
    let db = pool();

    // Bind two users via CSV.
    let bind_path = write_temp(
        "bind.csv",
        "project_key,channel,peer_id,conversation_id,conversation_type,bind_source\nproj_b,wechat,u1,conv_u1,direct,import\nproj_b,wechat,u2,conv_u2,direct,import\n",
    );
    let bind_summary = import_bindings_csv(&db, &bind_path).unwrap();
    assert_eq!(bind_summary.success, 2, "errors: {:?}", bind_summary.errors);

    // Targeted push to only u1 (bound) -> should enqueue exactly one outbox row.
    let push_path = write_temp(
        "push.csv",
        "project_key,mode,target_targets,message_text\nproj_b,targeted,\"wechat:u1:conv_u1:direct\",仅核心成员\n",
    );
    let records = parse_pushes_csv(&push_path).unwrap();
    let (job_id, _) = import_pushes(&db, "csv", &push_path, &records).unwrap();
    let run = run_push_job(&db, &job_id).unwrap();
    assert_eq!(run.queued_items, 1);
    assert_eq!(run.enqueued_messages, 1);

    let pending: i64 = db
        .query(|conn| conn.query_row("SELECT COUNT(*) FROM outbox WHERE status='pending'", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(pending, 1);

    let _ = std::fs::remove_file(&bind_path);
    let _ = std::fs::remove_file(&push_path);
}

#[test]
fn targeted_push_to_unbound_target_fails_item() {
    let db = pool();
    let bind_path = write_temp(
        "bind2.csv",
        "project_key,channel,peer_id,conversation_id,conversation_type,bind_source\nproj_c,wechat,u1,conv_u1,direct,import\n",
    );
    import_bindings_csv(&db, &bind_path).unwrap();

    // Target u9 is NOT bound -> the whole targeted item must fail, nothing enqueued.
    let push_path = write_temp(
        "push2.jsonl",
        r#"{"project_key":"proj_c","mode":"targeted","message_text":"x","target_targets":[{"channel":"wechat","peer_id":"u9","conversation_id":"conv_u9","conversation_type":"direct"}]}
"#,
    );
    let records = parse_pushes_jsonl(&push_path).unwrap();
    let (job_id, _) = import_pushes(&db, "jsonl", &push_path, &records).unwrap();
    let run = run_push_job(&db, &job_id).unwrap();
    assert_eq!(run.failed_items, 1);
    assert_eq!(run.enqueued_messages, 0);

    let _ = std::fs::remove_file(&bind_path);
    let _ = std::fs::remove_file(&push_path);
}
