//! Opt-in Feishu text result contract. It reports this invocation only and
//! contains no credentials, target text, message body, or provider error text.

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::channels::feishu::channel::FeishuTextDeliveryEvidence;
use crate::infrastructure::config::FeishuConfig;

pub const FEISHU_DELIVERY_SCHEMA_V1: &str = "magiclaw.feishu_delivery.v1";

#[derive(Serialize)]
pub struct FeishuDeliveryResultV1 {
    schema: &'static str,
    invocation_id: String,
    channel: &'static str,
    account_id: String,
    app_id_sha256: String,
    receive_id_type: String,
    target_sha256: String,
    content_sha256: String,
    kind: &'static str,
    phase: &'static str,
    message_request_started: bool,
    reason_code: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    receipt: Option<ReceiptV1>,
}

#[derive(Serialize)]
struct ReceiptV1 {
    message_id: String,
    platform_message_id: String,
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn valid_local_id(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| {
        id.get_version_num() == 4
            && id.get_variant() == uuid::Variant::RFC4122
            && id.to_string() == value
    })
}

fn valid_remote_id(value: &str) -> bool {
    value.strip_prefix("om_").is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix.len() <= 128
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    })
}

impl FeishuDeliveryResultV1 {
    pub fn new(invocation_id: &str, cfg: &FeishuConfig, target: &str, text: &str) -> Self {
        Self {
            schema: FEISHU_DELIVERY_SCHEMA_V1,
            invocation_id: invocation_id.to_owned(),
            channel: "feishu",
            account_id: cfg.account_id.clone(),
            app_id_sha256: sha256_bytes(cfg.app_id.as_bytes()),
            receive_id_type: cfg.receive_id_type.clone(),
            target_sha256: sha256_bytes(target.trim().as_bytes()),
            content_sha256: sha256_bytes(text.as_bytes()),
            kind: "Uncertain",
            phase: "preflight",
            message_request_started: false,
            reason_code: "feishu_preflight_unconfirmed",
            receipt: None,
        }
    }

    pub fn finish(mut self, evidence: FeishuTextDeliveryEvidence) -> Self {
        match evidence {
            FeishuTextDeliveryEvidence::AuthBeforeMessage => {
                self.kind = "RejectedBeforeMessage";
                self.phase = "auth";
                self.reason_code = "feishu_auth_failed_before_message";
            }
            FeishuTextDeliveryEvidence::MessageResult {
                result,
                message_request_started,
            } => {
                self.message_request_started = message_request_started;
                if message_request_started {
                    self.phase = "message";
                    self.reason_code = "feishu_message_result_unconfirmed";
                    if let Ok(receipt) = result {
                        if let Some(platform_message_id) =
                            receipt.platform_msg_id.filter(|id| valid_remote_id(id))
                        {
                            if valid_local_id(&receipt.message_id) {
                                self.kind = "Accepted";
                                self.reason_code = "accepted";
                                self.receipt = Some(ReceiptV1 {
                                    message_id: receipt.message_id,
                                    platform_message_id,
                                });
                            }
                        }
                    }
                }
            }
            FeishuTextDeliveryEvidence::Unavailable => {}
        }
        self
    }

    pub fn accepted(&self) -> bool {
        self.kind == "Accepted"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::channel_trait::SendReceipt;
    use crate::domain::error::ChannelError;

    fn result() -> FeishuDeliveryResultV1 {
        FeishuDeliveryResultV1::new(
            "TEST_INVOCATION",
            &FeishuConfig::default(),
            " oc_test ",
            " text\n",
        )
    }

    #[test]
    fn later_or_unavailable_failure_cannot_inherit_auth_rejection() {
        for evidence in [
            FeishuTextDeliveryEvidence::Unavailable,
            FeishuTextDeliveryEvidence::MessageResult {
                result: Err(ChannelError::Internal("TEST_SECRET".into())),
                message_request_started: false,
            },
            FeishuTextDeliveryEvidence::MessageResult {
                result: Err(ChannelError::Internal("TEST_SECRET".into())),
                message_request_started: true,
            },
        ] {
            let output = result().finish(evidence);
            assert!(!output.accepted());
            let json = serde_json::to_value(output).unwrap();
            assert_eq!(json["kind"], "Uncertain");
            assert!(json.get("receipt").is_none());
            assert!(!json.to_string().contains("TEST_SECRET"));
        }
    }

    #[test]
    fn accepted_requires_the_remote_id_and_started_marker() {
        let local = "00000000-0000-4000-8000-000000000001";
        let remote = "om_x100b0";
        for (started, local_id, remote_id) in [
            (true, local, None),
            (true, local, Some(" ")),
            (true, "", Some(remote)),
            (true, "local", Some(remote)),
            (false, local, Some(remote)),
            (true, local, Some(remote)),
            (true, local, Some("TEST_SECRET")),
            (true, local, Some("om_")),
            (true, local, Some("om_abc\n")),
            (true, local, Some("om_ABC")),
        ] {
            let output = result().finish(FeishuTextDeliveryEvidence::MessageResult {
                result: Ok(SendReceipt {
                    message_id: local_id.into(),
                    platform_msg_id: remote_id.map(str::to_owned),
                    timestamp_ms: 0,
                }),
                message_request_started: started,
            });
            assert_eq!(
                output.accepted(),
                started && local_id == local && remote_id == Some(remote)
            );
            assert!(!serde_json::to_string(&output)
                .unwrap()
                .contains("TEST_SECRET"));
        }
        assert!(!valid_local_id("00000000-0000-4000-0000-000000000001"));
        assert!(!valid_remote_id(&format!("om_{}", "a".repeat(129))));
    }

    #[test]
    fn auth_rejection_binds_exact_content_and_normalized_target() {
        let json =
            serde_json::to_value(result().finish(FeishuTextDeliveryEvidence::AuthBeforeMessage))
                .unwrap();
        assert_eq!(json["kind"], "RejectedBeforeMessage");
        assert_eq!(json["phase"], "auth");
        assert_eq!(json["message_request_started"], false);
        assert_eq!(json["target_sha256"], sha256_bytes(b"oc_test"));
        assert_eq!(json["content_sha256"], sha256_bytes(b" text\n"));
        assert!(json.get("receipt").is_none());
    }
}
