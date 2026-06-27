//! Audit query port — read-side companion to `AuditSink`.
//!
//! `AuditSink` covers the hot-path write side (one-line per decision).
//! `AuditQuery` covers the tooling/inspection side (filter by route, paginate).
//! Splitting them lets the hot path inject a cheap `NoopAuditSink` while the
//! tooling still receives a richer queryable store via DI.
//!
//! Red line: audit logs must remain queryable for ≥ 5 years. This trait
//! defines the read API; implementations decide storage layout and retention.

use crate::application::audit::AuditRecord;

/// Read-side audit log query port.
///
/// `query_by_route` returns the most recent `limit` records matching the
/// (serialized) route_key; `query_all` returns recent records across all routes
/// for operational dashboards.
pub trait AuditQuery: Send + Sync {
    fn query_by_route(
        &self,
        route_key: &str,
        limit: usize,
    ) -> Result<Vec<AuditRecord>, String>;

    fn query_all(&self, limit: usize) -> Result<Vec<AuditRecord>, String>;
}

/// No-op query implementation for contexts where audit reads are disabled
/// (production runtime never queries, only writes).
pub struct NoopAuditQuery;

impl AuditQuery for NoopAuditQuery {
    fn query_by_route(&self, _route_key: &str, _limit: usize) -> Result<Vec<AuditRecord>, String> {
        Ok(Vec::new())
    }

    fn query_all(&self, _limit: usize) -> Result<Vec<AuditRecord>, String> {
        Ok(Vec::new())
    }
}