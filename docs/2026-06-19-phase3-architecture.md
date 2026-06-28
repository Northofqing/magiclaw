# Phase 3 Architecture: 多信道扩展

**版本**: 1.0 | **日期**: 2026-06-17 | **状态**: Draft

## 0. Review Gate 对齐

本阶段仅描述多信道扩展方向；Dingtalk / Feishu 的 skeleton 仍属于 `stub`，不能写入已完成能力清单。Phase 3 只有在以下条件满足时才算 closed：

- 双信道并行启动，且路径级隔离生效。
- 单信道故障不会影响其他信道。
- ChannelRegistry 的注册、健康检查、启动/停止都接入运行时组合根。

## 1. 目标

- Dingtalk/Feishu 信道骨架 (register, send stub, poll stub)
- Channel trait 统一定义
- ChannelRegistry 多信道管理 + 健康检查
- 路径级隔离: 单信道故障不影响其他信道

## 2. Channel Trait

```rust
#[async_trait]
pub trait Channel: Send + Sync {
    fn id(&self) -> ChannelId;
    async fn start(&self, inbound_tx: Sender<Message>) -> Result<()>;
    async fn send_message(&self, to: &str, content: &MessageContent) -> Result<SendReceipt>;
    async fn stop(&self) -> Result<()>;
    async fn health(&self) -> Result<HealthStatus>;
}
```

## 3. ChannelRegistry

管理所有信道实例, 提供统一的生命周期管理:
- `register(channel)` — 注册信道
- `start_all()` — 并行启动所有信道
- `stop_all()` — 优雅停止
- `health_check()` — 返回各信道状态
- 信道隔离: 每个信道独立的 inbound channel

## 4. 验收清单

- [ ] Channel trait 定义
- [ ] WeChatChannel 实现 Channel trait
- [ ] DingtalkChannel skeleton
- [ ] FeishuChannel skeleton
- [ ] ChannelRegistry 多信道管理
- [ ] 双信道并行 + 路径隔离
- [ ] 单信道故障不影响其他信道
