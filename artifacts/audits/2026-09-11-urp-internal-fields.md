# URP 特殊内部字段审计

日期：2026-09-11。基线：`16f8920` / `v1.8.4`。

## 结论与范围

存在内部字段滥用。主要问题是公共语义未进入类型字段，以及原生重放副本覆盖变换后的类型字段。

本次扫描整个 `src/`，追踪请求解码、响应解码、流式解码、变换、编码和运行上下文。
扫描得到 44 个不同的 `_monoize_*` / `__monoize_*` 字符串标识。
其中 40 个用于协议元数据或运行上下文，4 个是账号、JavaScript 宿主函数名或图片节点 ID。
另检查了 3 个没有保留前缀的内部字段：`openwebui_reasoning_content`、`inject_reasoning_content`、`reasoning_item_id`。
文末清单覆盖这 47 项。计数是源码字符串标识数，不是运行时对象数量。
同目录的 `2026-09-11-urp-internal-fields.json` 列出每项的字符串位置、常量符号和符号引用位置，便于逐项追踪。

初始审计为只读；随后按用户要求完成本地修复，见下方修复记录。未新增测试，未部署代码。
生产环境只完成此前明确授权的 GPT 概要全局变换配置。
没有检查其他服务器上的私有补丁或数据库中的自定义 JavaScript 变换。

## 判定标准

1. `ReasoningConfig.summary` 应表示请求概要的模式。它不应与响应概要正文使用同一个概念。
2. `Node::Reasoning.summary` 应保存返回的概要字符串。
3. `Node::Reasoning.content` 应保存供应方实际返回的原始 CoT 字符串。
4. `NodeDelta::Reasoning` 应保留相同的 summary/content/encrypted 区分。
5. 来源、原生字段名、原生数组边界和不透明重放数据可以保留，但不能覆盖变换后的公共语义。
6. 运行上下文和输出兼容选项可以存在于独立上下文中；它们不应冒充供应方透传字段。
7. 已删除的类型字段不能由旧重放副本恢复。

第 1 项采用用户本次明确要求。现有 `ReasoningConfig` 只有 `effort` 和扁平 `extra_body`。
第 2 至第 4 项主要已有类型字段；问题并非所有 CoT 都存放在私有字段中。

现有规格存在需要修订的冲突：

- `spec/urp-v2-flat-structure.spec.md` 的 ORD-8 要求类型字段优先。
- 同文件 XTRA-10 又将 Chat 原生 reasoning detail 定义为 authoritative replay data。
- `spec/urp-transform-system.spec.md` 的 PRTS-4 至 PRTS-8 要求移动 content 到 summary，并保留 extra_body。
- RSRC-4 至 RSRC-7 明确要求使用 `openwebui_reasoning_content` 控制编码。

这些规则组合后允许旧副本恢复原始文本。不能只删除几个字段；应先统一规格中的权威数据来源。

## 已确认的源码问题

### F1 · P1 · 概要请求模式没有规范化为 URP 类型字段

入口和出口：

- `src/urp/mod.rs:890`：`ReasoningConfig` 没有 `summary` 类型字段。
- `src/urp/decode/openai_responses.rs:376`：Responses summary 进入普通 reasoning extra_body。
- `src/urp/decode/openai_chat.rs:843`：Chat reasoning 对象整体进入 `_monoize_chat_reasoning_config`。
- `src/urp/encode/openai_chat.rs:395`：只读取 Chat 私有副本和类型化 effort，不映射普通 extra_body.summary。
- `src/urp/encode/openai_responses/tool_call.inc.rs:73`：合并普通 extra_body，但不提取 Chat 私有副本。
- `src/urp/encode/mod.rs:16`：过滤 `_monoize_*` 字段。

具体结果：

| 输入 | 目标 | 当前结果 |
| --- | --- | --- |
| Responses `reasoning={effort:high,summary:detailed}` | Chat | summary 丢失 |
| Chat `reasoning={effort:high,summary:detailed}` | Responses | summary 丢失 |
| Responses 同协议转发 | Responses | 普通 extra_body.summary 可以保留 |

