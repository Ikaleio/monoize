# Monoize URP 全面重构：Vec 数据模型 + 非流式修复 + 流式 Hub-and-Spoke 重写

## 背景

本项目 `github.com/Ikaleio/monoize` 是一个 LLM API 聚合代理。核心架构是 URP（Unified Responses Protocol）：所有上游协议先 decode 成 URP 内部表达，再从 URP encode 成下游协议。这一原则同时适用于非流式和流式响应（spec FP5/FP6）。

本次修改是一次完整重构，涉及 URP 核心数据模型变更、所有编解码器重写、流式路径从 3x3 矩阵转为 Hub-and-Spoke 架构。

---

## 一、URP 核心数据模型（完整定义）

文件：`src/urp/mod.rs`

### 1.1 UrpResponse

旧版使用单个 `message: Message` 包含扁平的 `parts: Vec<Part>`。新版改为 `outputs: Vec<Message>`，每个 Message 自带 role 和 extra_body。

```text
UrpResponse {
    id: String,
    model: String,
    outputs: Vec<Message>,
    finish_reason: Option<FinishReason>,
    usage: Option<Usage>,
    extra_body: HashMap<String, Value>,
}
```

### 1.2 UrpRequest

请求侧同样使用 `Vec<Message>` 表达输入：

```text
UrpRequest {
    model: String,
    inputs: Vec<Message>,
    tools: Vec<Tool>,
    extra_body: HashMap<String, Value>,
}
```

### 1.3 Message

```text
Message {
    role: Role,
    parts: Vec<Part>,
    extra_body: HashMap<String, Value>,
}
```

设计要点：

- 没有 `phase` 字段。phase 和所有其他 message 级别字段（status、id 等）统一走 `extra_body`。
- decode 时把协议对象上除 role 和 content/tool_calls 等已解析字段外的所有字段都放进 extra_body。
- encode 时把 extra_body merge 回协议对象。phase 自然回来，未来任何新字段也自动保留。
- 一个 Message 内的所有 parts 共享同一个 role。不同 phase 的输出拆成不同 Message。

### 1.4 Part

```text
#[serde(tag = "type")]
enum Part {
    #[serde(rename = "text")]
    Text {
        content: String,
        extra_body: HashMap<String, Value>,
    },
    #[serde(rename = "reasoning")]
    Reasoning {
        content: Option<String>,
        encrypted: Option<String>,
        summary: Option<String>,
        source: Option<String>,
        extra_body: HashMap<String, Value>,
    },
    #[serde(rename = "refusal")]
    Refusal {
        content: String,
        extra_body: HashMap<String, Value>,
    },
    #[serde(rename = "tool_call")]
    ToolCall {
        call_id: String,
        name: String,
        arguments: String,
        extra_body: HashMap<String, Value>,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        call_id: String,
        output: Value,
        extra_body: HashMap<String, Value>,
    },
    #[serde(rename = "image")]
    Image {
        source: ImageSource,
        extra_body: HashMap<String, Value>,
    },
    #[serde(rename = "audio")]
    Audio {
        source: AudioSource,
        extra_body: HashMap<String, Value>,
    },
    #[serde(rename = "file")]
    File {
        source: FileSource,
        extra_body: HashMap<String, Value>,
    },
    #[serde(rename = "provider_item")]
    ProviderItem {
        item_type: String,
        body: Value,
        extra_body: HashMap<String, Value>,
    },
}
```

### 1.5 Reasoning 合并设计

- 旧版有两个独立 variant `Part::Reasoning` 和 `Part::ReasoningEncrypted`。新版合并为一个 `Part::Reasoning`。
- `encrypted` 字段的统一语义是“必须原样回传的不透明数据”。
- `source` 字段记录产生此 reasoning 的模型名。用于 encode 请求时过滤：
  - `source == 当前目标模型` -> 保留此 Reasoning part
  - `source != 当前目标模型` -> 跳过此 Reasoning part
  - `source == None` -> 保留（向后兼容）

### 1.6 ProviderItem

用于 URP 不理解内部语义的协议专属 item（如 `computer_call`、`computer_call_output`、`web_search_call`、`code_interpreter_call`、`mcp_call`）。URP 只负责保序透传。

---

## 二、流式数据模型（完整定义）

### 2.1 UrpStreamEvent

```text
enum UrpStreamEvent {
    ResponseStart { id: String, model: String, extra_body: HashMap<String, Value> },
    MessageStart { message_index: u32, role: Role, extra_body: HashMap<String, Value> },
    PartStart { part_index: u32, message_index: u32, header: PartHeader, extra_body: HashMap<String, Value> },
    Delta { part_index: u32, delta: PartDelta, usage: Option<Usage>, extra_body: HashMap<String, Value> },
    PartDone { part_index: u32, part: Part, usage: Option<Usage>, extra_body: HashMap<String, Value> },
    MessageDone { message_index: u32, message: Message, usage: Option<Usage>, extra_body: HashMap<String, Value> },
    ResponseDone { finish_reason: Option<FinishReason>, usage: Option<Usage>, outputs: Vec<Message>, extra_body: HashMap<String, Value> },
    Error { code: Option<String>, message: String, extra_body: HashMap<String, Value> },
}
```

### 2.2 PartHeader

