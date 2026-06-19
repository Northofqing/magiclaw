/// Audit sink port. Records key data-flow and send decisions for traceability.
///
/// Red line 2.6: 关键数据流与发送决策留痕(来源、时间、RouteKey、决策依据、执行结果)。
/// Implementations must be cheap to call from hot paths; a failure to record
/// must never abort the business operation (audit writes are best-effort and
/// logged on failure).
pub trait AuditSink: Send + Sync {
    /// Record an audit entry.
    ///
    /// - `route_key`: serialized RouteKey or namespaced identifier, if applicable.
    /// - `action`: the decision/operation name (e.g. `send`, `dead_letter`).
    /// - `result`: the execution result (e.g. `sent`, `retrying`, error detail).
    fn record(&self, route_key: Option<&str>, action: &str, result: &str);
}

/// No-op audit sink for tests and contexts where auditing is disabled.
pub struct NoopAuditSink;

impl AuditSink for NoopAuditSink {
    fn record(&self, _route_key: Option<&str>, _action: &str, _result: &str) {}
}
