use crate::domain::ports::conversation_state_repo::{
    ConversationStateRepo, PersistedConversationState,
};
use crate::domain::ports::inbox_repo::{RepoError, RepoResult};
use crate::infrastructure::db::DbPool;

/// SQLite-backed conversation state repository (red line 2.3).
///
/// Writes to the `conversation_state` table with `route_key` as primary key,
/// so each conversation has exactly one durable state row that is upserted on
/// lifecycle transitions.
pub struct SqliteConversationStateRepo {
    db: DbPool,
}

impl SqliteConversationStateRepo {
    pub fn new(db: DbPool) -> Self {
        Self { db }
    }
}

impl ConversationStateRepo for SqliteConversationStateRepo {
    fn upsert(&self, route_key: &str, state_json: &str, updated_at: i64) -> RepoResult<()> {
        let route_key = route_key.to_string();
        let state_json = state_json.to_string();
        self.db
            .execute(move |conn| {
                conn.execute(
                    "INSERT INTO conversation_state (route_key, state_json, updated_at)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT(route_key) DO UPDATE SET
                         state_json = excluded.state_json,
                         updated_at = excluded.updated_at",
                    rusqlite::params![route_key, state_json, updated_at],
                )?;
                Ok(())
            })
            .map_err(RepoError::Db)
    }

    fn load_all(&self) -> RepoResult<Vec<PersistedConversationState>> {
        let db = self.db.clone();
        db.query(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT route_key, state_json, updated_at FROM conversation_state ORDER BY updated_at DESC",
            )?;
            let entries: Result<Vec<_>, _> = stmt
                .query_map([], |row| {
                    Ok(PersistedConversationState {
                        route_key: row.get(0)?,
                        state_json: row.get(1)?,
                        updated_at: row.get(2)?,
                    })
                })?
                .collect();
            entries
        })
        .map_err(RepoError::Db)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::db::init_db;

    fn make_pool() -> DbPool {
        DbPool::new(init_db(":memory:").unwrap())
    }

    #[test]
    fn upsert_then_load_all() {
        let repo = SqliteConversationStateRepo::new(make_pool());
        repo.upsert("rk-1", "\"active\"", 1000).unwrap();
        let all = repo.load_all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].route_key, "rk-1");
        assert_eq!(all[0].state_json, "\"active\"");
    }

    #[test]
    fn upsert_updates_existing_row() {
        let repo = SqliteConversationStateRepo::new(make_pool());
        repo.upsert("rk-1", "\"active\"", 1000).unwrap();
        repo.upsert("rk-1", "\"closed\"", 2000).unwrap();
        let all = repo.load_all().unwrap();
        assert_eq!(all.len(), 1, "route_key is primary key, no duplicate row");
        assert_eq!(all[0].state_json, "\"closed\"");
        assert_eq!(all[0].updated_at, 2000);
    }
}
