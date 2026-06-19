# 计划:媒体流式上传 任务拆分(开发流程第 3 步)

> 输入:设计文档 [media-streaming-upload-design.md](media-streaming-upload-design.md)(已过第 2 步四角挑战收敛)
> 约束:每个任务可独立验收、依赖明确、标注涉及红线;本阶段**唯一主闭环**见 §主闭环。
> 能力分级遵循 §交付物拆分:`closed`(计入完成)/ `experimental`(`IlinkMediaUploader`,待契约)。

---

## 主闭环(唯一)

入口 `MessageContent::Image/File(media_id=None)` → Outbox(pending) → Worker → send gate →
registry → `WeChatChannel.send_message(media)` → `MediaUploader.upload(分块流, 不全量读)` →
媒体引用发送 → Outbox `sent`;失败 → `retrying`→`dead_letter`(带结构化原因);崩溃 → 恢复续传。

合并 Gate = 该主闭环的系统级集成测试(T8)通过。

---

## 任务清单

> 状态列:`todo`。验收列即该任务 Definition of Done。

### T1 — 领域错误与 Port:`MediaSource` / `MediaUploader`
- 内容:新增 `domain/ports/media_source.rs`(`MediaMeta`、`MediaSource`、`MediaByteStream`)、
  `domain/ports/media_uploader.rs`(`MediaRef`、`MediaUploader`)、`MediaError`(Source/TooLarge/Upload/Send 分类)。
- 依赖:无(可起步)。
- 红线:2.4(流式契约接口)、2.1(Port 在 domain,core 不依赖 adapter)。
- 验收:编译通过;trait 文档注明「open() per attempt + 可重开 + 禁止全量读入」;`MediaError` 单测覆盖分类。
- 分级:`closed`。

### T2 — `MediaLocation` 严格解析(边界防误发)
- 内容:`channels/wechat/media.rs` 内 `MediaLocation::parse(&str) -> Result<MediaLocation, MediaError>`,
  判定 `LocalFile(path)` / `RemoteUrl(url)`,拒绝歧义/不支持 scheme。
- 依赖:T1(`MediaError`)。
- 红线:2.4(B6 收敛项)。
- 验收:单测覆盖 `file://`、`http(s)://`、空串、未知 scheme、相对路径歧义 → 各自预期结果。
- 分级:`closed`。

### T3 — `FileMediaSource`(可重开 + 分块 + 大小上限)
- 内容:`channels/wechat/media.rs` 实现 `MediaSource`,基于 `tokio::fs::File` + `tokio_util::io::ReaderStream`,
  `open()` 每次返回全新流;`meta()` 提供 size/mime/filename;分块 `media_chunk_bytes`(默认 64 KiB)。
- 依赖:T1、T2。
- 红线:2.4(禁止全量读入)。
- 验收:单测断言多次 `open()` 各自独立可读完整;内存常驻 ≈ 单块(用大临时文件 + 分块计数验证)。
- 分级:`closed`。

### T4 — Fake/In-memory `MediaUploader` + 上传 Port 行为契约
- 内容:测试用 `InMemoryMediaUploader`(consume 流、累加字节、产出 `MediaRef`);
  以及一个测试 axum 媒体服务,断言**分块传输**与总字节匹配。
- 依赖:T1。
- 红线:2.4(闭环可验证流式)。
- 验收:单测/集成断言上传为分块、字节数 = 文件大小、返回 media_id 可用。
- 分级:`closed`(测试基建)。

### T5 — 独立 media 隔离舱 + 上传超时
- 内容:扩展 `core/resilience/bulkhead.rs` `BulkheadPools` 增加 `media`(默认并发 4);
  resilience 配置增加 `media_upload_timeout_ms`(默认 60s);上传路径包裹超时 + media permit。
- 依赖:T1。
- 红线:2.5(Bulkhead 隔离)、2.4(超时约束)。
- 验收:单测验证 media permit 与 send permit 互不占用;超时触发返回 `MediaError::Upload` 并释放 permit。
- 分级:`closed`。

### T6 — `WeChatChannel` 媒体分支(注入 uploader)
- 内容:`WeChatChannel` 构造注入 `Arc<dyn MediaUploader>`;`send_message` 的 `Image/File` 分支:
  解析 `MediaLocation` → 若 `media_id` 空则 `upload(FileMediaSource)` → 用 `MediaRef` 走媒体引用发送;
  替换现有 `"reserved but not yet implemented"` 报错。token 过期复用文本 ret=-2 refresh 路径。
