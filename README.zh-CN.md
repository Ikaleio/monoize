<div align="center">

<img src="frontend/public/monoize.svg" width="96" alt="Monoize 标志">

# Monoize

**AI API 接口形式相似，但底层协议约定各不相同。**

Monoize 是基于 Rust 开发的 AI API 网关。它在网关层完成 OpenAI Responses、Chat Completions、Anthropic Messages 之间的语义转换，将一个逻辑模型路由到多个上游 Channel，并在单个进程内提供管理控制台。

[English](README.md) · [简体中文](README.zh-CN.md)

</div>

<div align="center">
  <img src="docs/public/images/zh/dashboard.webp" width="880" alt="Monoize 控制台预览">
</div>

## 为什么需要 Monoize

AI API 网关需要解决的不仅是 JSON 字段映射。

Responses、Chat Completions 与 Messages 对对话历史、推理过程、工具调用、用量统计与流式事件的数据建模各不相同。简单的字段映射即使返回 HTTP 200，也可能破坏会话状态：

1. **丢失推理上下文**：Responses 使用 `encrypted_content` 在无状态请求间传递推理状态。无法表示该字段的转换器会在多轮对话中静默丢弃它。
2. **破坏流式生命周期**：各协议对内容块的开启与闭合规则不同。在文本块内发送推理增量，或重复发送初始化事件，会让下游 SDK 丢弃数据。
3. **流式切换串流**：网关必须重试失败的上游。但向下游发送首个响应字节后，切换上游会把两次不同的生成内容拼进同一条流。

Monoize 通过类型化协议模型、流状态机与有界路由瀑布处理这些问题。

## 核心设计

### 1. URP v2 协议模型

Monoize 将每个接入协议解码为 URP v2。URP v2 是扁平的类型化表示，将文本、推理摘要、原始推理、加密推理、工具调用、工具结果、图像、文件、拒答、用量与控制边界表示为独立节点。上游适配器将节点编码为目标协议，响应按相反方向转换。

- 明文推理与加密推理分离表示。可选的 `mz2` 信封在不兼容的重放格式间保留不透明推理数据。
- 工具调用 ID、并行调用、多段工具结果与助手历史保持原有角色。
- Responses 输出项与 Messages 内容块的生命周期事件保持配对。
- 同协议族内的未知字段透传；跨协议族转换时剥离目标协议无法表示的嵌套字段。

### 2. 首字节前重试

一个逻辑模型可匹配多个有序 Provider，每个 Provider 包含带权重的 Channel。

1. 选择第一个匹配的 Provider。
2. 按权重与 Channel 亲和性选择健康 Channel。
3. 在配置的预算内重试可重试失败。
4. 当前路由耗尽后推进到下一个路由。
5. 发送首个响应字节后停止回退。

网络错误、超时、`429` 与指定的 `5xx` 推进瀑布；`400`、`401`、`403`、`422` 终止瀑布。熔断器、被动健康检测、主动探测与冷却期将异常 Channel 排除在路径之外。Monoize 不会在可见流的中途切换 Provider。规则见[路由规范](spec/monoize-upstream-routing.spec.md)。

### 3. 低转发开销

- Rust 与 Tokio 处理异步 I/O，请求路径无解释器。
- 默认流式路径通过有界通道增量解码与编码。
- 用量随流式增量到达即时累加，不缓冲完整响应文本。

部分响应 Transform 需要重构完整响应时会使用缓冲合成流，Replicate 也使用该路径。默认桥接保持增量。这里比较的是代理自身的 CPU、内存与延迟，不代表上游模型生成更快。

## 功能范围

**协议转换**：Responses、Chat Completions、Messages 之间的流式与非流式互转；Gemini、OpenAI 图像 API 与 Replicate 作为上游接入。

**路由**：Provider 有序回退、Channel 权重分流、熔断与主动探测、Channel 亲和性、按 API Key 的模型重定向。

**Transform 边界适配**，可挂载在全局、Provider 或 API Key 级别，按模型通配符匹配：

