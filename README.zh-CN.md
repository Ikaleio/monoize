<div align="center">

<img src="frontend/public/monoize.svg" width="96" alt="Monoize 标志">

# Monoize

**AI API 看起来相似，但协议并不相同。**

Monoize 是一个用 Rust 编写的 AI API 网关。它支持 OpenAI Responses、Chat Completions、Anthropic Messages、Gemini、Embeddings 和图像 API。它转换协议语义。它把一个逻辑模型路由到多个上游 Channel。它处理真实客户端与真实网关之间的兼容问题。

[English](README.md) · [简体中文](README.zh-CN.md)

</div>

## 问题

AI API 网关不能只改几个 JSON 字段名。

Responses、Chat Completions 和 Messages 对对话历史、推理、工具、用量、错误和流事件有不同定义。一次转换即使返回 HTTP 200，也可能破坏对话状态。它可能漏掉加密推理，把增量放进错误的内容块，重复发送流事件，或把工具结果变成助手文本。

路由本身也是一个状态机。网关需要重试失败的 Channel。它需要继续尝试下一个 Provider。它需要在向客户端发出响应字节后停止回落。此后再切换上游，会把两次不同的生成拼进同一条流。

客户端和上游网关还存在各自的边界行为。Claude Code、OpenRouter 兼容客户端、Codex WebSocket 客户端、DeepSeek 工具循环、图像服务和不同的 SSE 实现都带有不同假设。

内联大图会产生另一类开销。上传时间和上游图像预处理可能占据大部分首 Token 时间。如果每次重试都重复转发同一个超大 base64 请求体，这项开销会进一步增加。

## 常见转换器为什么会出错

支持一种格式，不等于正确实现一种协议。以下公开证据已于 2026-08-10 核对：