- 依赖:T1–T5。
- 红线:2.1(channel→port,无 core→adapter 泄漏)、2.4。
- 验收:单测(stub 模式 + fake uploader)媒体发送成功返回 receipt;来源失败/超限走对应错误。
- 分级:`closed`(WeChat 媒体分支);真实 ilink 发送字段待 T9。

### T7 — DLQ 结构化错误 + 审计 `media_upload`
- 内容:dead_letter 写入携带失败阶段(source/upload/send)+ 平台 ret/errcode;
  `AuditSink` 记录 `media_upload` 动作(ok/failed)。
- 依赖:T6。
- 红线:2.3(DLQ)、2.6(审计留痕)。
- 验收:集成测试断言失败媒体进入 DLQ 且 reason 含阶段与错误码;audit_log 有 `media_upload` 记录。
- 分级:`closed`。

### T8 — 主闭环系统级集成测试(合并 Gate)
- 内容:`tests/media_upload_closed_loop.rs`:Outbox(media)→Worker→send gate→channel→fake 媒体服务,
  覆盖设计 §7 验收 1–5(分块上传、media_id 引用、来源失败 dead_letter、超限早拒、流式内存断言)。
- 依赖:T1–T7。
- 红线:全部上述;AGENTS 0.3(阶段需系统级验收测试)。
- 验收:测试通过即主闭环成立;`cargo test` 全绿;改动文件 clippy 干净。
- 分级:`closed`(Gate)。

### T9 — `IlinkMediaUploader`(真实,待契约)
- 内容:`IlinkMediaUploader` 实现真实 ilink 媒体上传(reqwest `wrap_stream` 分块);补 contract test
  (端点、字段、ret/errcode、media_id 结构)。
- 依赖:T1–T8 + **外部:ilink 媒体契约确认(设计 §8)**。
- 红线:2.4(contract test)。
- 验收:contract test 覆盖真实契约关键字段;接入 runtime 装配根后由 `experimental` 升 `closed`。
- 分级:`experimental` → 契约确认且测试通过后 `closed`。**本阶段不阻塞合并。**

### T10 — runtime 装配 + 配置开关
- 内容:`AppConfig` 增加 `wechat.media_upload_enabled`(默认 false)、`media_chunk_bytes`、`media_upload_timeout_ms`、
  media 隔离舱并发;runtime 组合根装配 uploader 注入 `WeChatChannel`(开关关 → 维持现状报错)。
- 依赖:T5、T6;(真实路径)T9。
- 红线:2.4(默认不裸开/可灰度)、0.2(运行时装配,非初始化即弃)。
- 验收:开关 false 时媒体仍报错(回滚等价);true + fake uploader 时主闭环生效;`Default` 同步更新。
- 分级:`closed`(装配)。

---

## 依赖图

```
T1 ──┬─ T2 ── T3 ──┐
     ├─ T4         ├─ T6 ── T7 ── T8(Gate)
     └─ T5 ────────┘                │
                                     └─ T10(装配/开关)
T9(experimental, 外部契约阻塞) ┄┄ 依赖 T1–T8,独立于 Gate
```

## 并行性

- 可并行起步:T2、T4、T5(均仅依赖 T1)。
- 串行关键路径:T1 → T3 → T6 → T7 → T8。
- T9 旁路:不在合并关键路径,待外部契约。

## 风险与缓解

| 风险 | 缓解 |
|------|------|
| ilink 媒体契约长期不确定 | T9 隔离为 `experimental`,主闭环用 fake uploader 闭环,**不阻塞本阶段合并** |
| 流式重试复用已消费流(回归) | T1 接口硬约束 + T3 多次 open 单测 + T8 重试路径覆盖 |
| 大文件占用资源拖垮文本发送 | T5 独立隔离舱 + 上传超时,T8 验证隔离 |
| 媒体误发(本地/远程混淆) | T2 严格解析 + 拒绝歧义 + 单测 |

## 阶段完成判定

- T1–T8、T10 全部 `closed` 且 T8 主闭环 Gate 通过 → 阶段「媒体流式上传(fake/file 路径)」完成。
- T9 标 `experimental`,在 ilink 契约确认前**不计入完成度**,确认后单独升级。

---

## 第 5 步:Review 记录(开发流程第 5 步)

审查范围:本次 `closed` 增量(T1–T6、T8)。审查者:自审 + 静态检查(clippy/test)。

### Review 意见(全部记录)

