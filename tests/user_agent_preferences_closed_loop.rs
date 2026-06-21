//! Integration tests for Phase A+ user agent preference switching.
//!
//! This test suite verifies the complete closed-loop behavior:
//! 1. User sends agent switching command (e.g., "cc", "/cc claude code")
//! 2. Preference is persisted to the database
//! 3. Future messages use the saved preference
//! 4. AiMiddleware respects the user's agent selection

use magiclaw::application::agent_preferences;
use magiclaw::core::pipeline::agent_command_middleware::AgentCommandMiddleware;
use magiclaw::core::pipeline::agent_command::AgentCommandParser;
use magiclaw::core::pipeline::middleware::{Middleware, PipelineContext};
use magiclaw::domain::entities::message::{Direction, Message, MessageContent};
use magiclaw::domain::aggregates::conversation::Conversation;
use magiclaw::domain::value_objects::route_key::{ChannelId, ConversationType, RouteKey};
use magiclaw::infrastructure::config::{AppConfig, default_agent_aliases};
use magiclaw::infrastructure::db::{self, DbPool};
use std::collections::HashMap;

#[tokio::test]
async fn test_user_agent_preference_closed_loop() {
    // Setup: Create an in-memory database
    let conn = db::init_db(":memory:")
        .expect("Failed to init DB");
    let db_pool = DbPool::new(conn);

    // Create config with agent aliases
    let mut config = AppConfig::default();
    config.agent.enable_user_preferences = true;
    
    // Create middleware
    let middleware = AgentCommandMiddleware::new(db_pool.clone(), config.agent.clone());

    // Test 1: User sends "cc" command
    let rk = RouteKey::new(
        ChannelId::new("wechat"),
        "conv_123",
        "user_456",
        ConversationType::Direct,
    );
    let ctx = PipelineContext {
        message: Message {
            id: "msg_1".into(),
            route_key: rk.clone(),
            sequence: None,
            timestamp_ms: 1000,
            direction: Direction::Inbound,
            content: MessageContent::Text("cc".into()),
            audit_mark: None,
        },
        conversation: Conversation::new(rk.clone(), 100),
        config: config.clone(),
        ai_response: None,
        short_circuit: false,
        user_agent_selection: None,
    };

    // Process the command
    let result = middleware.process(ctx).await.expect("Middleware failed");
    
    // Verify: Command was recognized and short-circuited with confirmation
    assert!(result.short_circuit, "Command should short-circuit");
    assert!(result.ai_response.is_some(), "Should have response");
    let response = result.ai_response.as_ref().unwrap();
    assert!(response.contains("claude_code"), "Response should mention claude_code");

    // Verify: Preference was persisted
    let saved_pref = agent_preferences::get_user_agent(
        &db_pool,
        "wechat",
        "default",  // account_scope comes from config.wechat.account_id (default: "default")
        "user_456",
    )
    .expect("DB query failed")
    .expect("Preference should be saved");
    assert_eq!(saved_pref, "claude_code", "Saved preference should be claude_code");

    // Test 2: User sends "openclaw" command with /
    let ctx2 = PipelineContext {
        message: Message {
            id: "msg_2".into(),
            route_key: rk.clone(),
            sequence: None,
            timestamp_ms: 2000,
            direction: Direction::Inbound,
            content: MessageContent::Text("/openclaw".into()),
            audit_mark: None,
        },
        conversation: Conversation::new(rk.clone(), 100),
        config: config.clone(),
        ai_response: None,
        short_circuit: false,
        user_agent_selection: None,
    };

    let result2 = middleware.process(ctx2).await.expect("Middleware failed");
    assert!(result2.short_circuit, "Command should short-circuit");
    assert!(result2.ai_response.is_some(), "Should have response");

    // Verify: Preference was updated
    let saved_pref2 = agent_preferences::get_user_agent(
        &db_pool,
        "wechat",
        "default",  // account_scope comes from config.wechat.account_id
        "user_456",
    )
    .expect("DB query failed")
    .expect("Preference should exist");
    assert_eq!(saved_pref2, "openclaw", "Preference should be updated to openclaw");

    // Test 3: User sends regular message (should use saved preference)
    let ctx3 = PipelineContext {
        message: Message {
            id: "msg_3".into(),
            route_key: rk.clone(),
            sequence: None,
            timestamp_ms: 3000,
            direction: Direction::Inbound,
            content: MessageContent::Text("帮我总结一下".into()),
            audit_mark: None,
        },
        conversation: Conversation::new(rk.clone(), 100),
        config: config.clone(),
        ai_response: None,
        short_circuit: false,
        user_agent_selection: None,
    };

    let result3 = middleware.process(ctx3).await.expect("Middleware failed");
    
    // Should NOT short-circuit (regular message)
    assert!(!result3.short_circuit, "Regular message should not short-circuit");
    
    // Should populate user_agent_selection from saved preference
    assert!(result3.user_agent_selection.is_some(), "Should populate user_agent_selection");
    assert_eq!(
        result3.user_agent_selection.as_ref().unwrap(),
        "openclaw",
        "Should use saved preference"
    );

    // Test 4: User sends "当前 agent" query command
    let ctx4 = PipelineContext {
        message: Message {
            id: "msg_4".into(),
            route_key: rk.clone(),
            sequence: None,
            timestamp_ms: 4000,
            direction: Direction::Inbound,
            content: MessageContent::Text("当前 agent".into()),
            audit_mark: None,
        },
        conversation: Conversation::new(rk.clone(), 100),
        config: config.clone(),
        ai_response: None,
        short_circuit: false,
        user_agent_selection: None,
    };

    let result4 = middleware.process(ctx4).await.expect("Middleware failed");
    assert!(result4.short_circuit, "Query should short-circuit");
    assert!(result4.ai_response.is_some(), "Should have response");
    let query_response = result4.ai_response.as_ref().unwrap();
    assert!(query_response.contains("openclaw"), "Response should mention current agent");

    // Test 5: Different user with no preference
    let rk2 = RouteKey::new(
        ChannelId::new("wechat"),
        "conv_123",
        "user_789",
        ConversationType::Direct,
    );
    let ctx5 = PipelineContext {
        message: Message {
            id: "msg_5".into(),
            route_key: rk2.clone(),
            sequence: None,
            timestamp_ms: 5000,
            direction: Direction::Inbound,
            content: MessageContent::Text("这是一条消息".into()),
            audit_mark: None,
        },
        conversation: Conversation::new(rk2.clone(), 100),
        config: config.clone(),
        ai_response: None,
        short_circuit: false,
        user_agent_selection: None,
    };

    let result5 = middleware.process(ctx5).await.expect("Middleware failed");
    
    // Should short-circuit with a prompt (no preference)
    assert!(result5.short_circuit, "Should short-circuit when no preference");
    assert!(result5.ai_response.is_some(), "Should have prompt response");
    let prompt = result5.ai_response.as_ref().unwrap();
    assert!(prompt.contains("agent") && prompt.contains("选择"), "Should prompt user to select agent");

    // Test 6: Agent command parser recognizes aliases
    let aliases = default_agent_aliases();
    let parser = AgentCommandParser::new(aliases);
    
    assert_eq!(
        parser.resolve_alias("cc").unwrap(),
        "claude_code",
        "Should resolve cc to claude_code"
    );
    assert_eq!(
        parser.resolve_alias("claude code").unwrap(),
        "claude_code",
        "Should resolve 'claude code' to claude_code"
    );
    assert_eq!(
        parser.resolve_alias("cx").unwrap(),
        "codex",
        "Should resolve cx to codex"
    );
    assert_eq!(
        parser.resolve_alias("h").unwrap(),
        "hermes",
        "Should resolve h to hermes"
    );
}