整改：添加类型化概要请求模式，所有相关解码器和编码器读取同一个类型字段。
保留未知配置字段时，只保留尚未建模的部分。
`field_set` 和自定义变换也必须写入该类型字段。

### F2 · P1 · Responses 原生推理数组覆盖变换后的概要与 CoT

位置：`src/urp/decode/openai_responses.rs:781`；`src/urp/encode/openai_responses/reasoning.inc.rs:48`。

解码器同时保存类型化 content/summary 和完整原生 content/summary 数组。
编码器优先使用 `_monoize_responses_reasoning_summary` 与 `_monoize_responses_reasoning_content`。

具体路径：输入原生 reasoning content=`A`、summary=`[]`，应用 `reasoning_content_to_summary`。
变换后的 URP 是 content 缺省、summary=`A`。
重新编码 Responses 时，旧 summary=`[]` 和旧 content=`A` 仍被输出。

`src/transforms/reasoning_content_to_summary.rs:158` 修改类型字段，但不刷新原生副本。
因此变换在同协议输出上可以失效。

整改：类型字符串必须决定文本。原生数组只能提供分段与未知字段信息。
文本改变后，应重建数组或使旧数组失效。空值和字段删除也必须参与一致性判断。

### F3 · P1 · Chat 原生 reasoning detail 可绕过类型字段和变换

位置：

- `src/urp/stream_encode/openai_chat.rs:257`：合成流直接发送原生 detail，然后 continue。
- `src/urp/stream_encode/openai_chat.rs:1278`：实际流直接发送原生 detail，然后 return。
- `src/urp/encode/openai_chat.rs:1035`：非流输出克隆原生 detail，只覆盖当前存在且与原始 type 匹配的字段。

具体路径：原生 `reasoning.text` detail 含 text=`A`。
`reasoning_content_to_summary` 把类型化 content 移到 summary。
流式编码仍发送旧 `reasoning.text`；非流编码也不会删除旧 text 或改成 summary detail。

因此原始 CoT 虽已进入 URP，最终输出仍可能由旧副本决定。

整改：根据类型字段生成语义字段和 detail 类型，再合并未知字段。
不得用早返回绕过类型字段。保留重复项和顺序的需求仍然有效。

### F4 · P1 · 原生 instructions 覆盖已修改或已删除的提示词节点

位置：`src/urp/decode/openai_responses.rs:369`、`:415`；`src/urp/encode/openai_responses/tool_call.inc.rs:20`。

解码器把 instructions 转成普通节点，同时保存 `_monoize_responses_instructions` 原值。
编码器只要发现原值，就过滤所有带 `_monoize_responses_instruction_node` 的节点，重新发送原值。

具体路径：Responses instructions 包含 `x-anthropic-billing-header` 行。
`prompt_strip_anthropic_billing_header` 修改或删除对应 URP 节点，但没有修改原生 instructions 副本。
Responses 编码随后恢复原始 instructions；即使节点已经被删除，也会恢复原文。

变换位置：`src/transforms/prompt_strip_anthropic_billing_header.rs:93`。
这不是只影响推理的局部问题，而是另一个公共语义存在双重权威来源的例子。

整改：从变换后的节点构建 instructions。来源标记只能决定可选的原生位置或形状，不能提供旧正文。

### F5 · P2 · 输出兼容控制使用非保留字段，并复制推理正文

位置：

- `src/transforms/reasoning_summary_to_raw_cot.rs:117` 写入 `openwebui_reasoning_content`。
- `src/transforms/reasoning_inject_content_field.rs:131` 写入 `inject_reasoning_content` 文本副本。
- `src/urp/encode/openai_responses/reasoning.inc.rs:79` 透传所有非 `_monoize_` 的节点 extra_body 字段。
- `src/urp/decode/mod.rs:19` 只把 `_monoize_` 前缀识别为保留内部字段。

