use base64::engine::general_purpose::STANDARD_NO_PAD;
use base64::Engine as _;
use rand::rngs::OsRng;
use rand::RngCore;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::infrastructure::db::DbPool;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiClientRecord {
    pub id: String,
    pub project_id: String,
    pub client_name: String,
    pub token_hash: String,
    pub scopes: Vec<String>,
    pub created_at: i64,
    pub expires_at: i64,
    pub revoked_at: Option<i64>,
    pub rotated_from: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IssuedApiClient {
    pub raw_token: String,
    pub record: ApiClientRecord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthFailure {
    Unauthorized,
    Forbidden,
}

#[derive(Clone)]
pub struct ApiClientRegistry {
    db: DbPool,
}

impl ApiClientRegistry {
    pub fn new(db: DbPool) -> Self {
        Self { db }
    }

    fn now_ts() -> i64 {
        chrono::Utc::now().timestamp()
    }

    fn hash_token(raw_token: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(raw_token.as_bytes());
        STANDARD_NO_PAD.encode(hasher.finalize())
    }

    fn generate_token() -> String {
        let mut bytes = [0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        STANDARD_NO_PAD.encode(bytes)
    }

    fn parse_scopes(scopes_json: &str) -> Result<Vec<String>, String> {
        let scopes: Vec<String> = serde_json::from_str(scopes_json)
            .map_err(|e| format!("invalid api client scopes JSON: {}", e))?;
        Ok(scopes)
    }

    fn scopes_to_json(scopes: &[String]) -> Result<String, String> {
        serde_json::to_string(scopes).map_err(|e| format!("failed to serialize scopes: {}", e))
    }

    fn record_from_row(row: &rusqlite::Row<'_>) -> Result<ApiClientRecord, rusqlite::Error> {
        let scopes_json: String = row.get("scopes")?;
        let scopes = Self::parse_scopes(&scopes_json).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
            )
        })?;
        Ok(ApiClientRecord {
            id: row.get("id")?,
            project_id: row.get("project_id")?,
            client_name: row.get("client_name")?,
            token_hash: row.get("token_hash")?,
            scopes,
            created_at: row.get("created_at")?,
            expires_at: row.get("expires_at")?,
            revoked_at: row.get("revoked_at")?,
            rotated_from: row.get("rotated_from")?,
        })
    }

    pub fn issue_token(
        &self,
        project_id: &str,
        client_name: &str,
        scopes: &[String],
        ttl_secs: i64,
        rotated_from: Option<&str>,
    ) -> Result<IssuedApiClient, String> {
        let project_id = project_id.trim();
        if project_id.is_empty() {
            return Err("project_id cannot be empty".into());
        }
        let client_name = client_name.trim();
        if client_name.is_empty() {
            return Err("client_name cannot be empty".into());
        }
        if ttl_secs <= 0 {
            return Err("ttl_secs must be positive".into());
        }

        let raw_token = Self::generate_token();
        let token_hash = Self::hash_token(&raw_token);
        let record = ApiClientRecord {
            id: uuid::Uuid::new_v4().to_string(),
            project_id: project_id.to_string(),
            client_name: client_name.to_string(),
            token_hash: token_hash.clone(),
            scopes: scopes.to_vec(),
            created_at: Self::now_ts(),
            expires_at: Self::now_ts() + ttl_secs,
            revoked_at: None,
            rotated_from: rotated_from.map(|s| s.to_string()),
        };
        let scopes_json = Self::scopes_to_json(&record.scopes)?;

        self.db.execute(|conn| {
            conn.execute(
                "INSERT INTO api_clients (id, project_id, client_name, token_hash, scopes, created_at, expires_at, revoked_at, rotated_from)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    record.id,
                    record.project_id,
                    record.client_name,
                    record.token_hash,
                    scopes_json,
                    record.created_at,
                    record.expires_at,
                    record.revoked_at,
                    record.rotated_from,
                ],
            )?;
            Ok(())
        })?;

        Ok(IssuedApiClient { raw_token, record })
    }

    pub fn list_tokens(&self, project_id: Option<&str>) -> Result<Vec<ApiClientRecord>, String> {
        self.db.query(|conn| {
            let sql = if project_id.is_some() {
                "SELECT id, project_id, client_name, token_hash, scopes, created_at, expires_at, revoked_at, rotated_from
                 FROM api_clients WHERE project_id = ?1 ORDER BY created_at DESC"
            } else {
                "SELECT id, project_id, client_name, token_hash, scopes, created_at, expires_at, revoked_at, rotated_from
                 FROM api_clients ORDER BY created_at DESC"
            };
            let mut stmt = conn.prepare(sql)?;
            let mut rows = if let Some(project_id) = project_id {
                stmt.query(params![project_id])?
            } else {
                stmt.query([])?
            };
            let mut items = Vec::new();
            while let Some(row) = rows.next()? {
                items.push(Self::record_from_row(row)?);
            }
            Ok(items)
        })
    }

    pub fn revoke_token(&self, raw_token: &str) -> Result<bool, String> {
        let token_hash = Self::hash_token(raw_token.trim());
        let now = Self::now_ts();
        self.db.query(|conn| {
            let updated = conn.execute(
                "UPDATE api_clients SET revoked_at = ?1 WHERE token_hash = ?2",
                params![now, token_hash],
            )?;
            Ok(updated > 0)
        })
    }

    fn load_by_token_hash(&self, token_hash: &str) -> Result<Option<ApiClientRecord>, String> {
        self.db.query(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, project_id, client_name, token_hash, scopes, created_at, expires_at, revoked_at, rotated_from
                 FROM api_clients WHERE token_hash = ?1 LIMIT 1",
            )?;
            let mut rows = stmt.query(params![token_hash])?;
            if let Some(row) = rows.next()? {
                Ok(Some(Self::record_from_row(row)?))
            } else {
                Ok(None)
            }
        })
    }

    fn matches_scope(record: &ApiClientRecord, required_scope: &str) -> bool {
        record
            .scopes
            .iter()
            .any(|scope| scope == required_scope || scope == "*")
    }

    pub fn authorize_bearer(
        &self,
        auth_header: Option<&str>,
        required_scope: &str,
    ) -> Result<ApiClientRecord, AuthFailure> {
        let Some(header) = auth_header else {
            return Err(AuthFailure::Unauthorized);
        };
        let Some(raw_token) = header.strip_prefix("Bearer ") else {
            return Err(AuthFailure::Unauthorized);
        };
        let raw_token = raw_token.trim();
        if raw_token.is_empty() {
            return Err(AuthFailure::Unauthorized);
        }

        let token_hash = Self::hash_token(raw_token);
        let Some(record) = self.load_by_token_hash(&token_hash).map_err(|_| AuthFailure::Unauthorized)? else {
            return Err(AuthFailure::Unauthorized);
        };

        let now = Self::now_ts();
        if record.revoked_at.is_some() || record.expires_at <= now {
            return Err(AuthFailure::Unauthorized);
        }

        if !Self::matches_scope(&record, required_scope) {
            return Err(AuthFailure::Forbidden);
        }

        Ok(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> ApiClientRegistry {
        ApiClientRegistry::new(DbPool::new(crate::infrastructure::db::init_db(":memory:").unwrap()))
    }

    #[test]
    fn issue_list_and_revoke_flow_works() {
        let registry = registry();
        let issued = registry
            .issue_token("proj-a", "worker-a", &["send".into(), "window_status".into()], 3600, None)
            .unwrap();

        assert!(!issued.raw_token.trim().is_empty());
        assert_eq!(issued.record.project_id, "proj-a");
        assert_eq!(issued.record.scopes.len(), 2);

        let listed = registry.list_tokens(Some("proj-a")).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].client_name, "worker-a");

        assert!(registry.revoke_token(&issued.raw_token).unwrap());
        assert!(matches!(
            registry.authorize_bearer(Some(&format!("Bearer {}", issued.raw_token)), "send"),
            Err(AuthFailure::Unauthorized)
        ));
    }

    #[test]
    fn scope_enforcement_distinguishes_forbidden_from_unauthorized() {
        let registry = registry();
        let issued = registry
            .issue_token("proj-b", "worker-b", &["send".into()], 3600, None)
            .unwrap();

        assert!(matches!(
            registry.authorize_bearer(Some(&format!("Bearer {}", issued.raw_token)), "window_status"),
            Err(AuthFailure::Forbidden)
        ));
    }
}