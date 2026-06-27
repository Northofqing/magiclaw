//! Audit log chain hashing — tamper-evident log integrity.
//!
//! CLAUDE.md red line #5.3: audit logs are immutable, retention ≥ 5 years.
//! Each entry carries a `prev_hash` (the previous entry's hash, NULL for
//! genesis) and `entry_hash` = SHA-256(prev_hash || route_key || action ||
//! result || created_at). A startup verifier walks the chain and refuses to
//! start the daemon if any hash mismatches, surfacing tampering instead of
//! silently shipping corrupted logs to audit consumers.
//!
//! Trade-off: SHA-256 is one-way, so verification recomputes from the row
//! content and checks equality with the stored `entry_hash`. Tampering that
//! recomputes hashes throughout the chain would still defeat this — for true
//! non-repudiation, pair with an externally-anchored timestamp authority
//! (out of scope for v1).

use sha2::{Digest, Sha256};

/// Lowercase hex encoder (no external dep). Output is 64 chars for a SHA-256
/// digest. Pulled into its own function so `compute_entry_hash` stays readable
/// and tests can assert against a stable string format.
fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// Domain error for chain operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainError {
    /// The stored `prev_hash` does not match the expected chain head.
    /// Concurrent writers must serialise.
    HeadMismatch {
        id: i64,
        expected: Option<String>,
        actual: Option<String>,
    },
    /// Stored hash does not match the recomputed hash for the row content.
    /// Indicates tampering or on-disk corruption.
    HashMismatch {
        id: i64,
        expected: String,
        actual: String,
    },
}

/// A row in the audit chain as loaded from storage.
#[derive(Debug, Clone)]
pub struct ChainRow {
    pub id: i64,
    pub prev_hash: Option<String>,
    pub entry_hash: String,
    pub route_key: Option<String>,
    pub action: String,
    pub result: String,
    pub created_at: i64,
}

/// Compute the entry hash for a candidate row.
///
/// `prev_hash` is the hex sha256 of the previous row (or empty string for
/// the genesis row — empty string is never a real hash, so genesis is
/// distinguishable from a tampering reset).
pub fn compute_entry_hash(
    prev_hash: &str,
    route_key: &str,
    action: &str,
    result: &str,
    created_at: i64,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(prev_hash.as_bytes());
    hasher.update(b"|");
    hasher.update(route_key.as_bytes());
    hasher.update(b"|");
    hasher.update(action.as_bytes());
    hasher.update(b"|");
    hasher.update(result.as_bytes());
    hasher.update(b"|");
    hasher.update(created_at.to_be_bytes());
    to_hex(&hasher.finalize())
}

/// Verify a chain in order. Returns the head hash on success, or the first
/// `ChainError` encountered.
pub fn verify<I>(rows: I) -> Result<Option<String>, ChainError>
where
    I: IntoIterator<Item = ChainRow>,
{
    let mut expected_prev: Option<String> = None;
    let mut last_hash: Option<String> = None;
    for row in rows {
        // 1. prev_hash must equal the previous entry's entry_hash (or None for genesis).
        if row.prev_hash != expected_prev {
            return Err(ChainError::HeadMismatch {
                id: row.id,
                expected: expected_prev,
                actual: row.prev_hash,
            });
        }
        // 2. recompute entry_hash from row content and stored prev_hash.
        let prev_for_recompute = row.prev_hash.as_deref().unwrap_or("");
        let rk = row.route_key.as_deref().unwrap_or("");
        let computed = compute_entry_hash(
            prev_for_recompute,
            rk,
            &row.action,
            &row.result,
            row.created_at,
        );
        if computed != row.entry_hash {
            return Err(ChainError::HashMismatch {
                id: row.id,
                expected: computed,
                actual: row.entry_hash,
            });
        }
        // 3. advance head
        expected_prev = Some(computed.clone());
        last_hash = Some(computed);
    }
    Ok(last_hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        id: i64,
        prev: Option<&str>,
        entry: &str,
        rk: Option<&str>,
        action: &str,
        result: &str,
        ts: i64,
    ) -> ChainRow {
        ChainRow {
            id,
            prev_hash: prev.map(str::to_string),
            entry_hash: entry.to_string(),
            route_key: rk.map(str::to_string),
            action: action.to_string(),
            result: result.to_string(),
            created_at: ts,
        }
    }

    #[test]
    fn hash_is_deterministic_for_same_inputs() {
        let a = compute_entry_hash("", "wechat/c1", "send", "sent", 1700000000);
        let b = compute_entry_hash("", "wechat/c1", "send", "sent", 1700000000);
        assert_eq!(a, b);
    }

    #[test]
    fn hash_changes_when_any_field_changes() {
        let base = compute_entry_hash("", "wechat/c1", "send", "sent", 1700000000);
        assert_ne!(
            compute_entry_hash("prev", "wechat/c1", "send", "sent", 1700000000),
            base
        );
        assert_ne!(
            compute_entry_hash("", "wechat/c2", "send", "sent", 1700000000),
            base
        );
        assert_ne!(
            compute_entry_hash("", "wechat/c1", "send", "queued", 1700000000),
            base
        );
        assert_ne!(
            compute_entry_hash("", "wechat/c1", "send", "sent", 1700000001),
            base
        );
    }

    #[test]
    fn verify_accepts_valid_chain() {
        let h0 = compute_entry_hash("", "a", "send", "ok", 100);
        let h1 = compute_entry_hash(&h0, "a", "send", "ok", 200);
        let h2 = compute_entry_hash(&h1, "b", "dead_letter", "max retries", 300);

        let rows = vec![
            row(1, None, &h0, Some("a"), "send", "ok", 100),
            row(2, Some(&h0), &h1, Some("a"), "send", "ok", 200),
            row(3, Some(&h1), &h2, Some("b"), "dead_letter", "max retries", 300),
        ];
        let verified = verify(rows).unwrap();
        assert_eq!(verified, Some(h2));
    }

    #[test]
    fn verify_detects_broken_link() {
        let h0 = compute_entry_hash("", "a", "send", "ok", 100);
        let h1 = compute_entry_hash(&h0, "a", "send", "ok", 200);

        let rows = vec![
            row(1, None, &h0, Some("a"), "send", "ok", 100),
            row(2, Some("deadbeef"), &h1, Some("a"), "send", "ok", 200),
        ];
        let err = verify(rows).unwrap_err();
        assert!(matches!(err, ChainError::HeadMismatch { id: 2, .. }), "got {:?}", err);
    }

    #[test]
    fn verify_detects_tampered_result() {
        let h0 = compute_entry_hash("", "a", "send", "ok", 100);
        let h1 = compute_entry_hash(&h0, "a", "send", "ok", 200);

        let rows = vec![
            row(1, None, &h0, Some("a"), "send", "ok", 100),
            row(2, Some(&h0), &h1, Some("a"), "send", "TAMPERED", 200),
        ];
        let err = verify(rows).unwrap_err();
        assert!(matches!(err, ChainError::HashMismatch { id: 2, .. }), "got {:?}", err);
    }

    #[test]
    fn empty_chain_returns_none() {
        let rows: Vec<ChainRow> = vec![];
        let verified = verify(rows).unwrap();
        assert_eq!(verified, None);
    }
}