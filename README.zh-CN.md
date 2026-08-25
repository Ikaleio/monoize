<div align="center">

<img src="frontend/public/monoize.svg" width="96" alt="Monoize 标志">

# Monoize

**AI API 接口形式相似，但底层协议约定各不相同。**

Monoize 是基于 Rust 开发的 AI API 网关，支持 OpenAI Responses、Chat Completions、Anthropic Messages、Gemini、Embeddings 与图像 API。它在网关层完成协议语义转换，将单个逻辑模型分发至多个上游 Channel，并统一处理客户端与上游之间的兼容和容灾问题。
[English](README.md) · [简体中文](README.zh-CN.md)
</div>

## 问题背景

AI API 网关需要解决的不仅是 JSON 字段映射。

Responses、Chat Completions 与 Messages 对对话历史、推理过程、工具调用、Token 计量、错误状态及流式事件的数据建模各不相同。简单的字段重命名即便返回 HTTP 200，也可能破坏会话状态：例如丢失加密推理上下文、将流式增量追加到错误的内容块、重复发送生命周期事件，或将工具执行结果误转为助手文本。

多上游路由同样依赖严格的状态机。网关需要自动重试偶发失败、按序回退到备用 Provider，并在向下游客户端发送首个响应字节后锁定当前连接。若在数据下发后切换上游，会导致两次不同的生成内容拼接进同一条流。

此外，客户端与上游网关存在各种边界差异。Claude Code、OpenRouter 兼容客户端、Codex WebSocket 客户端、DeepSeek 工具循环、图像服务以及各厂商的 SSE 实现，都有各自的协议假设。

直接传输内联图像会增加延迟。Base64 上传与上游图像预处理耗时会增加首字时间（TTFT），如果每次重试都原样转发未优化的请求体，开销还会成倍叠加。
## 常见转换器的典型缺陷

支持某类数据格式，不等于正确实现了对应协议。以下公开问题已于 2026-08-10 核对验证：

