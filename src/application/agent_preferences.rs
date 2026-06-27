/// User agent preference management.
use crate::domain::ports::user_preference_store::UserPreferenceStore;
use crate::infrastructure::db::DbPool;
use rusqlite::OptionalExtension;

#[derive(Debug, Clone)]
pub struct UserAgentPreferences {
    pub channel: String,
    pub account_scope: String,
    pub peer_id: String,
    pub agent_name: String,
}

// Port-based entry points (preferred): take a `&dyn UserPreferenceStore`
// instead of a raw `DbPool`. The application layer no longer needs to know
// about the storage technology.

pub fn get_user_agent_via_port(
    store: &dyn UserPreferenceStore,
    channel: &str,
    account_scope: &str,
    peer_id: &str,
) -> Result<Option<String>, String> {
    store.get(channel, account_scope, peer_id)
}

pub fn set_user_agent_via_port(
    store: &dyn UserPreferenceStore,
    channel: &str,
    account_scope: &str,
    peer_id: &str,
    agent_name: &str,
) -> Result<(), String> {
    store.set(channel, account_scope, peer_id, agent_name)
}

pub fn list_user_agents_via_port(
    store: &dyn UserPreferenceStore,
    channel: Option<&str>,
) -> Result<Vec<UserAgentPreferences>, String> {
    store.list(channel)
}

// Legacy DbPool-based entry points: kept for callers not yet wired up to the
// port. New code should prefer the port-based functions.

#[deprecated(note = "use get_user_agent_via_port with a UserPreferenceStore port")]
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

#[deprecated(note = "use set_user_agent_via_port with a UserPreferenceStore port")]
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

#[deprecated(note = "use list_user_agents_via_port with a UserPreferenceStore port")]
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
    use crate::adapters::sqlite_user_preference_store::SqliteUserPreferenceStore;
    use crate::domain::ports::user_preference_store::InMemoryPreferenceStore;
    use crate::infrastructure::db::init_db;

    fn make_pool() -> DbPool {
        DbPool::new(init_db(":memory:").unwrap())
    }

    #[test]
    fn test_set_and_get_user_agent_via_port() {
        let store = SqliteUserPreferenceStore::new(make_pool());

        assert!(get_user_agent_via_port(&store, "wechat", "account1", "user1")
            .unwrap()
            .is_none());

        set_user_agent_via_port(&store, "wechat", "account1", "user1", "claude_code").unwrap();
        assert_eq!(
            get_user_agent_via_port(&store, "wechat", "account1", "user1").unwrap(),
            Some("claude_code".into())
        );

        // Override
        set_user_agent_via_port(&store, "wechat", "account1", "user1", "codex").unwrap();
        assert_eq!(
            get_user_agent_via_port(&store, "wechat", "account1", "user1").unwrap(),
            Some("codex".into())
        );

        // Per-user isolation
        set_user_agent_via_port(&store, "wechat", "account1", "user2", "hermes").unwrap();
        assert_eq!(
            get_user_agent_via_port(&store, "wechat", "account1", "user2").unwrap(),
            Some("hermes".into())
        );
        // user1 unchanged
        assert_eq!(
            get_user_agent_via_port(&store, "wechat", "account1", "user1").unwrap(),
            Some("codex".into())
        );
    }

    #[test]
    fn test_user_agent_isolation_by_account_via_port() {
        let store = SqliteUserPreferenceStore::new(make_pool());

        set_user_agent_via_port(&store, "wechat", "account1", "user1", "claude_code").unwrap();
        set_user_agent_via_port(&store, "wechat", "account2", "user1", "codex").unwrap();

        assert_eq!(
            get_user_agent_via_port(&store, "wechat", "account1", "user1").unwrap(),
            Some("claude_code".into())
        );
        assert_eq!(
            get_user_agent_via_port(&store, "wechat", "account2", "user1").unwrap(),
            Some("codex".into())
        );
    }

    #[test]
    fn test_list_user_agents_via_port() {
        let store = SqliteUserPreferenceStore::new(make_pool());

        set_user_agent_via_port(&store, "wechat", "account1", "user1", "claude_code").unwrap();
        set_user_agent_via_port(&store, "wechat", "account1", "user2", "codex").unwrap();
        set_user_agent_via_port(&store, "dingtalk", "account1", "user1", "hermes").unwrap();

        assert_eq!(list_user_agents_via_port(&store, None).unwrap().len(), 3);
        assert_eq!(list_user_agents_via_port(&store, Some("wechat")).unwrap().len(), 2);
        assert_eq!(list_user_agents_via_port(&store, Some("dingtalk")).unwrap().len(), 1);
    }

    #[test]
    fn test_in_memory_store_roundtrip() {
        let store = InMemoryPreferenceStore::new();
        set_user_agent_via_port(&store, "wechat", "a", "u1", "claude_code").unwrap();
        assert_eq!(
            get_user_agent_via_port(&store, "wechat", "a", "u1").unwrap(),
            Some("claude_code".into())
        );
    }
}