在启用这些变换且输出 Responses 时，内部选项可以作为未知字段进入 reasoning wire object。
`inject_reasoning_content` 还建立第二份文本；后续修改 summary/content 不会自动修改该副本。
供应方的同名非保留字段也没有独立的可信来源标识。

整改：用独立的类型化输出选项控制兼容别名，并在编码时读取当前 URP 文本。
不要把用户可提供的透传字段直接作为内部控制开关。
不能把实际概要改标成原始 CoT；兼容展示与文本语义需要分开。

### F6 · P2 · 推理启停与预算仍依赖原生配置包

位置：`src/urp/decode/openai_chat.rs:843`；`src/urp/decode/anthropic.rs:88`；`src/urp/encode/anthropic/reasoning.inc.rs:407`。

Chat `thinking={type:disabled}` 没有被 `extract_reasoning` 提取为类型化启停状态。
在没有 reasoning_effort 时，Chat → Responses 路径会忽略这一私有配置包。

Messages 的预算先被粗粒度换算为 effort，精确预算只存在 `_monoize_messages_thinking_config`。
Messages 编码器一旦看到原生 thinking/output_config，就跳过根据类型化 effort 重建这些控制。
因此在解码后改变 effort 的变换，可能无法改变同 Messages 输出的实际推理控制。
原生优先的部分行为是当前规格明确允许的设计；它与本次要求的公共语义归属不一致。

整改：明确表示 enabled/disabled、effort、budget 和 summary 请求模式。
定义原生配置与变换后类型字段冲突时的唯一优先级。
不要把有信息损失的预算到 effort 换算当作精确保留。

## 需要迁移或限制的设计项

以下是类型模型缺口或边界风险，不能全部表述为已在线复现的故障。

| 项目 | 判断与建议 |
| --- | --- |
| 推理 kind、presentation-only、summary 来源标记 | 会改变重放资格或流式内容选择。应成为类型化语义/展示元数据，而不是任意 JSON 键。不能简单删除，否则会破坏密文重放和 Messages 展示。 |
| `reasoning_item_id`、envelope item id | 在节点 ID 之外提供晚到 ID 或封装来源。应使用明确的节点身份更新/传输元数据，避免多个字符串键互相覆盖。 |
| Messages 起始 Usage | 已有 `Usage` 类型，却序列化进 ResponseStart.extra_body，再反序列化。宜让开始事件明确承载起始 usage。 |
| Messages citation delta | 引用本身只放在 extra_body，delta 用空 Text 事件加私有键表示。宜明确建模注释/引用和它们的增量。未据此断言所有跨协议引用都应可映射。 |
| Gemini 普通文本 thoughtSignature | `decode/gemini.rs:523` 对 thought=true 使用类型化 encrypted；普通文本分支把 thoughtSignature 留在 `_monoize_gemini_part`。同 Gemini 可以重放，跨协议没有类型化承载。应建模签名与被签名节点的关联，不能粗暴前置成任意推理节点。 |
| Gemini functionResponse 形状 | `_monoize_gemini_function_response` 决定是否把 ToolResult 文本解析回 JSON 对象。当前属于重放形状信息，不是原始 CoT；未来若支持结构化工具结果，应由明确的内容类型表示。 |
| Responses 整体快照 | `_monoize_responses_response_source` 保留整个响应，随后覆盖部分类型字段，但 status 仍可以来自原始副本/普通 extra_body。自定义变换修改 finish_reason 后有不一致风险。保留字段缺省形状不应要求保存完整的第二份语义对象。 |
| Chat message audio | 被存为带来源的 ProviderItem，而不是 AudioSource。它可能含供应方音频句柄等无法直接转换的信息；不能仅凭 audio 名称认定滥用。可转换媒体应提取，供应方句柄仍需限制重放来源。 |

## 原始 CoT 的现状