#[tokio::test]
async fn test_user_agent_preference_isolation_by_account() {
    // Verify that preferences are isolated by account_scope
    let conn = db::init_db(":memory:")
        .expect("Failed to init DB");
    let db_pool = DbPool::new(conn);

    let mut config = AppConfig::default();
    config.agent.enable_user_preferences = true;
    let middleware = AgentCommandMiddleware::new(db_pool.clone(), config.agent.clone());

    // User 1 on WeChat account "account1"
    let ctx1 = PipelineContext {
        message: Message {
            id: "msg_1".into(),
            route_key: RouteKey::new(
                ChannelId::new("wechat"),
                "conv_1",
                "user_123",
                ConversationType::Direct,
            ),
            sequence: None,
            timestamp_ms: 1000,
            direction: Direction::Inbound,
            content: MessageContent::Text("cc".into()),
            audit_mark: None,
        },
        conversation: Conversation::new(
            RouteKey::new(
                ChannelId::new("wechat"),
                "conv_1",
                "user_123",
                ConversationType::Direct,
            ),
            100,
        ),
        config: config.clone(),
        ai_response: None,
        short_circuit: false,
        user_agent_selection: None,
    };

    middleware.process(ctx1).await.expect("Middleware failed");

    // Same user on different channel (Dingtalk)
    let ctx2 = PipelineContext {
        message: Message {
            id: "msg_2".into(),
            route_key: RouteKey::new(
                ChannelId::new("dingtalk"),
                "conv_1",
                "user_123",
                ConversationType::Direct,
            ),
            sequence: None,
            timestamp_ms: 2000,
            direction: Direction::Inbound,
            content: MessageContent::Text("cx".into()),
            audit_mark: None,
        },
        conversation: Conversation::new(
            RouteKey::new(
                ChannelId::new("dingtalk"),
                "conv_1",
                "user_123",
                ConversationType::Direct,
            ),
            100,
        ),
        config: config.clone(),
        ai_response: None,
        short_circuit: false,
        user_agent_selection: None,
    };

    middleware.process(ctx2).await.expect("Middleware failed");

    // Verify: Preferences are isolated by channel
    let pref_wechat = agent_preferences::get_user_agent(
        &db_pool,
        "wechat",
        "default",  // account_scope comes from config.wechat.account_id
        "user_123",
    )
    .expect("DB query failed");
    
    let pref_dingtalk = agent_preferences::get_user_agent(
        &db_pool,
        "dingtalk",
        "default",  // account_scope comes from config.wechat.account_id
        "user_123",
    )
    .expect("DB query failed");

    assert_eq!(pref_wechat.as_deref(), Some("claude_code"), "WeChat should have claude_code");
    assert_eq!(pref_dingtalk.as_deref(), Some("codex"), "DingTalk should have codex");
}

#[test]
fn test_agent_command_parser_longest_match() {
    let mut aliases = HashMap::new();
    aliases.insert("claude_code".into(), vec![
        "cc".into(),
        "claude".into(),
        "claude code".into(),
    ]);
    aliases.insert("codex".into(), vec!["cx".into(), "codex".into()]);
    
    let parser = AgentCommandParser::new(aliases);
    
    // "claude code" should match the longest alias before "claude"
    assert_eq!(parser.resolve_alias("claude code").unwrap(), "claude_code");
    
    // Single word should also match
    assert_eq!(parser.resolve_alias("claude").unwrap(), "claude_code");
    
    // Short alias should match
    assert_eq!(parser.resolve_alias("cc").unwrap(), "claude_code");
}