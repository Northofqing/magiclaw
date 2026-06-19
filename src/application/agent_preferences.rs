/// User agent preference management.
use crate::infrastructure::db::DbPool;
use rusqlite::OptionalExtension;

#[derive(Debug, Clone)]
pub struct UserAgentPreferences {
    pub channel: String,
    pub account_scope: String,
    pub peer_id: String,
    pub agent_name: String,
}

/// Get the current agent preference for a user, or None if not set.
pub fn get_user_agent(
    db: &DbPool,
    channel: &str,
    account_scope: &str,
    peer_id: &str,
) -> Result<Option<String>, String> {
    db.query(|conn| {
        let mut stmt = conn.prepare(
            "SELECT agent_name FROM user_agent_preferences 
             WHERE channel = ?1 AND account_scope = ?2 AND peer_id = ?3",
        )?;
        let result = stmt
            .query_row([channel, account_scope, peer_id], |row| {
                row.get::<_, String>(0)
            })
            .optional()?;
        Ok(result)
    })
}

/// Set the agent preference for a user.
pub fn set_user_agent(
    db: &DbPool,
    channel: &str,
    account_scope: &str,
    peer_id: &str,
    agent_name: &str,
) -> Result<(), String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();

    db.execute(|conn| {
        conn.execute(
            "INSERT OR REPLACE INTO user_agent_preferences 
             (channel, account_scope, peer_id, agent_name, updated_at) 
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![channel, account_scope, peer_id, agent_name, now],
        )?;
        Ok(())
    })
}

/// Get all agent preferences (for admin/debugging).
pub fn list_user_agents(
    db: &DbPool,
    channel: Option<&str>,
) -> Result<Vec<UserAgentPreferences>, String> {
    db.query(|conn| {
        let query = if channel.is_some() {
            "SELECT channel, account_scope, peer_id, agent_name FROM user_agent_preferences WHERE channel = ?1 ORDER BY updated_at DESC"
        } else {
            "SELECT channel, account_scope, peer_id, agent_name FROM user_agent_preferences ORDER BY updated_at DESC"
        };

        let mut stmt = conn.prepare(query)?;
        let rows: Vec<UserAgentPreferences> = if let Some(ch) = channel {
            stmt.query_map([ch], |row| {
                Ok(UserAgentPreferences {
                    channel: row.get(0)?,
                    account_scope: row.get(1)?,
                    peer_id: row.get(2)?,
                    agent_name: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map([], |row| {
                Ok(UserAgentPreferences {
                    channel: row.get(0)?,
                    account_scope: row.get(1)?,
                    peer_id: row.get(2)?,
                    agent_name: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?
        };

        Ok(rows)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_and_get_user_agent() {
        let conn = crate::infrastructure::db::init_db(":memory:").unwrap();
        let db = DbPool::new(conn);

        // Initially no preference
        let agent = get_user_agent(&db, "wechat", "account1", "user1").unwrap();
        assert!(agent.is_none());

        // Set preference
        set_user_agent(&db, "wechat", "account1", "user1", "claude_code").unwrap();

        // Get preference
        let agent = get_user_agent(&db, "wechat", "account1", "user1").unwrap();
        assert_eq!(agent, Some("claude_code".to_string()));

        // Override preference
        set_user_agent(&db, "wechat", "account1", "user1", "codex").unwrap();
        let agent = get_user_agent(&db, "wechat", "account1", "user1").unwrap();
        assert_eq!(agent, Some("codex".to_string()));

        // Different user, different preference
        set_user_agent(&db, "wechat", "account1", "user2", "hermes").unwrap();
        let agent = get_user_agent(&db, "wechat", "account1", "user2").unwrap();
        assert_eq!(agent, Some("hermes".to_string()));

        // Original user still has old preference
        let agent = get_user_agent(&db, "wechat", "account1", "user1").unwrap();
        assert_eq!(agent, Some("codex".to_string()));
    }

    #[test]
    fn test_user_agent_isolation_by_account() {
        let conn = crate::infrastructure::db::init_db(":memory:").unwrap();
        let db = DbPool::new(conn);

        // Same user, different account
        set_user_agent(&db, "wechat", "account1", "user1", "claude_code").unwrap();
        set_user_agent(&db, "wechat", "account2", "user1", "codex").unwrap();

        let agent1 = get_user_agent(&db, "wechat", "account1", "user1").unwrap();
        let agent2 = get_user_agent(&db, "wechat", "account2", "user1").unwrap();

        assert_eq!(agent1, Some("claude_code".to_string()));
        assert_eq!(agent2, Some("codex".to_string()));
    }

    #[test]
    fn test_list_user_agents() {
        let conn = crate::infrastructure::db::init_db(":memory:").unwrap();
        let db = DbPool::new(conn);

        set_user_agent(&db, "wechat", "account1", "user1", "claude_code").unwrap();
        set_user_agent(&db, "wechat", "account1", "user2", "codex").unwrap();
        set_user_agent(&db, "dingtalk", "account1", "user1", "hermes").unwrap();

        // List all
        let all = list_user_agents(&db, None).unwrap();
        assert_eq!(all.len(), 3);

        // List by channel
        let wechat = list_user_agents(&db, Some("wechat")).unwrap();
        assert_eq!(wechat.len(), 2);
        let dingtalk = list_user_agents(&db, Some("dingtalk")).unwrap();
        assert_eq!(dingtalk.len(), 1);
    }
}