| 输入形式 | 当前 URP 落点 | 结论 |
| --- | --- | --- |
| Chat `reasoning_content` / 标量 `reasoning` | `Node::Reasoning.content`，附原生字段名标记 | 字符串已经类型化；标记本身不是藏匿正文。 |
| Chat `reasoning_details[type=reasoning.text].text` | `Node::Reasoning.content` + 原生 detail 副本 | 类型化入口存在；F3 是出口权威来源错误。 |
| Chat `reasoning_details[type=reasoning.summary].summary` | `Node::Reasoning.summary` + 原生 detail 副本 | 同上。 |
| Responses reasoning content / summary | `content` / `summary` + 原生数组副本 | 类型化入口存在；F2 是副本覆盖问题。 |
| Gemini `thought=true,text` | `Node::Reasoning.content`；thoughtSignature 进入 encrypted | 正文落点符合原始 CoT 语义。 |
| Anthropic Messages `thinking` | `Node::Reasoning.summary` | 当前 PM5 明确规定该语义。不能把所有 Messages thinking 自动认定为原始 CoT。 |

## 完整字段清单

分类：**修复**表示上文确认的问题；**类型化**表示语义/事件/输出选项应获得明确模型；**保留**表示存在合理重放或来源用途；**上下文**表示不属于供应方语义；**非字段**表示扫描到的其他标识。
“保留”不等于无限制重放。所有重放必须服从变换后的类型字段、来源限制和内部字段过滤。

