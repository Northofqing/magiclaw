# 设计:媒体流式上传(红线 2.4)

> 流程阶段:开发流程第 1 步(`/architecture-patterns` 设计文档)
> 关联红线:2.4 「媒体上传支持流式和分段处理,禁止一次性全量读入大文件」
> 架构风格:Hexagonal(Port/Adapter),core 不依赖具体平台与传输实现
> 能力分级:本文档落地时为 `experimental`;接入主链路 + 闭环测试 + 满足红线后方可标 `closed`

---

## 1. 背景与现状

- 现状:[src/channels/wechat/channel.rs](../../src/channels/wechat/channel.rs#L207) 中 `send_message` 对 `Image/File/Unknown` 直接返回
  `"wechat media send is reserved but not yet implemented"`,**无任何媒体上传链路**。
- 仅文本经 [src/channels/wechat/ilink.rs](../../src/channels/wechat/ilink.rs) 的 `send_text_via_ilink` 真正发送。
- 领域模型 [src/domain/entities/message.rs](../../src/domain/entities/message.rs#L48) 已有 `MessageContent::Image { url, media_id }`、
  `File { url, name, size }`,但 outbound 时缺少「字节来源 → 上传 → 引用」的执行路径。
- **外部未知**:ilink 真实媒体上传端点契约(URL、字段、分片/断点协议、返回的 media_id 结构)当前未确认。
  本设计用 Port 抽象隔离该未知,使核心闭环可用 fake adapter 测试,真实 ilink adapter 待契约确认后补齐。

## 2. 目标与非目标

目标:
- 为 outbound 媒体消息提供「流式/分段上传」执行路径,**全程不把整文件读入内存**(红线 2.4)。
- 复用既有 Outbox 状态机 / 重试 / DLQ / 崩溃恢复 / 审计,不另起持久化体系。
- core 与平台解耦:上传能力以 Port 暴露,WeChat 为其中一个 adapter。

非目标:
- 不在本阶段确定 ilink 真实媒体协议细节(契约确认后单独补 adapter + contract test)。
- 不实现媒体下载/转码/缩略图。
- 不把媒体二进制写入 SQLite Outbox(只持久化引用,避免大对象入库)。

## 3. 领域与 Port 设计

### 3.1 字节来源抽象(驱动侧)

```rust
// domain/ports/media_source.rs
pub struct MediaMeta { pub filename: String, pub mime: String, pub size: u64 }

#[async_trait]
pub trait MediaSource: Send + Sync {
    fn meta(&self) -> &MediaMeta;
    /// 打开一个分块字节流。实现必须惰性读取,禁止一次性全量读入内存。
    /// 硬约束(挑战 B2):必须"可重开"——每次发送尝试调用一次 open(),
    /// 失败重试时丢弃旧流并重新 open() 全新流,绝不复用已消费的流。
    async fn open(&self) -> Result<MediaByteStream, MediaError>;
}
// MediaByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, MediaError>> + Send>>
```

- 文件来源 adapter 用 `tokio::fs::File` + `ReaderStream`(逐块产出 `Bytes`),内存占用 = 单块大小。
- 默认分块大小 `media_chunk_bytes`(默认 64 KiB),可配置。

### 3.2 上传 Port(被驱动侧)

```rust
// domain/ports/media_uploader.rs
pub struct MediaRef { pub media_id: String, pub url: Option<String> }

#[async_trait]
pub trait MediaUploader: Send + Sync {
    /// 以分段/流式方式上传 source,返回平台侧引用。实现内部必须分块传输。
    async fn upload(&self, source: &dyn MediaSource) -> Result<MediaRef, MediaError>;
}
```

- adapter:`IlinkMediaUploader`(真实,契约 TBD)以 reqwest streaming body(`reqwest::Body::wrap_stream`)分块 PUT/POST;
  fake:`InMemoryMediaUploader` / 测试用 axum 服务,断言收到的是分块传输且总字节匹配。
- 硬约束(挑战 B3):每次 `upload` 受 `media_upload_timeout_ms`(默认 60s)约束;媒体上传使用**独立隔离舱**
  `BulkheadPools.media`(默认并发 4),不与文本/普通 send 共用 permit,避免大文件长占导致文本发送队头阻塞。
- 依赖方向(挑战 B5):`MediaUploader` 为被驱动 port,由 `WeChatChannel` 构造时注入;组合根 runtime 负责装配。
  core 不感知具体 uploader,无 core→adapter 泄漏。

### 3.3 与现有 `MessageContent` 的关系(模型变更同步,AGENTS 0.5)

- 复用现有 `Image/File` 变体,不新增枚举变体 → **无 schema / 序列化 / recovery 破坏**。
- 约定:`url` 字段可为 `file://<path>` 或 `http(s)://`;`media_id` 为空表示「需上传」,非空表示「已就绪可直接引用」。
- 严格解析(挑战 B6):在 channel 边界用 `MediaLocation::parse(url)` 显式判定 `LocalFile(path)` / `RemoteUrl(url)`,
  **拒绝歧义/不支持的 scheme**(返回 `MediaError::Source`,直接 `dead_letter`),禁止把远程 URL 误当本地路径。
  该解析有独立单元测试覆盖各 scheme 与非法输入。若后续歧义频发,再评估升级为类型化 `MediaLocation` 变体(届时走 0.5 模型同步)。
- 0.5 同步检查表(本设计结论):
  - 领域模型:复用,无变更。
  - 持久化 schema:Outbox 仍存 `MessageContent` JSON(仅引用/路径,不含二进制)→ 无变更。
  - 序列化边界:不变。
  - crash recovery / DLQ replay:沿用 Outbox;重放时按 `url` 重新打开来源重新上传(见 4.3 幂等)。
  - audit:上传决策与结果写 `audit_log`(action `media_upload` / result `ok|failed`)。
  - 集成测试:新增媒体闭环测试。

## 4. 数据流与失败模式

### 4.1 主数据流图

```
Outbound 媒体发送请求 (MessageContent::Image/File, media_id=None)
  → Outbox.insert(pending)                         [既有]
  → OutboxWorker.dequeue                            [既有]
  → ResilientOutboxSender (send gate: breaker+bulkhead)  [既有]
  → RegistryOutboxSender → registry.send_via        [既有]
  → WeChatChannel.send_message(Image/File)          [新增分支]
       ├─ media_id 为空 → MediaUploader.upload(MediaSource.open() 分块流)  [新增]
       │        └─ 全程分块,never read-to-end          (红线 2.4)
       │        → 得到 MediaRef.media_id
       └─ ilink sendmessage(媒体引用 media_id)         [新增, 契约 TBD]
  → Outbox.sent  (audit: media_upload=ok, send=sent) [既有状态机]
```

### 4.2 失败模式分析(每个来源/步骤)

| 失败点 | 触发 | 处理 | 终态 |
|--------|------|------|------|
| 来源打开失败 / 非法 scheme | 文件不存在/无权限/歧义 url | `MediaError::Source`;不可重试类 | 直接 `dead_letter` + audit failed |
| 上传超时 | 超 `media_upload_timeout_ms` | 中止本次,释放独立 permit | `retrying`→阈值后 `dead_letter` |
| 文件超限 | size > 上限(边界校验,上传前) | `MediaError::TooLarge`,早拒 | `dead_letter`(不无限重试) |
| 上传网络抖动 | 连接/超时 | send gate 熔断 + Outbox 重试退避 | `retrying`→阈值后 `dead_letter` |
| 上传中断/部分上传 | 流中途失败 | 无 `media_id` 产出 → 视为发送失败 | 重试时整体重传(见 4.3) |
| token 过期 | ret=-2 | 复用文本同款 refresh→重试 1 次 | 成功或 `retrying` |
| ilink 媒体业务错误 | errcode≠0 | 解析 ret/errcode 报错 | `retrying`/`dead_letter` |

- 边界校验仅在系统边界做一次(上传前 size 上限);不在内部臆造校验(遵循实现纪律)。

### 4.3 幂等与恢复

- 崩溃恢复沿用 Outbox:进程重启后 `pending/sending/retrying` 条目被 crash_recovery 重新投递。
- 重放即「重新打开来源 → 重新上传」。为避免重复媒体:上传请求携带 **client 端内容指纹(hash)** 作为幂等键;
  若平台支持去重则复用既有 media_id,否则重复上传可接受(对账以 Outbox `sent` 为准)。
- 媒体二进制**不入库**,因此恢复依赖 `url`/路径仍可访问;来源不可达 → 走「来源打开失败」失败模式。
- DLQ 可诊断(挑战 B4):进入 `dead_letter` 的媒体条目必须携带**结构化错误原因**(失败阶段 = source/upload/send + 平台 ret/errcode),
  使运维与用户可从 DLQ 直接判因。

## 5. 回滚方案

- 能力为**纯增量**:媒体分支当前是「reserved 报错」。
- 回滚 = 将 `WeChatChannel::send_message` 媒体分支恢复为现有 `Err("...reserved...")`;
  `MediaUploader` / `MediaSource` port 与 adapter 成为未引用代码,可整体删除。
- **无 schema 变更、无数据迁移**,回滚零数据风险。
- 配置开关:`wechat.media_upload_enabled`(默认 false),未开启时维持现状报错,实现灰度与即时关停。

## 6. 与旧模块的关系(旧模块接入检查表预填)

| 旧模块 | 是否升级接入 | 说明 |
|--------|--------------|------|
| `WeChatChannel::send_message` | 是 | 新增 Image/File 分支调用 MediaUploader |
| Outbox / OutboxWorker / DLQ | 否(复用) | 媒体走同一状态机,无需改造 |
| `ResilientOutboxSender` / send gate | 否(复用) | 上传纳入既有熔断+隔离舱 |
| `audit_log` / AuditSink | 是(扩展) | 增加 `media_upload` 动作留痕 |
| `send_text_via_ilink` | 否 | 文本路径不变;媒体新增独立函数 |
| MCP/REST adapter | 否(本阶段) | 暂不暴露媒体上传入口,后续阶段评估 |

## 7. 阶段主闭环定义(唯一)

- **入口**:`MessageContent::Image/File`(media_id=None)的 outbound 请求进入 Outbox(`pending`)。
- **路径**:Outbox → Worker → send gate → registry → `WeChatChannel.send_message(media)` →
  `MediaUploader.upload(分块流)` → ilink 媒体引用发送。
- **出口**:Outbox `sent`(audit `media_upload=ok` + `send=sent`);失败 → `retrying`→`dead_letter`;崩溃 → 恢复续传。

### 闭环验收测试(系统/集成级,合并 Gate)

1. 启动 fake 媒体服务:断言上传体为**分块传输**、累计字节 = 文件大小、服务端无需整体缓冲即可校验。
2. 上传返回 media_id,后续 sendmessage 引用该 media_id;Outbox 终态 `sent`。
3. 来源打开失败 → `dead_letter` + audit failed。
4. 超限文件 → 上传前早拒 → `dead_letter`(不重试)。
5. (内存断言)流式读取期间常驻内存 ≈ 单块大小,验证「禁止全量读入」。

## 8. 待澄清(外部依赖,挑战后仍开放)

- ilink 真实媒体上传端点契约(URL/字段/分片协议/media_id 结构)—— 决定 `IlinkMediaUploader` 能否标 `closed`。
- 幂等键策略(内容 hash vs 平台去重)是否需平台确认。

> 注:大小上限 / 分块大小 / 上传超时 / 独立隔离舱并发 已在四角挑战中定为可配置项(见 §3、§9),不再阻塞。

## 9. 四角挑战记录与收敛(开发流程第 2 步)

退出条件:无新增 Blocking 异议。共 2 轮收敛,未触及 3 轮上限。

### 第 1 轮 Blocking 异议与处置

| 编号 | 角色 | Blocking 异议 | 处置(已反哺设计) | 状态 |
|------|------|---------------|--------------------|------|
| B1 | AI 产品经理 | ilink 契约未知,`IlinkMediaUploader` 无法 `closed`,阶段交付物无依据 | §7 拆分交付物:Port + 文件 `MediaSource` + fake-uploader 闭环标 `closed`;真实 ilink adapter 标 `experimental` 待契约 | resolved |
| B2 | 网络研发 | reqwest 流式 body 一次性,重试不能复用已消费流 | §3.1 硬约束:`open()` per attempt + 来源可重开;§4.3 重放即重新 open | resolved |
| B3 | 网络研发 | 大文件长占 send 隔离舱 permit → 文本队头阻塞 | §3.2 独立 `BulkheadPools.media`(并发 4)+ `media_upload_timeout_ms`(60s) | resolved |
| B4 | 用户 | 媒体异步永久失败无可见信号 | §4.3 DLQ 必带结构化错误原因(source/upload/send 阶段 + ret/errcode),可诊断 | resolved |
| B5 | DDD 架构 | uploader 调用位置导致 adapter→adapter 耦合 | §3.2 `MediaUploader` 经 `WeChatChannel` 构造注入,runtime 装配,core 不感知 | resolved |
| B6 | DDD 架构 | `url` 字符串过载语义,易误发 | §3.3 channel 边界 `MediaLocation::parse` 严格判定 + 拒绝歧义 scheme + 单测 | resolved |

非 Blocking(记录,不阻塞):上传可观测指标(字节/耗时/失败率)纳入 §5 配置开关同期埋点;连接复用 / multipart vs 流式 PUT 待 ilink 契约定。

### 第 2 轮:复检收敛产物是否引入新 Blocking

- 独立 media 隔离舱 → 仅新增 resilience 配置项,非 Blocking。
- `MediaLocation` 严格解析 → 有单测覆盖,非 Blocking。
- 交付物拆分(closed/experimental)→ 符合 AGENTS 0.4 能力分级,非 Blocking。

**结论:第 2 轮无新增 Blocking 异议,四角挑战收敛。可进入第 3 步 `/project-planner` 拆分计划。**

### 阶段交付物拆分(B1 结论,补充 §7)

- `closed`(本阶段计入完成):`MediaSource` port + `FileMediaSource`(可重开、分块、超时)+ `MediaUploader` port +
  fake/in-memory uploader + `WeChatChannel` 媒体分支(注入 uploader)+ `MediaLocation` 严格解析 + 独立 media 隔离舱 +
  DLQ 结构化错误 + 系统级闭环测试(§7 验收 1–5)。
- `experimental`(不计入完成):`IlinkMediaUploader` 真实端点实现 —— 待 §8 ilink 媒体契约确认后,补 contract test 方可升 `closed`。