- OpenAI 使用 `encrypted_content` 保存无状态多轮对话所需的推理状态。在 New API 提交 [`823e263`](https://github.com/QuantumNous/new-api/commit/823e26304a396854ace30b52b98ec497c2dd9c36) 中，Responses 输出 DTO [无法表示该字段](https://github.com/QuantumNous/new-api/blob/823e26304a396854ace30b52b98ec497c2dd9c36/relaykit/dto/openai_response.go#L327-L339)，且 Responses 到 Chat 转换器[仅提取明文推理文本](https://github.com/QuantumNous/new-api/blob/823e26304a396854ace30b52b98ec497c2dd9c36/relaykit/relayconvert/internal/oai_responses/to_oai_chat_resp.go#L212-L229)，导致加密推理数据在转换中静默丢失。具体机制参见 [OpenAI 推理开发指南](https://developers.openai.com/api/docs/guides/reasoning#preserve-reasoning-without-stored-responses)。
- LiteLLM 问题 [#32357](https://github.com/BerriAI/litellm/issues/32357) 显示其 Anthropic 适配器会重复触发 `message_start`，并在文本块中下发 `thinking_delta`。由于违反了内容块生命周期规范，Anthropic 官方 SDK 会直接丢弃这部分推理输出。
- New API 问题 [#5480](https://github.com/QuantumNous/new-api/issues/5480) 记录了流式转发路径为估算 Token 而在内存中全量保留完整生成文本的问题，导致网关内存占用随生成长度与并发连接数线性膨胀。

Monoize 从协议模型、流状态机、路由规则和资源上限等设计上处理上述问题。
## Monoize 如何解决

### 协议语义转换

Monoize 将接入的协议统一解码为 URP v2 规范表示。URP v2 采用扁平且强类型的结构，将普通文本、推理摘要、原始推理、加密推理、工具调用、工具返回值、图像、文件、拒答信息、用量数据与流控制边界分别表示为独立的类型化节点。

选定的上游适配器将这些节点编码为目标上游格式；上游响应则按相反流程转换后返回给客户端。

该设计提供以下特性：

- Responses、Chat Completions 与 Messages 之间的双向转换均覆盖流式与非流式测试用例。
- 加密推理与明文推理隔离，可选的 `mz2` 信封机制可在跨格式重放时保留不透明推理状态。
- 工具调用 ID、并行调用、多段工具结果及助手历史维持原始角色与层级结构。
- Responses 输出项与 Anthropic 内容块生命周期保持有序开启与闭合。
- 同协议族内的未知字段正常透传；跨协议族转换时自动剥离无对应表示的嵌套字段，避免触发上游 400 参数校验错误。

规范场景及对应测试用例见[协议测试矩阵](spec/urp-v2-flat-protocol-test-matrix.spec.md)。
### 首字节前重试与故障转移

一个逻辑模型可配置多个按优先级排序的 Provider，每个 Provider 包含若干带权重的 Channel。

Monoize 按照有界瀑布策略执行路由调度：

1. 匹配当前逻辑模型优先级最高的 Provider。
2. 根据权重与会话亲和性选择健康的 Channel。
3. 在配置的预算内自动重试偶发故障。
4. 当前 Channel 耗尽重试后，自动推进至下一个可用路由。
5. 向客户端发送首个响应字节后，立即锁定当前路由并停止回退。

网络断开、请求超时、HTTP `429` 以及指定的 `5xx` 错误会触发路由推进；`400`、`401`、`403`、`422` 等客户端错误则直接返回，不触发故障切换。熔断器、被动健康检测、主动健康探测、冷却期机制与模型亲和性可确保异常通道迅速脱离热路径。

Monoize 严禁在流式响应中途切换上游 Provider。详细状态转换规则请参见[路由规范](spec/monoize-upstream-routing.spec.md)。
### Transform 边界适配

核心适配器负责通用协议转换，Transform 流水线则用于处理特定客户端、Provider、模型或 API Key 的专有行为。

常见 Transform 场景包括：

- 提取 OpenRouter 结构化推理格式与末尾用量块。
- 在 DeepSeek 工具调用循环中重放历史推理上下文。
- 处理 Anthropic thinking 内容块与签名生命周期。
- 适配 Codex Responses WebSocket 传输与 `/v1/responses/compact` 上下文压缩。
- 将 data URL 图像载荷转为上游原生支持的图像来源。
- 为行缓冲区较小的客户端拆分超长 SSE 数据帧。
- 清理孤立工具调用并自动合并连续同角色消息。
- 映射 `system` 与 `developer` 角色差异。
- 为系统提示词、工具定义与 OpenAI 工具链自动插入缓存断点（Prompt Cache）。
- 剥离厂商私有请求头、处理模型后缀与推理 Token 预算映射。
Transform 支持在 Provider、API Key 或全局级别挂载，并通过模型匹配通配符指定生效范围。完整规则见 [Transform 规范](spec/urp-transform-system.spec.md)。
### 请求图像优化

`compress_user_message_images` 是一个可选开启的请求 Transform，可在向请求上游转发前，自动缩放并重新压缩用户消息中的内联图像。支持输出 JPEG、PNG、WebP 及 JPEG XL 格式。

该 Transform 会完整保留图像节点与厂商专有清晰度参数，并跳过普通远程 URL 和不支持的格式。输入大小、解码像素、并发编码线程数、缓存条目与内存占用均受严格限制。

该优化可减小请求体积，缩短内联大图场景下的首字时间（TTFT）。内置缓存还可避免重试或重复请求时的二次编码开销。
### 低开销代理转发

Monoize 专注于降低网关本身的系统开销：

- 基于 Rust 与 Tokio 实现原生异步 I/O，请求热路径无解释器开销与垃圾回收停顿。
- 默认流式链路通过有界异步通道对数据块进行增量解码与编码。
- 随流式数据块到达即时累加 Token 计数，无需在内存中全量缓冲响应正文。
- 限流键、健康状态、亲和性表、API Key 缓存、抓包缓冲区、WebSocket 历史与图像转换均有严格的内存上限。
- Release 构建将编译好的 React 控制台直接内嵌至可执行文件中，单进程同时提供 API 代理、管理后台与 Prometheus 指标。

部分 Transform 在需要整段重构响应时会选择缓冲后合成流，Replicate 同样使用该路径；默认协议桥接均保持增量流式转发。

这里比较的是代理自身的 CPU、内存和延迟开销，并不代表上游模型生成速度会变快。实现细节见[流式用量统计](src/handlers/usage.rs)和[运行时资源上限](spec/runtime-resource-bounds.spec.md)。
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

使用 Bun 单次运行 Monoize：

```bash
bunx monoize
```

也可以全局安装：

```bash
bun add --global monoize
monoize
```

同一个包也兼容 npm 和 pnpm：

```bash
npx monoize
# 或：pnpm dlx monoize
# 全局安装：npm install --global monoize
# 全局安装：pnpm add --global monoize
```

包管理器只会安装与当前操作系统和 CPU 匹配的原生二进制文件。npm 包支持基于 GNU libc 或 musl 的 Linux，以及 macOS 和 Windows 的 x86-64 与 ARM64 环境。Linux 包使用静态 musl 可执行文件，不依赖宿主机的 libc 或 `libstdc++`。

如需从源码构建，请安装稳定版 Rust 工具链和 [Bun](https://bun.sh/)。Release 构建会编译前端并把它嵌入可执行文件。

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

该工作流还会构建七个 npm 压缩包：一个由 TypeScript 构建的启动器，以及六个平台包。Bun、npm 或 pnpm 正常安装时，会根据 `os` 和 `cpu` 元数据选择一个平台包。npm 发布任务通过 npm Trusted Publishing 和 GitHub Actions OIDC 完成认证，不使用长期有效的 npm token。准确的 npm 约束见 [npm CLI 分发规范](spec/npm-cli-distribution.spec.md)。

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
