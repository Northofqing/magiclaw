use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboxEntry {
    pub id: String,
    pub route_key: String,
    pub payload: String,
    pub status: OutboxStatus,
    pub retry_count: u32,
    pub next_retry_at: Option<i64>,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl OutboxEntry {
    pub fn new(
        id: impl Into<String>,
        route_key: impl Into<String>,
        payload: impl Into<String>,
        now_ts: i64,
    ) -> Self {
        Self {
            id: id.into(),
            route_key: route_key.into(),
            payload: payload.into(),
            status: OutboxStatus::Pending,
            retry_count: 0,
            next_retry_at: None,
            last_error: None,
            created_at: now_ts,
            updated_at: now_ts,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutboxStatus {
    Pending,
    Sending,
    Sent,
    Retrying,
    DeadLetter,
}

impl OutboxStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Sending => "sending",
            Self::Sent => "sent",
            Self::Retrying => "retrying",
            Self::DeadLetter => "dead_letter",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "sending" => Some(Self::Sending),
            "sent" => Some(Self::Sent),
            "retrying" => Some(Self::Retrying),
            "dead_letter" => Some(Self::DeadLetter),
            _ => None,
        }
    }
}

/// Retry configuration with exponential backoff and jitter.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    pub max_retries: u32,
    pub base_backoff_ms: u64,
    pub max_backoff_ms: u64,
    pub jitter: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 5,
            base_backoff_ms: 1000,
            max_backoff_ms: 60000,
            jitter: 0.1,
        }
    }
}

impl RetryConfig {
    /// Calculate the next retry delay: min(base * 2^attempt + jitter, max_backoff).
    pub fn next_delay_ms(&self, retry_count: u32) -> u64 {
        let base = self.base_backoff_ms.saturating_mul(2u64.saturating_pow(retry_count));
        let capped = base.min(self.max_backoff_ms);
        let jitter_range = (capped as f64 * self.jitter) as u64;
        let with_jitter = if jitter_range > 0 {
            capped.saturating_add(retry_count as u64 * 7 % jitter_range)
        } else {
            capped
        };
        with_jitter.min(self.max_backoff_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_delay_grows_exponentially() {
        let config = RetryConfig::default();
        let d1 = config.next_delay_ms(0);
        let d2 = config.next_delay_ms(1);
        let d3 = config.next_delay_ms(2);
        assert!(d2 > d1);
        assert!(d3 > d2);
    }

    #[test]
    fn retry_delay_capped_at_max() {
        let config = RetryConfig {
            max_backoff_ms: 5000,
            ..Default::default()
        };
        let d = config.next_delay_ms(10);
        assert!(d <= 5000);
    }

    #[test]
    fn outbox_entry_new_is_pending() {
        let entry = OutboxEntry::new("m1", "wechat/conv1", "hello", 1000);
        assert_eq!(entry.status, OutboxStatus::Pending);
        assert_eq!(entry.retry_count, 0);
    }
}
