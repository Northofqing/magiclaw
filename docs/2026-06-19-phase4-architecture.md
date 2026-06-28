# Phase 4: Pipeline + Pluggable AI

## 0. Review Gate 对齐

本阶段文档中的 AI backend、middleware 仅在实现并接入主链路后才算 `closed`。当前如果仍是 `Echo/OpenAI/Claude/Local stubs`，只能作为占位说明，不能计入阶段完成度。

## Middleware Chain

```
Message → Normalize → Dedup → Permission → RateLimit → ConversationLoad → AI → Formatter → Outbox
```

## AI Backend trait + Echo/OpenAI/Claude/Local stubs

## Ports: Middleware trait, AiBackend trait
