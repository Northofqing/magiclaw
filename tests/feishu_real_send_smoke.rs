// tests/feishu_real_send_smoke.rs
//
// 临时实机冒烟测试：通过 magiclaw 自己的 FeishuChannel（OpenApi 真实模式）
// 直接调用飞书 OpenAPI 发送一条文本消息，验证代码路径端到端可用。
//
// 运行: cargo test --test feishu_real_send_smoke -- --ignored --nocapture
// 需要环境变量: FEISHU_APP_ID, FEISHU_APP_SECRET, FEISHU_CHAT_ID

use magiclaw::channels::channel_trait::Channel;
use magiclaw::channels::feishu::channel::FeishuChannel;
use magiclaw::domain::entities::message::MessageContent;
use magiclaw::infrastructure::config::FeishuConfig;

#[tokio::test]
#[ignore = "实机测试，需要真实飞书凭证与 chat_id"]
async fn feishu_real_send_text() {
    let app_id = std::env::var("FEISHU_APP_ID").expect("FEISHU_APP_ID required");
    let app_secret = std::env::var("FEISHU_APP_SECRET").expect("FEISHU_APP_SECRET required");
    let chat_id = std::env::var("FEISHU_CHAT_ID").expect("FEISHU_CHAT_ID required");

    let cfg = FeishuConfig {
        enabled: true,
        app_id,
        app_secret,
        receive_id_type: "chat_id".to_string(),
        ..Default::default()
    };

    let channel = FeishuChannel::from_config(cfg);
    let content = MessageContent::Text("magiclaw 自有通道实机验证 ✅ (via FeishuChannel)".to_string());

    let receipt = channel
        .send_message(&chat_id, &content)
        .await
        .expect("feishu send should succeed");

    println!("发送成功: platform_msg_id={:?}", receipt.platform_msg_id);
    assert!(receipt.platform_msg_id.is_some(), "应返回平台消息 ID");
    assert!(
        receipt.platform_msg_id.as_deref().unwrap().starts_with("om_"),
        "真实飞书消息 ID 应以 om_ 开头，得到: {:?}",
        receipt.platform_msg_id
    );
}