| 标识 | 分类 | 实际用途与审计判断 |
| --- | --- | --- |
| `_monoize_chat_choice_extra` | 保留 | choice 层未知字段及流事件位置，不能与 delta 层合并。 |
| `_monoize_chat_delta_extra` | 保留 | 原生 delta 层未知字段，保留封装层级。 |
| `_monoize_chat_error_event` | 保留 | 重放原生错误形状；不是推理正文。 |
| `_monoize_chat_native_finish_reason` | 保留 | 为 `FinishReason::Other` 保留原生枚举值。当前 Chat 编码并非无条件覆盖类型化 finish_reason。 |
| `_monoize_chat_legacy_function_definition` | 保留 | 标识旧 functions 形状；函数参数和名字已有类型字段。 |
| `_monoize_chat_legacy_function_choice` | 保留 | 旧 function_call 选择形状；编码时重写类型化名字。 |
| `_monoize_chat_legacy_function_call` | 保留 | 旧函数调用格式标记；调用语义已有类型字段。 |
| `_monoize_chat_legacy_function_result` | 保留 | 旧函数结果消息格式标记。 |
| `_monoize_chat_message_audio` | 保留/拆分 | 标识完整 message.audio ProviderItem；可转换内容不应永久不透明。 |
| `_monoize_chat_message_item` | 保留 | 完整 Chat configuration_update 消息封装。 |
| `_monoize_chat_reasoning_config` | 修复 F1 | 不能包住公共 summary 等请求控制。 |
| `_monoize_chat_thinking_config` | 修复 F6 | 不能只在原生包中保存启停与预算。 |
| `_monoize_chat_reasoning_detail` | 修复 F3 | 可保留 detail 未知字段及顺序；不能作为覆盖类型文本的权威副本。 |
| `_monoize_chat_reasoning_surface` | 保留 | 记录 reasoning / reasoning_content 原生入口。字符串本体已在 content 中。 |
| `_monoize_file_id_origin` | 保留 | 防止跨供应方错误重放文件 ID；不是可见内容。 |
| `_monoize_gemini_function_response` | 保留/拆分 | 结构化工具结果原生形状。 |
| `_monoize_gemini_part` | 保留/拆分 | Gemini Part 未知字段；普通文本 thoughtSignature 是已识别的类型模型缺口。 |
| `_monoize_messages_chat_reasoning_detail_type` | 上下文 | Messages 编码器将相邻 Chat plaintext/encrypted detail 合并成一个输出块时使用。 |
| `_monoize_messages_citation_delta` | 类型化 | 引用增量应获得明确事件表达。 |
| `_monoize_messages_output_config` | 修复 F6/拆分 | effort 和已识别 format 不应只有原生权威副本；未知选项可重放。 |
| `_monoize_messages_provider_item_start_body` | 保留 | 不透明供应方块的 start payload，受协议来源限制。 |
| `_monoize_messages_stream_start_usage` | 类型化 | 用字符串键搬运已有 Usage 类型。 |
| `_monoize_messages_thinking_config` | 修复 F6 | 精确预算与启停应类型化。 |
| `_monoize_reasoning_downstream_only_presentation` | 类型化 | 决定推理是否允许向上游重放，属于重放/展示策略。 |
| `_monoize_reasoning_envelope_item_id` | 类型化 | 传输封装中的原始 item id。 |
| `_monoize_reasoning_kind` | 类型化 | 区分 redacted_thinking，不能作为任意 JSON 标志处理。 |
| `_monoize_response_history` | 上下文 | 请求历史缓存的关联上下文，实际历史节点仍是 URP。 |
| `_monoize_responses_custom_messages_bridge` | 上下文 | custom tool 转 Messages function 的桥接标记。 |
| `_monoize_tool_namespace_bridge` | 上下文 | namespaced tool 的别名反向映射；类型语义与适配状态应分开。 |
| `_monoize_responses_image_generation_call` | 保留/限制 | 原生图片生成项元数据。编码已用类型化图像数据替换 result；其他与媒体类型相关字段仍需保持一致。 |
| `_monoize_responses_instruction_node` | 修复 F4 | 来源标记可以保留，但不能据此丢弃变换后的节点。 |
| `_monoize_responses_instructions` | 修复 F4 | 原始 instructions 副本覆盖公共提示词节点。 |
| `_monoize_responses_reasoning_content` | 修复 F2 | 原生数组覆盖类型化 CoT。 |
| `_monoize_responses_reasoning_summary` | 修复 F2 | 原生数组覆盖类型化概要。 |
| `_monoize_responses_response_source` | 保留/限制 | 仅为缺省字段形状保存完整响应副本，范围过大；不得覆盖类型化状态。 |
| `_monoize_responses_stream_start_source` | 保留/限制 | 起始响应 envelope 的原生形状；应只保留未类型化的形状信息。 |
| `_monoize_summary_from_messages_thinking` | 类型化 | Messages summary delta 的来源/展示选择。 |
| `_monoize_summary_from_plaintext_reasoning` | 类型化 | 变换后的文本来源/展示选择。 |
| `__monoize_username` | 上下文 | 注入缓存变换身份信息；正常 handler 在发送上游前删除。宜放 TransformRuntimeContext。 |
| `__monoize_api_key_id` | 上下文 | 同上，缓存身份粒度。 |
| `openwebui_reasoning_content` | 修复 F5 | 无保留前缀的内部输出选项。 |
| `inject_reasoning_content` | 修复 F5 | 无保留前缀的第二份推理文本。 |
| `reasoning_item_id` | 类型化 | 流式晚到的 reasoning item id，不应混同未知供应方字段。 |
| `_monoize_active_probe` | 非字段 | 内部主动探测账号名称。 |
| `__monoize_host_fetch` | 非字段 | JavaScript sandbox 临时宿主函数名。 |
| `__monoize_host_log` | 非字段 | JavaScript sandbox 临时宿主函数名。 |
| `__monoize_image_api_mask` | 非字段 | Image API mask 节点的保留 ID；不是 extra_body 字段。 |

## 修复顺序与验收条件

1. 先统一规格：请求配置、返回正文、传输元数据、原生重放信息各有唯一所有者。
2. 修复 F1，让 Requests 的 summary 模式成为类型字段，并更新 `field_set` 的字段访问。
3. 修复 F2/F3/F4，移除旧语义副本的优先权。保留必要的顺序、分段和未知字段。
4. 将输出选项、Usage 起始状态、引用和身份更新从任意 extra_body 键迁出。
5. 对确有协议局限的信息保留来源限制，不伪造目标协议不支持的能力。

