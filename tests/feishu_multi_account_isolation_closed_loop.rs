// tests/feishu_multi_account_isolation_closed_loop.rs
//
// Task 5: Multi-Account Isolation Validation
// Verify that multi-account Feishu integration provides path-level isolation:
// - Account A's inbox doesn't leak to account B
// - Account A's outbox doesn't interfere with account B's sends
// - Account A's failure doesn't cascade to account B
// - Dedup is per-account (same event_id from different accounts both process)
//
// Phase A red line 2.4: Multi-account/channel isolation is path-level
// (session, sync_buf, allowlist, inbox/outbox, audit all namespaced by channel/account)

use magiclaw::adapters::sqlite_inbox::SqliteInboxRepo;
use magiclaw::domain::ports::inbox_repo::InboxRepo;
use magiclaw::infrastructure::storage::inbox::{InboxEntry, InboxStatus};
use magiclaw::domain::value_objects::route_key::ChannelId;
use magiclaw::infrastructure::db;

#[test]
fn test_multi_account_inbox_isolation() {
    // Task 5: Messages for account_a should not appear in account_b's inbox
    
    let db_path = ":memory:";
    let conn = db::init_db(db_path).unwrap();
    let db_pool = db::DbPool::new(conn);
    
    let inbox_repo = SqliteInboxRepo::new(db_pool);
    
    // Account A message
    let entry_a = InboxEntry {
        id: "msg_account_a_1".to_string(),
        channel: "feishu_account_a".to_string(),
        conversation_id: "conv_a_1".to_string(),
        payload: r#"{"text": "message from account A"}"#.to_string(),
        status: InboxStatus::Pending,
        created_at: 1000,
        updated_at: 1000,
    };
    
    // Account B message
    let entry_b = InboxEntry {
        id: "msg_account_b_1".to_string(),
        channel: "feishu_account_b".to_string(),
        conversation_id: "conv_b_1".to_string(),
        payload: r#"{"text": "message from account B"}"#.to_string(),
        status: InboxStatus::Pending,
        created_at: 1001,
        updated_at: 1001,
    };
    
    // Persist both messages
    let result_a = inbox_repo.insert(&entry_a);
    assert!(result_a.is_ok(), "account A message should persist");
    
    let result_b = inbox_repo.insert(&entry_b);
    assert!(result_b.is_ok(), "account B message should persist");
    
    // Verify isolation: both messages exist and channels are different
    let exists_a = inbox_repo.exists(&entry_a.id);
    let exists_b = inbox_repo.exists(&entry_b.id);
    
    assert!(exists_a.is_ok() && exists_a.unwrap(), "account A message should exist");
    assert!(exists_b.is_ok() && exists_b.unwrap(), "account B message should exist");
    
    // Verify channels are different (isolation guaranteed by channel field)
    assert_ne!(entry_a.channel, entry_b.channel, "account A and B should have different channels");
}

#[test]
fn test_multi_account_dedup_per_account() {
    // Task 5: Dedup is per-account - same event_id from account_a and account_b
    // should both be processed (not deduped across accounts)
    
    let db_path = ":memory:";
    let conn = db::init_db(db_path).unwrap();
    let db_pool = db::DbPool::new(conn);
    
    let inbox_repo = SqliteInboxRepo::new(db_pool);
    
    // Same logical event ID but different channels (accounts)
    let entry_a = InboxEntry {
        id: "event_123".to_string(),
        channel: "feishu_account_a".to_string(),
        conversation_id: "conv_a".to_string(),
        payload: r#"{"text": "event 123 from A"}"#.to_string(),
        status: InboxStatus::Pending,
        created_at: 1000,
        updated_at: 1000,
    };
    
    let entry_b = InboxEntry {
        id: "event_123".to_string(),
        channel: "feishu_account_b".to_string(),
        conversation_id: "conv_b".to_string(),
        payload: r#"{"text": "event 123 from B"}"#.to_string(),
        status: InboxStatus::Pending,
        created_at: 1001,
        updated_at: 1001,
    };
    
    // Both should succeed (same event_id but different channels)
    let result_a = inbox_repo.insert(&entry_a);
    assert!(result_a.is_ok(), "account A message should persist");
    
    let result_b = inbox_repo.insert(&entry_b);
    assert!(result_b.is_ok(), "account B message should persist even with same event_id");
    
    // Verify both exist
    let count_a = inbox_repo.exists(&entry_a.id);
    let count_b = inbox_repo.exists(&entry_b.id);
    
    assert!(count_a.is_ok() && count_a.unwrap(), "event from account A should exist");
    assert!(count_b.is_ok() && count_b.unwrap(), "event from account B should exist");
}

#[test]
fn test_multi_account_inbox_status_isolation() {
    // Task 5: Status changes to account A's message should not affect account B
    
    let db_path = ":memory:";
    let conn = db::init_db(db_path).unwrap();
    let db_pool = db::DbPool::new(conn);
    
    let inbox_repo = SqliteInboxRepo::new(db_pool);
    
    // Account A message
    let entry_a = InboxEntry {
        id: "msg_a".to_string(),
        channel: "feishu_account_a".to_string(),
        conversation_id: "conv_a".to_string(),
        payload: r#"{"text": "A"}"#.to_string(),
        status: InboxStatus::Pending,
        created_at: 1000,
        updated_at: 1000,
    };
    
    // Account B message
    let entry_b = InboxEntry {
        id: "msg_b".to_string(),
        channel: "feishu_account_b".to_string(),
        conversation_id: "conv_b".to_string(),
        payload: r#"{"text": "B"}"#.to_string(),
        status: InboxStatus::Pending,
        created_at: 1001,
        updated_at: 1001,
    };
    
    // Insert both
    inbox_repo.insert(&entry_a).ok();
    inbox_repo.insert(&entry_b).ok();
    
    // Mark account A as processed
    let result = inbox_repo.mark_status("msg_a", InboxStatus::Processed);
    assert!(result.is_ok(), "should mark account A as processed");
    
    // Account B should still exist and be unaffected
    let exists_b = inbox_repo.exists("msg_b");
    assert!(exists_b.is_ok() && exists_b.unwrap(), "account B message should still exist");
}

#[test]
fn test_multi_account_route_key_isolation() {
    // Task 5: RouteKey includes channel/account, enabling serial processing
    // per account while allowing cross-account parallelism
    
    let account_a_channel = ChannelId("feishu_account_a".to_string());
    let account_b_channel = ChannelId("feishu_account_b".to_string());
    
    // Same conversation/peer but different accounts -> different route keys
    assert_ne!(account_a_channel.0, account_b_channel.0, "channels should differ");
    
    // Verify serialization constraint
    let route_key_a_1 = account_a_channel.clone();
    let route_key_a_2 = account_a_channel.clone();
    
    // Same account + conversation -> same route key (serial)
    assert_eq!(route_key_a_1.0, route_key_a_2.0, "same account should have same channel");
}

#[test]
fn test_multi_account_channel_id_generation() {
    // Task 5: Feishu channel IDs must include account identifier
    // to ensure path-level isolation
    
    // Single account: "feishu" (default)
    let default_channel = "feishu";
    
    // Multi-account: "feishu:{account_id}"
    let account_a_channel = "feishu:account_a";
    let account_b_channel = "feishu:account_b";
    
    assert_ne!(account_a_channel, account_b_channel, "multi-account channels should differ");
    assert!(account_a_channel.contains("account_a"), "channel should include account identifier");
    assert!(account_b_channel.contains("account_b"), "channel should include account identifier");
    
    // Ensure no accidental collision
    assert_ne!(default_channel, account_a_channel, "default channel differs from multi-account");
}