```text
enum PartHeader {
    Text,
    Reasoning,
    Refusal,
    ToolCall { call_id: String, name: String },
    Image { extra_body: HashMap<String, Value> },
    Audio { extra_body: HashMap<String, Value> },
    File { extra_body: HashMap<String, Value> },
    ProviderItem { item_type: String, body: Value },
}
```

### 2.3 PartDelta

```text
enum PartDelta {
    Text { content: String },
    Reasoning { content: String },
    Refusal { content: String },
    ToolCallArguments { arguments: String },
    Image { source: ImageSource },
    Audio { source: AudioSource },
    File { source: FileSource },
    ProviderItem { data: Value },
}
```

### 2.4 关键设计

- `PartDone.part` 必须携带完整 Part。
- `MessageDone.message` 必须携带完整 Message。
- `ResponseDone.outputs` 是最终权威结果。

### 2.5 ResponseDone.outputs 构建规则

- 如果上游 terminal 事件提供完整结构，直接 decode 完整结构并替换累积结果。
- 如果上游 terminal 事件没有完整结构，则使用 decoder 累积结果。
- 下游 encoder 构建完整响应对象时必须使用 `ResponseDone.outputs`。

### 2.6 状态机规则

- `ResponseStart` 和 `ResponseDone`（或 `Error`）各出现恰好一次。
- 每个 Message 生命周期：`MessageStart -> (PartStart -> Delta* -> PartDone)* -> MessageDone`。
- `message_index` 单调递增，`part_index` 全局单调递增。
- 同一 Message 内允许多个 ToolCall part 并发 open。
- 文本类 Part 在同一 Message 内同一时刻最多一个 open。
- `PartDone.part` / `MessageDone.message` / `ResponseDone.outputs` 都必须是完整最终状态。

---

## 三、非流式要求

### OpenAI Responses

- 每个 output item decode 成一个 Message，保留 output 顺序。
- `message` / `function_call` / `reasoning` / 未识别 item type 都必须有明确映射。
- encode 时必须按 Message/Part 顺序重建 output items；混合 parts 时拆分成多个 items。
- `Reasoning.source` 过滤只在 encode request 生效。

### OpenAI Chat

- reasoning_content / reasoning_details 都要 decode 成合并后的 `Part::Reasoning`。
- encode 时要恢复 `reasoning_content` 与 `reasoning_details`。
- `ProviderItem` 在 Chat 中静默跳过。

### Anthropic

- thinking / redacted_thinking 都 decode 成 `Part::Reasoning`。
- encode 时连续 assistant Messages 合并为一个 Anthropic assistant message。
- phase 通过 `Message.extra_body` 下放到 text block 扩展字段。

---

## 四、流式路径重写为 Hub-and-Spoke

- 新增 upstream decoders：
  - `src/urp/stream_decode/openai_responses.rs`
  - `src/urp/stream_decode/openai_chat.rs`
  - `src/urp/stream_decode/anthropic.rs`
- 新增 downstream encoders：
  - `src/urp/stream_encode/openai_responses.rs`
  - `src/urp/stream_encode/openai_chat.rs`
  - `src/urp/stream_encode/anthropic.rs`
- `src/handlers/streaming.rs` 只保留路由调度和 channel/tokio 任务管理。
- 删除旧的协议到协议直接翻译和 synthetic emit helper。

---

## 五、行为约束

1. 不要把 phase 做成 Message 的 typed 字段。
2. 不要把 Reasoning 再拆成两个 variant。
3. Reasoning.source 过滤只在 encode request 时生效。
4. 不要牺牲任何协议的无损性。
5. extra_body passthrough 必须保留并增强。
6. 遇到未知 event/item/field 不得 panic 或中断流。
7. 不做无关代码风格改动。

---

## 六、回传保证（Round-Trip Invariants）

- Responses -> URP -> Responses：reasoning / phase / provider item 无损。
- Anthropic -> URP -> Anthropic：thinking / signature / redacted_thinking 无损。
- 任意路径 -> URP -> Chat：明文 reasoning 和 encrypted reasoning 都要完整保留到 `reasoning_details`。
- `Reasoning.source` 过滤必须阻止跨模型回传模型专属 opaque reasoning。

---

## 七、验收标准

- `UrpResponse` 使用 `outputs: Vec<Message>`。
- Message 无 typed `phase` 字段。
- `Part` 包含合并后的 `Reasoning` 与 `ProviderItem`。
- `UrpStreamEvent` 包含 `MessageStart` / `MessageDone` / 完整 `PartDone` / `ResponseDone.outputs`。
- 非流式与流式都必须保序、保留 `extra_body`、保留 reasoning opaque 数据。
- `streaming.rs` 不再存在任意协议 SSE 到另一协议 SSE 的直接翻译逻辑。
- 所有现有测试继续通过。

---

## 八、输出要求

完成后需要说明：

1. 修改了哪些文件及各自目的。
2. 旧/新 URP 模型迁移点。
3. 非流式保序与 extra_body 保留策略。
4. 流式 Hub-and-Spoke 架构拆分。
5. Reasoning 的 content / encrypted / summary / source 流转。
6. Reasoning.source 过滤实现位置。
7. 并行工具调用缓冲策略。
8. PartDone / MessageDone / ResponseDone 完整结构实现方式。
9. phase 如何经 extra_body 无损流转。
10. 中途错误如何在各 downstream encoder 中表达。
11. 删除了哪些旧代码。
12. 可能尚未覆盖的边界情况。