- OpenAI 用 `encrypted_content` 保存无状态多轮推理所需的状态。在 New API 的 [`823e263`](https://github.com/QuantumNous/new-api/commit/823e26304a396854ace30b52b98ec497c2dd9c36) 提交中，Responses 输出 DTO [无法表示该字段](https://github.com/QuantumNous/new-api/blob/823e26304a396854ace30b52b98ec497c2dd9c36/relaykit/dto/openai_response.go#L327-L339)。它的 Responses 到 Chat 转换器也[只读取可见推理文本](https://github.com/QuantumNous/new-api/blob/823e26304a396854ace30b52b98ec497c2dd9c36/relaykit/relayconvert/internal/oai_responses/to_oai_chat_resp.go#L212-L229)。因此，它在格式转换时仍会漏掉加密推理。加密状态需要随后续输入重放，原因见 [OpenAI 推理指南](https://developers.openai.com/api/docs/guides/reasoning#preserve-reasoning-without-stored-responses)。
- LiteLLM 的 [#32357](https://github.com/BerriAI/litellm/issues/32357) 报告指出，其 Anthropic 适配器会重复发送 `message_start`，并把 `thinking_delta` 放进文本块。事件违反内容块生命周期后，Anthropic SDK 会丢弃这段推理。
- New API 的 [#5480](https://github.com/QuantumNous/new-api/issues/5480) 记录了多条流式转发路径为估算 Token 而保留完整生成文本的问题。内存会随输出长度和并发数增长。

这些不是少写了几个字段别名，而是设计缺陷。Monoize 在协议模型、流状态机、路由规则和资源上限中处理这些问题。

## Monoize 如何解决

### 转换语义，不是替换字段名

Monoize 先把每种受支持的协议解码为 URP v2。URP v2 是一个扁平、强类型的统一表示。文本、推理摘要、原始推理、加密推理、工具调用、工具结果、图像、文件、拒答、用量和控制边界使用不同节点。

选中的上游适配器再把这些节点编码为目标协议。响应按相反方向经过同一条路径。

这个设计提供以下保证：

- Responses、Chat Completions 和 Messages 的完整请求与响应矩阵都覆盖流式和非流式测试。
- 加密推理与可见推理保持分离。可选的 `mz2` 信封可以让不透明推理状态在原本不兼容的重放格式之间保留。
- 工具调用 ID、并行调用、多段工具结果和助手历史不会失去角色。
- Responses 输出项和 Messages 内容块的生命周期保持有序且闭合。
- 同协议的未知字段可以保留。跨协议时，不安全的嵌套字段会被删除，不会泄漏进无效请求。

规范场景及对应测试见[协议测试矩阵](spec/urp-v2-flat-protocol-test-matrix.spec.md)。

### 只在首字节前重试

一个逻辑模型可以匹配多个有序 Provider。每个 Provider 可以包含多个带权重的 Channel。

Monoize 按有界瀑布顺序执行：

1. 选择第一个匹配的 Provider。
2. 根据权重和亲和性选择可用 Channel。
3. 在配置的预算内重试可重试错误。
4. 当前路由耗尽后，继续下一个可用路由。
5. 发出第一个下游响应字节后停止回落。

网络错误、超时、`429` 和指定的 `5xx` 可以触发继续尝试。`400`、`401`、`403` 和 `422` 等客户端错误会停止瀑布。熔断器、被动健康状态、主动探测、冷却时间和模型亲和性会把已知故障 Channel 移出热路径。

Monoize 不会在可见流中途切换 Provider。准确的状态转换见[路由规范](spec/monoize-upstream-routing.spec.md)。

### 在边界处理客户端和网关怪癖

核心适配器负责正常协议转换。有序 Transform 负责只属于某个客户端、Provider、模型或 API Key 的行为。

例如：

- OpenRouter 兼容的结构化推理和末尾用量块；
- DeepSeek 工具循环中的推理历史重放；
- Anthropic thinking 内容块和签名；
- Codex Responses WebSocket 会话和 `/v1/responses/compact`；
- 将 data URL 图像转换为上游原生图像来源；
- 为单行缓冲区较小的客户端拆分 SSE 帧；
- 清理孤立工具调用，修复连续同角色消息；
- 映射 system 和 developer 角色；
- 为系统提示、工具使用和 OpenAI 工具设置缓存断点；
- 删除特定网关头，映射模型后缀和推理 Token 预算。

Transform 可以作用于 Provider、全局或 API Key。模型 glob 决定规则的适用范围。完整行为见 [Transform 规范](spec/urp-transform-system.spec.md)。

### 在请求上游前降低大图开销

`compress_user_message_images` 是一个需要显式启用的请求 Transform。它可以在路由到上游前缩放并重新压缩内联用户图像。输出格式包括 JPEG、PNG、WebP 和 JPEG XL。

Transform 保留图像节点及其 Provider 专用细节参数。它会跳过不支持的来源和普通远程 URL。输入字节数、解码像素数、并发编码数、缓存条目数和缓存总字节数都有明确上限。

它会减小请求体积，并降低图像请求中可避免的 TTFT。缓存还会避免在重试或重复请求中再次执行相同编码。

### 显著降低转发开销

Monoize 在转发热路径上的运行效率显著高于常见 API 转发器。

- Rust 和 Tokio 处理并发 I/O，不需要解释器参与每个请求。
- 正常流式路径通过有界 Channel 增量解码和编码。
- 用量估算随增量更新计数器，不会只为计数而保留完整生成文本。
- 限流键、路由健康状态、亲和性、API Key 缓存、请求捕获、WebSocket 历史、发现响应体和图像转换都有明确上限。
- Release 构建把 React 控制台嵌入可执行文件。一个进程同时提供 API、控制台和指标。

部分响应 Transform 会有意选择缓冲后合成流。Replicate 也使用该路径。默认协议桥接仍是增量式的。

这里比较的是转发器自身的 CPU、内存和延迟开销，不是上游模型的生成速度。实现可见[流式用量统计](src/handlers/usage.rs)和[运行时资源上限](spec/runtime-resource-bounds.spec.md)。

## 支持范围

### 下游端点

| 方法 | 端点 | 协议 |
| --- | --- | --- |
| `GET` | `/v1/models` | OpenAI 兼容模型列表 |
| `POST` | `/v1/responses` | OpenAI Responses，流式或非流式 |
| `GET` | `/v1/responses` | OpenAI Responses WebSocket 传输 |
| `POST` | `/v1/responses/compact` | Responses 压缩上下文 |
| `POST` | `/v1/chat/completions` | OpenAI Chat Completions |
| `POST` | `/v1/messages` | Anthropic Messages |
| `POST` | `/v1/embeddings` | Embeddings |
| `POST` | `/v1/images/generations` | 图像生成 |
| `POST` | `/v1/images/edits` | Multipart 图像编辑 |

所有转发端点也提供 `/api/v1/...` 别名。

### 上游 Channel 类型

| 类型 | 上游原生协议 |
| --- | --- |
| `responses` | OpenAI Responses 兼容协议 |
| `chat_completion` | OpenAI Chat Completions 兼容协议 |
| `messages` | Anthropic Messages 兼容协议 |
| `gemini` | Google Gemini 原生协议 |
| `openai_image` | OpenAI 兼容图像 API |
| `replicate` | Replicate Predictions |

Provider 定义路由顺序、重试预算和健康策略。Channel 保存实际的上游类型、Base URL、凭据、模型映射、权重和超时。

## 请求路径

```text
客户端协议
    │
    ▼
解码为强类型 URP v2
    │
    ▼
Provider 瀑布 ──► 带权 Channel ──► 熔断器 / 亲和性
    │                                    │
    │                         首字节前重试或向后回落
    ▼
Provider、全局和 API Key Transform
    │
    ▼
上游协议编码
    │
    ▼
上游流 ──► URP v2 事件 ──► 下游协议事件
```

## 快速开始

安装稳定版 Rust 工具链和 [Bun](https://bun.sh/)。Release 构建会编译前端并把它嵌入可执行文件。

```bash
cargo build --release
./target/release/monoize
```

打开 `http://localhost:8080`。即使公开注册已被关闭，第一个注册账户仍会成为 `super_admin`。然后：

1. 创建一个 Provider。
2. 添加至少一个 Channel，并填写上游地址和凭据。
3. 把逻辑模型映射到该 Channel。
4. 创建一个 API Key。

### Docker

发布镜像支持 Linux x86-64 和 ARM64。使用持久化 SQLite 数据卷启动：

```bash
docker run -d \
  --name monoize \
  --restart unless-stopped \
  -p 8080:8080 \
  -v monoize-data:/app/data \
  ghcr.io/ikaleio/monoize:latest
```

如需使用 PostgreSQL 或非默认 SQLite 路径，请通过 `-e` 设置 `MONOIZE_DATABASE_DSN`。

通过任意受支持的下游协议调用这个逻辑模型：

```bash
curl http://localhost:8080/v1/responses \
  -H 'Authorization: Bearer sk-your-monoize-key' \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "your-logical-model",
    "input": "解释为什么流式回落必须在首字节后停止。",
    "stream": true
  }'
```

## 配置

运行时引导使用环境变量。Provider、Channel、模型、路由策略、Transform、用户和 API Key 保存在数据库中。控制台负责管理这些配置。

| 变量 | 默认值 | 用途 |
| --- | --- | --- |
| `MONOIZE_LISTEN` | `0.0.0.0:8080` | HTTP 监听地址 |
| `MONOIZE_DATABASE_DSN` | `sqlite://./data/monoize.db` | SQLite 或 PostgreSQL DSN |
| `DATABASE_URL` | 未设置 | `MONOIZE_DATABASE_DSN` 未设置时的后备 DSN |
| `MONOIZE_METRICS_PATH` | `/metrics` | Prometheus 指标路径 |
| `MONOIZE_HTTP_BODY_MAX_BYTES` | `52428800` | 转发请求体上限 |
| `MONOIZE_TRUSTED_PROXY_CIDRS` | 空 | 受信任的反向代理网段 |
| `MONOIZE_UPSTREAM_PROXY_URL` | 未设置 | 本节点的上游出站 HTTP(S) 代理；Channel 可通过 `proxy_url` 单独覆盖 |
| `MONOIZE_CAP_API_ENDPOINT` | 未设置 | 可选的外部 Cap 公共站点端点（包含 site key 路径）；未设置时使用 Monoize 内置 Cap |
| `MONOIZE_CAP_SECRET_KEY` | 未设置 | 外部 Cap 站点的 secret key；必须与 `MONOIZE_CAP_API_ENDPOINT` 同时配置 |

仪表盘登录和注册默认使用 Monoize 内置的 Cap 工作量证明服务，无需额外配置。管理员可在系统设置中关闭人机验证；关闭后，登录和注册将失去机器人与撞库攻击防护。如需改用 [Cap Standalone](https://capjs.js.org/zh/guide/)，请创建 site key、同时设置上述两个变量，并在 Cap 的 CORS 配置中允许仪表盘来源。Monoize 随后通过该站点的 `/siteverify` 端点验证令牌。

Monoize 支持 SQLite 和 PostgreSQL。业务表只支持由一个 Monoize 应用进程写入。

### 主从部署

Monoize 支持一个可写主机加若干只读从机的部署形态。所有节点共享同一个 PostgreSQL 数据库（见 `spec/primary-replica-deployment.spec.md`）。从机只服务 `/v1/**` 转发流量，不提供控制台。从机通过带鉴权的内部接口，把请求日志和计费扣减上报主机落库。余额预检会扣除尚未上报的本地欠账，以约束超支。故障切换为手动操作：把从机角色改为主机并重启，即可完成提升。

| 变量 | 默认值 | 用途 |
| --- | --- | --- |
| `MONOIZE_NODE_ROLE` | `primary` | `primary` 或 `replica` |
| `MONOIZE_PRIMARY_INTERNAL_URL` | 从机必填 | 主机内部地址，用于计量上报 |
| `MONOIZE_REPLICA_TOKEN` | 未设置 | 节点共享密钥：从机必填；主机设置后开启接收端点 |
| `MONOIZE_REPLICA_ID` | 自动生成并持久化 | 从机固定标识（UUID v4）。未设置时首次启动自动生成，并持久化到计量外存目录下的 `replica-identity` 文件，重启后保持不变 |
| `MONOIZE_CONFIG_POLL_INTERVAL_SECONDS` | `5` | 从机配置纪元轮询间隔 |
| `MONOIZE_METERING_SHIP_INTERVAL_SECONDS` | `10` | 从机计量上报间隔 |
| `MONOIZE_METERING_SHIP_BATCH_MAX_ENTRIES` | `500` | 单批次条目上限（硬上限 2000） |
| `MONOIZE_REPLICA_METERING_SPOOL_DIR` | `./data/replica-metering-spool` | 计量差额外存目录 |

## 运维能力

内嵌控制台可以管理：

- Provider、Channel、健康状态、优先级、模型映射和价格倍率；
- API Key、配额、模型限制、IP 白名单、Transform 和子账户；
- 用户、余额、nano-dollar 精度计费和只追加账本；
- 包含 TTFB、总耗时、Token、费用、错误和已尝试路由的请求日志；
- 从 [Models.dev](https://models.dev) 导入的模型元数据和价格；
- Prometheus 指标和实时运维视图。

请求捕获需要显式启用，并且有资源上限。正常可观测日志不会记录凭据和提示词正文。

## 限制与非目标

- Monoize 转发工具定义和工具调用，但不在本地执行工具。
- Monoize 不提供 OpenAI Files、Vector Stores 或本地检索。
- 当前不实现 Responses 对象存储和后续对象读取。
- 下游开始接收字节后，回落结束。系统明确禁止流中途切换 Provider。
- 跨协议转换保留目标协议可以表示的语义。没有安全目标表示的 Provider 专用嵌套字段会被删除。
- 图像压缩需要显式启用。除非配置独立的 URL 解析 Transform，否则它不会抓取任意远程图像。

## Release 构建产物

发布 GitHub Release 时，如果标签等于 `v` 加 Cargo 包版本，[Release 工作流](.github/workflows/release.yml)会自动运行。它为 Linux、macOS 和 Windows 分别构建原生 x86-64 与 ARM64 二进制文件。

Linux 和 macOS 使用 `tar.gz`。Windows 使用 `zip`。每个压缩包都包含中英文 README 和许可证。每个压缩包都带有独立的 SHA-256 文件。六个平台全部构建成功且校验通过后，工作流才会上传文件。

手动运行工作流可以执行相同的六平台预检。它不会修改 GitHub Release。准确的构建产物约束见 [Release Artifact 规范](spec/release-artifacts.spec.md)。

## 开发与验证

运行后端测试：

```bash
cargo test
```

检查前端：

```bash
cd frontend
bun install
bun run lint
bun run build
```

对已配置实例运行三协议实时测试：

```bash
cd sdk-tests
bun run live-protocol-suite.ts <baseURL> <apiKey> <model>
```

该测试覆盖 Chat Completions、Responses 和 Messages 的非流式文本、流式文本、工具循环和流式工具循环。

所有可观察行为都在 [`spec/`](spec/) 中定义。代码和规范必须同步修改。

## 许可证

Monoize 使用 [MIT License](LICENSE)。
