//! System-level closed-loop test for conversation_state persistence (red line 2.3).
//!
//! Verifies the runtime conversation lifecycle writes durable `conversation_state`
//! rows: creating a conversation persists `active`; GC reclaiming it persists
//! `closed`; and the state survives a fresh repo/connection (restart recovery).

use std::sync::Arc;
use std::time::Duration;

use magiclaw::adapters::conversation_store::ConversationStore;
use magiclaw::adapters::sqlite_conversation_state::SqliteConversationStateRepo;
use magiclaw::domain::entities::message::{Direction, Message, MessageContent};
use magiclaw::domain::ports::conversation_queue::{ConversationGC, ConversationQueue};
use magiclaw::domain::ports::conversation_state_repo::ConversationStateRepo;
use magiclaw::domain::value_objects::route_key::{ChannelId, ConversationType, RouteKey};
use magiclaw::infrastructure::config::AppConfig;
use magiclaw::infrastructure::db::{init_db, DbPool};

fn make_route_key() -> RouteKey {
    RouteKey::new(
        ChannelId::new("wechat"),
        "conv_persist",
        "user_a",
        ConversationType::Direct,
    )
}

fn make_msg(key: &RouteKey) -> Message {
    Message {
        id: "m1".into(),
        route_key: key.clone(),
        sequence: Some(1),
        timestamp_ms: 100,
        direction: Direction::Inbound,
        content: MessageContent::Text("hello".into()),
        audit_mark: None,
    }
}

#[tokio::test]
async fn conversation_lifecycle_persists_and_recovers_state() {
    let db = DbPool::new(init_db(":memory:").expect("init db"));
    let repo = Arc::new(SqliteConversationStateRepo::new(db.clone()));
    let store = ConversationStore::new(
        256,
        1800,
        200,
        None,
        AppConfig::default(),
        Some(repo.clone() as Arc<dyn ConversationStateRepo>),
    );

    let key = make_route_key();
    let expected_key = serde_json::to_string(&key).unwrap();

    // Creating a conversation persists state=active.
    store.enqueue(&key, make_msg(&key)).unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;

    let active = repo.load_all().unwrap();
    assert_eq!(active.len(), 1, "one persisted conversation");
    assert_eq!(active[0].route_key, expected_key);
    assert_eq!(active[0].state_json, "\"active\"");

    // GC reclaiming the idle conversation persists state=closed.
    let reclaimed = store.collect_idle(0);
    assert_eq!(reclaimed, 1);

    let closed = repo.load_all().unwrap();
    assert_eq!(closed.len(), 1, "still one row (upsert, not insert)");
    assert_eq!(closed[0].state_json, "\"closed\"");

    // Restart recovery: a fresh repo over the same DB sees the persisted state.
    let recovered_repo = SqliteConversationStateRepo::new(db.clone());
    let recovered = recovered_repo.load_all().unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].route_key, expected_key);
    assert_eq!(recovered[0].state_json, "\"closed\"");
}