| 编号 | 严重度 | 文件 | 意见 | 处置 |
|------|--------|------|------|------|
| F1 | **Blocking-for-T10**(本阶段 Non-blocking) | [media.rs](../src/channels/wechat/media.rs) `MediaLocation::LocalFile` | `file://` 允许上传任意本地文件(OWASP 路径遍历/本地文件泄露)。当前**无任何 runtime 入口**暴露媒体发送(未接 runtime/MCP/REST),风险潜伏未触达。 | **T10 接入 live 前必须**加「允许根目录」白名单约束 + 拒绝越界路径;在 T10 前不得放开 live 媒体入口。已登记为 T10 前置阻塞项。 |
| F2 | Non-blocking | [channel.rs](../src/channels/wechat/channel.rs) `send_media` | 计划 T7 的 `media_upload` 审计动作尚未单独记录。 | 现有 outbox_worker 通用审计已覆盖 send/sent/dead_letter;`media_upload` 专项动作在 T10 接 live outbox 路径时补记。 |
| F3 | Nit | [media.rs](../src/channels/wechat/media.rs) `FileMediaSource::new` | metadata 大小校验与 `open()` 之间存在 TOCTOU。 | 媒体场景可接受;size 仅用于限额与 meta,不做安全判定。wontfix(记录)。 |
| F4 | 正向确认 | media.rs / channel.rs | 来源可重开(每次 attempt 新 `open()`)、独立 media 隔离舱、上传超时均有测试覆盖。 | 符合四角挑战 B2/B3 收敛项。 |

无本阶段 Blocking 项;F1 已显式登记为 T10 前置门禁。

### 旧模块接入检查表(MUST)

| 旧模块 | 是否升级接入 | 结论/计划 |
|--------|--------------|-----------|
| `WeChatChannel::send_message` | 是 | 已新增 Image/File 媒体分支(注入 uploader) |
| `ChannelRegistry::send_via` | 否(复用) | 媒体走同一 `send_via`,无需改造(T8 已验证) |
| `DingtalkChannel` / `FeishuChannel` `send_message` | 否(暂不) | 仍为 skeleton,媒体上传待其平台实现;记录不接入理由:无真实平台契约 |
| Outbox / OutboxWorker / DLQ | 否(复用) | 媒体复用状态机;DLQ 结构化原因随 T10 live 接入补全(F2) |
| `ResilientOutboxSender` / send gate | 否(复用) | 媒体上传隔离用**独立** media 隔离舱,不与 send gate 混用(B3) |
| `AuditSink` / `audit_log` | 待接(T10) | `media_upload` 动作待 live 接入记录(F2) |
| MCP / REST adapter | 否(暂不) | 本阶段不暴露媒体入口;F1 要求 live 暴露前先加路径白名单 |
| `send_text_via_ilink` | 否 | 文本路径不变,媒体独立分支 |
| 确认无「应接入却遗漏」 | 是 | 真实 ilink 媒体发送(T9)因外部契约未知,显式 `experimental`,非遗漏 |

### 迁移红线检查表(MUST)

| 红线 | 条目 | 状态 |
|------|------|------|
| 2.4 | 媒体上传流式/分段,禁止全量读入 | ✅ `FileMediaSource` 分块 + T8 断言 `max_chunk ≤ chunk_bytes`、`bytes==size` |
| 2.5 | Bulkhead 隔离(媒体不拖垮文本) | ✅ 独立 media 隔离舱 + 上传超时,单测+集成验证 |
| 2.1 | core 不依赖 adapter;Port 在 domain | ✅ `MediaSource`/`MediaUploader` 在 `domain/ports`,channel 注入 |
| 2.3 | 失败进 DLQ/重试 | ⚠️ 部分:错误带 `media[stage]` 可传播;落库 DLQ 随 T10 live(F2) |
| 2.6 | 关键发送决策审计 | ⚠️ 部分:通用 outbox 审计覆盖;`media_upload` 专项待 T10(F2) |
| 安全 | 路径遍历/本地文件泄露 | ⚠️ F1:当前未暴露;T10 前置加白名单(门禁) |

### 能力分级结论(AGENTS 0.4)

- `closed`(计入完成):T1 ports、T2 `MediaLocation`、T3 `FileMediaSource`、T4 fake uploader、T5 `ResilientMediaUploader`、T6 channel 媒体分支、T8 系统级闭环测试。
- `experimental`(不计入):T9 真实 `IlinkMediaUploader` + ilink 媒体发送。
- `pending`(不计入,有门禁):T10 runtime 装配 —— 前置 F1 路径白名单 + F2 审计补记。

**Review 结论:本阶段 `closed` 增量无 Blocking 问题,可进入第 6 步(无需修复项,F1/F2 登记为 T10 前置门禁)。**
