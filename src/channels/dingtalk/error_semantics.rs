//! Dingtalk error semantics — maps HTTP status codes and Dingtalk errcodes
//! to retryable vs terminal classifications for the outbox worker.
//!
//! Reference: https://open.dingtalk.com/document/orgapp-server/error-codes

/// Classification of Dingtalk API errors for outbox state transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DingtalkErrorSemantics {
    Retryable,
    Terminal,
    Unknown,
}

impl DingtalkErrorSemantics {
    /// Classify HTTP status code.
    pub fn from_http_status(status_code: u16) -> Self {
        match status_code {
            400 => Self::Terminal,
            401 | 403 => Self::Terminal,
            404 => Self::Terminal,
            405 => Self::Terminal,
            408 | 429 => Self::Retryable,
            500..=599 => Self::Retryable,
            _ => Self::Unknown,
        }
    }

    /// Classify based on Dingtalk error code.
    pub fn from_dingtalk_errcode(code: i64) -> Self {
        match code {
            0 => Self::Terminal, // not an error
            // System busy / rate limit
            33001 | 90001 | 90002 | 90007 => Self::Retryable,
            // Auth / token expired
            40001 | 40003 | 40014 | 40102 => Self::Terminal,
            // Invalid param / not found
            40002 | 40004 | 60008 => Self::Terminal,
            // Server error
            -1 => Self::Retryable,
            _ => Self::Unknown,
        }
    }

    pub fn should_retry(&self) -> bool {
        matches!(self, Self::Retryable | Self::Unknown)
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Terminal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_429_is_retryable() {
        assert!(DingtalkErrorSemantics::from_http_status(429).should_retry());
    }

    #[test]
    fn http_500_is_retryable() {
        assert!(DingtalkErrorSemantics::from_http_status(500).should_retry());
    }

    #[test]
    fn http_403_is_terminal() {
        assert!(DingtalkErrorSemantics::from_http_status(403).is_terminal());
    }

    #[test]
    fn errcode_40001_is_terminal() {
        assert!(DingtalkErrorSemantics::from_dingtalk_errcode(40001).is_terminal());
    }

    #[test]
    fn errcode_33001_is_retryable() {
        assert!(DingtalkErrorSemantics::from_dingtalk_errcode(33001).should_retry());
    }

    #[test]
    fn unknown_defaults_to_retryable() {
        assert!(DingtalkErrorSemantics::Unknown.should_retry());
        assert!(!DingtalkErrorSemantics::Unknown.is_terminal());
    }
}