验收应覆盖：

- summary 模式通过 Responses → Chat → Responses 后不变。
- 返回 summary/content/encrypted 彼此独立；缺省、空串和删除均不会被旧副本恢复。
- content_to_summary 在流式、非流式和合成流上产生相同的终态语义。
- 修改或删除 instructions 对应节点后，输出不能恢复原文。
- 保留同协议必要的未知字段、重复 detail、detail 顺序、密文和引用增量。
- 不向供应方或客户端输出内部控制字段。

这些是待修复后的验收条件，本次未编写测试或声称它们已经通过。

## 生产配置验证记录

TYO 全局变换已追加一条启用的请求规则：

```json
{"transform":"field_set","enabled":true,"models":["gpt-*"],"phase":"request","config":{"path":"reasoning.summary","value":"detailed"}}
```

设置接口和数据库回读一致；原有 4 条规则未改动。
请求 `8551f79c-a629-488d-b367-071fd2334daa` 的抓包确认：

- 模型：`gpt-5.6-sol`；Provider：`0j1kha73`（LGS）。
- transformed URP reasoning 与 upstream request reasoning 均为 `{"effort":"low","summary":"detailed"}`。
- 请求成功完成，但没有 summary 文本或 summary delta。
- 因此已验证配置和参数注入，没有验证该上游一定生成概要，也没有验证 OpenWebUI 的新概要显示。

抓包位于服务器项目数据目录的 `dumps/8551f79c_20260911T092711235Z.json.zst`。
报告未保存会话令牌、API key、密文内容或用户聊天正文。

## 本地修复记录

- F1：`ReasoningConfig.summary` 已类型化；Chat 和 Responses 双向映射。字段变换直接读写类型控制。
- F2：删除原生 summary/content 文本副本。仅保存分段长度和未知成员，输出始终取当前类型文本。
- F3：Chat detail 仅保留未知成员和形状。真实流、合成流和非流输出均由当前类型字段构建。
- F4：删除 instructions 原文副本，从当前节点重建。会话亲和信息也读取当前指令节点。
- F5：展示选项迁入 `ReasoningMetadata`，不再复制正文或透传内部开关。
- F6：启停、预算和展示模式已类型化。移除预算到 effort 的粗略解码；原生配置仅保留未知成员。
- 附加迁移：推理传输 ID、redacted 标记、Messages 起始用量、引用增量和 Gemini 文本签名已有明确类型字段。
- 缓存身份信息迁入 `RequestContext`，不再注入供应方 extra_body。JavaScript 变换不能替换可信运行上下文。
- Responses 原生响应快照不再保存已类型化的输出、用量、身份或终态。终态由当前 finish_reason 重建。
- `AGENTS.md` 和 URP 规格已禁止用内部字段绕过类型字段，包括通过旧副本恢复已删除的值。

保留的内部字段仅承担原生形状、未知成员、来源限制或适配上下文职责。它们不能覆盖当前 URP 语义。
本次未扩展原生音频句柄、未知 ProviderItem 或结构化工具结果的跨协议支持。

### 验证

- `cargo test --lib`：现有 3 项测试通过；没有新增测试。
- 本地已编译库的转换探针：9 个输出样本，记录在 `urp-runtime-probe.log`。
- 探针确认 summary 双向转换、summary 删除、CoT 移动及删除、instructions 删除、终态更新、Gemini 预算及签名往返。
- `cd docs && bun install && bun run build`：通过。
- `git diff --check`：通过。

这些验证不等于生产 OpenWebUI 验收。TYO 及 fisx-mono 的运行程序尚未部署本次代码。
若上游只返回密文且不返回概要，转换层不能生成供应方未返回的概要。