- OpenRouter 结构化推理与末尾用量块。
- DeepSeek 工具循环中的推理重放。
- Anthropic thinking 块与签名。
- Codex Responses WebSocket 会话与 `/v1/responses/compact`。
- 系统提示词、工具定义与历史消息的 Prompt Cache 断点。
- `compress_user_message_images`：按需重压缩用户内联图像为 JPEG、PNG、WebP 或 JPEG XL，缩短 TTFT。
- 自定义 JavaScript Transform：在运行时改写请求与响应。
- SSE 帧拆分、孤立工具调用清理、连续同角色合并、`system`/`developer` 角色映射。

**运营**：

- 内嵌 React 控制台：Provider、Channel、模型映射、定价、用户、API Key 与子账户。
- 纳美元精度计费、倍率与追加式账本；价格同步自 [models.dev](https://models.dev)、[OpenRouter](https://openrouter.ai) 与 new-api。
- 请求日志：TTFB、耗时、Token、费用、错误与尝试过的路由。
- Request Capture：按请求查看事件时间线，可选开启且有容量上限。
- 内置 Cap 工作量证明人机验证，无需外部 Captcha 服务。
- Prometheus `/metrics`。

## 请求路径

```text
客户端协议 (Responses / Chat Completions / Messages)
    │
    ▼
解码为 URP v2
    │
    ▼
Provider 瀑布 ──► 加权 Channel ──► 熔断 / 亲和性
    │                                  │
    │                        首字节前重试或推进
    ▼
Transform（全局 / Provider / API Key）
    │
    ▼
编码为上游协议
    │
    ▼
上游流 ──► URP v2 事件 ──► 下游协议事件
```

## 快速开始

### npm / Bun

```bash
bunx monoize
# 或: npx monoize
```

全局安装：

```bash
bun add --global monoize
monoize
```

包管理器只安装当前系统与 CPU 对应的原生二进制。支持 Linux x86-64/ARM64（glibc 与 musl）及 Windows x86-64。

### Docker

```bash
docker run -d \
  --name monoize \
  --restart unless-stopped \
  -p 8080:8080 \
  -v monoize-data:/app/data \
  ghcr.io/ikaleio/monoize:latest
```

`docker-compose.yml`：

```yaml
services:
  monoize:
    image: ghcr.io/ikaleio/monoize:latest
    restart: unless-stopped
    ports:
      - "8080:8080"
    volumes:
      - ./data:/app/data
    # PostgreSQL 时设置:
    # environment:
    #   - MONOIZE_DATABASE_DSN=postgres://user:pass@host/monoize
```

### 源码构建

需要 Rust 稳定版工具链与 [Bun](https://bun.sh/)。Release 构建会编译前端并嵌入可执行文件。

```bash
cargo build --release
./target/release/monoize
```

### 首次配置

打开 `http://localhost:8080`。首个注册账号成为 `super_admin`，即使已关闭公开注册。

1. 创建 Provider。
2. 添加至少一个 Channel，填入上游地址与凭证。
3. 将逻辑模型映射到 Channel。
4. 创建 API Key。

```bash
curl http://localhost:8080/v1/chat/completions \
  -H 'Authorization: Bearer sk-your-monoize-key' \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "your-logical-model",
    "messages": [{"role": "user", "content": "你好"}],
    "stream": true
  }'
```

## 支持范围

### 下游端点

| 方法 | 端点 | 协议 |
| --- | --- | --- |
| `GET` | `/v1/models` | OpenAI 兼容模型列表 |
| `POST` | `/v1/responses` | OpenAI Responses，流式或非流式 |
| `GET` | `/v1/responses` | OpenAI Responses WebSocket 传输 |
| `POST` | `/v1/responses/compact` | Responses 上下文压缩 |
| `POST` | `/v1/chat/completions` | OpenAI Chat Completions |
| `POST` | `/v1/messages` | Anthropic Messages |
| `POST` | `/v1/embeddings` | Embeddings |
| `POST` | `/v1/images/generations` | 图像生成 |
| `POST` | `/v1/images/edits` | Multipart 图像编辑 |

所有转发端点均有 `/api/v1/...` 别名。

### 上游 Channel 类型

| 类型 | 上游原生协议 |
| --- | --- |
| `responses` | OpenAI Responses 兼容 |
| `chat_completion` | OpenAI Chat Completions 兼容 |
| `messages` | Anthropic Messages 兼容 |
| `gemini` | Google Gemini 原生 |
| `openai_image` | OpenAI 兼容图像 API |
| `replicate` | Replicate Predictions |

## 配置

运行时引导使用环境变量。Provider、Channel、模型、路由、Transform、用户与 API Key 存储在数据库中，由控制台管理。

| 环境变量 | 默认值 | 说明 |
| --- | --- | --- |
| `MONOIZE_LISTEN` | `0.0.0.0:8080` | HTTP 监听地址 |
| `MONOIZE_DATABASE_DSN` | `sqlite://./data/monoize.db` | SQLite 或 PostgreSQL DSN |
| `MONOIZE_METRICS_PATH` | `/metrics` | Prometheus 指标路径 |
| `MONOIZE_HTTP_BODY_MAX_BYTES` | `52428800` | 转发请求体上限 |
| `MONOIZE_TRUSTED_PROXY_CIDRS` | `127.0.0.0/8,::1/128` | 受信任的反向代理网段；显式设为空则禁用 |
| `MONOIZE_UPSTREAM_PROXY_URL` | 未设置 | 节点级出站 HTTP(S) 代理；Channel 可通过 `proxy_url` 覆盖 |
| `MONOIZE_CAP_API_ENDPOINT` | 未设置 | 外部 Cap 站点端点；未设置时使用内置 Cap 服务 |
| `MONOIZE_CAP_SECRET_KEY` | 未设置 | 外部 Cap 站点密钥，与上一项同时配置 |

### 主从部署

Monoize 可运行为一个可写 Primary 加多个只读 Replica，所有节点共享一个 PostgreSQL 数据库。Replica 只服务 `/v1/**` 流量，不提供控制台。Replica 通过鉴权的内部 API 向 Primary 发送请求日志与计费增量，余额检查会扣除本地尚未发送的费用以限制超支。故障转移为手动：切换角色并重启。详见[主从部署规范](spec/primary-replica-deployment.spec.md)。

| 环境变量 | 默认值 | 说明 |
| --- | --- | --- |
| `MONOIZE_NODE_ROLE` | `primary` | `primary` 或 `replica` |
| `MONOIZE_PRIMARY_INTERNAL_URL` | Replica 必填 | Primary 的内部地址 |
| `MONOIZE_REPLICA_TOKEN` | 未设置 | 共享密钥；Replica 必填，Primary 设置后开启接收端点 |
| `MONOIZE_REPLICA_ID` | 自动生成并持久化 | 固定 Replica 身份（UUID v4） |
| `MONOIZE_CONFIG_POLL_INTERVAL_SECONDS` | `5` | Replica 配置轮询间隔 |
| `MONOIZE_METERING_SHIP_INTERVAL_SECONDS` | `10` | Replica 计量发送间隔 |
| `MONOIZE_REPLICA_METERING_SPOOL_DIR` | `./data/replica-metering-spool` | 持久化计量 Spool 目录 |

## 边界与非目标

- Monoize 转发工具定义与工具调用，不在本地执行工具。
- 不提供 OpenAI Files、向量存储或本地检索。
- 不实现 Responses 对象存储与后续按 ID 检索。
- 下游开始接收字节后不再回退，禁止流中途切换 Provider。
- 跨协议族转换保留可表示的语义；无安全对应表示的厂商私有嵌套字段会被移除。
- 图像压缩需显式开启；不抓取远程图片 URL，除非单独配置 URL 解析 Transform。

## 规范与文档

可观测行为定义在 [`spec/`](spec/) 下，代码与规范同步变更。完整文档见 [`docs/`](docs/)。

## 许可

Monoize 使用 [MIT License](LICENSE)。
