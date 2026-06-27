//! SQLite implementation of `UserPreferenceStore`.

use crate::application::agent_preferences::UserAgentPreferences;
use crate::domain::ports::user_preference_store::UserPreferenceStore;
use crate::infrastructure::db::DbPool;

/// SQLite-backed user preference store.
pub struct SqliteUserPreferenceStore {
    pool: DbPool,
}

impl SqliteUserPreferenceStore {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }
}

impl UserPreferenceStore for SqliteUserPreferenceStore {
    fn get(
        &self,
        channel: &str,
        account_scope: &str,
        peer_id: &str,
    ) -> Result<Option<String>, String> {
        self.pool
            .query(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT agent_name FROM user_agent_preferences \
                     WHERE channel = ?1 AND account_scope = ?2 AND peer_id = ?3",
                )?;
                let result = stmt
                    .query_row([channel, account_scope, peer_id], |row| {
                        row.get::<_, String>(0)
                    })
                    .ok();
                Ok(result)
            })
    }

    fn set(
        &self,
        channel: &str,
        account_scope: &str,
        peer_id: &str,
        agent_name: &str,
    ) -> Result<(), String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_secs();

        self.pool.execute(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO user_agent_preferences \
                 (channel, account_scope, peer_id, agent_name, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![channel, account_scope, peer_id, agent_name, now],
            )?;
            Ok(())
        })
    }

    fn list(&self, channel: Option<&str>) -> Result<Vec<UserAgentPreferences>, String> {
        self.pool.query(|conn| {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::db::init_db;

    #[test]
    fn roundtrip_set_get_list() {
        let pool = DbPool::new(init_db(":memory:").unwrap());
        let store = SqliteUserPreferenceStore::new(pool);

        assert!(store.get("wechat", "a", "u1").unwrap().is_none());
        store.set("wechat", "a", "u1", "claude_code").unwrap();
        assert_eq!(
            store.get("wechat", "a", "u1").unwrap(),
            Some("claude_code".into())
        );

        store.set("wechat", "a", "u2", "codex").unwrap();
        store.set("dingtalk", "a", "u1", "hermes").unwrap();

        let wechat = store.list(Some("wechat")).unwrap();
        assert_eq!(wechat.len(), 2);
        let dingtalk = store.list(Some("dingtalk")).unwrap();
        assert_eq!(dingtalk.len(), 1);
        let all = store.list(None).unwrap();
        assert_eq!(all.len(), 3);
    }
}