# 文件与媒体协议审计

审计日期：2026-09-12。对象：当前工作区的 Chat Completions、Responses、Messages、Gemini 编解码器，以及 URP、请求分发和流事件转换路径。

**结论：目前不能认定文件传输完整正确。** 常见的 PDF Base64 路径可保留字节和 MIME，但图片 data URL、文本文档、目标 MIME 校验、工具结果媒体和私有文件引用仍有缺口。部分现有双向测试验证的是编解码器的自洽性，其样本不符合官方响应结构。

本次只读审计没有修改实现、规格或永久测试。上一轮编解码器修改和用户原有改动均保留。没有调用付费模型、上传文件、部署、提交或推送。

## 主要发现

优先级表示影响和修复顺序，不表示这些问题都由上一轮修改引入。P1 表示合法内容丢失或目标结构错误；P2 表示元数据、适配或资源能力缺口。

| 编号 | 优先级 | 已观察到的行为 | 判断与证据 |
| --- | --- | --- | --- |
| F1 | P1 | Chat/Responses 的图片 `data:image/...;base64,...` 进入 `ImageSource::Url`，随后发成 Gemini `fileData.fileUri`。 | Base64 数据没有规范化到内联媒体分支。[图片解析](/Users/ikaleio/Projects/monoize/src/urp/decode/mod.rs:424)、[Gemini 编码](/Users/ikaleio/Projects/monoize/src/urp/encode/gemini.rs:542)。探针 `chat-image-data-url-*`、`responses-image-data-url-*`。Gemini 将内联字节放在 `inlineData`；其格式与文件 URI 不同。[官方 Blob](https://ai.google.dev/api/generate-content#Blob) |
| F2 | P1 | 合法的 Messages `document.source={type:"content",content:"alpha"}` 解码后变成空输入；同协议也无法恢复。 | 解析器只接受数组；规格 PM2c.1 也漏了字符串分支。[解析代码](/Users/ikaleio/Projects/monoize/src/urp/decode/mod.rs:613)。探针 `messages-content-string-*`。[官方 ContentBlockSource 类型](https://raw.githubusercontent.com/anthropics/anthropic-sdk-python/main/src/anthropic/types/content_block_source_param.py) |
| F3 | P1 | Messages 的 text 文档和 content 数组进入 URP 后，转往其余三个协议时被静默丢弃。 | 文件文本已在内存中，当前没有展开为目标 text/媒体，也没有明确返回不支持错误。这是能力与失败策略缺口。[Gemini](/Users/ikaleio/Projects/monoize/src/urp/encode/gemini.rs:554)、[Responses](/Users/ikaleio/Projects/monoize/src/urp/encode/openai_responses/request_response.inc.rs:260)、[Chat](/Users/ikaleio/Projects/monoize/src/urp/encode/openai_chat.rs:136)。探针 `messages-text-source-*`、`messages-content-array-*`。 |
| F4 | P1 | Gemini JSON 等非 PDF Base64 文件，被编码成 Messages `document.source.type=base64`，携带 `application/json` 等 MIME。URL 文件也直接进入 PDF URL source。 | Anthropic 的 Base64 文档 source 只允许 `application/pdf`；纯文本有单独的 text source。目标编码器缺少 MIME 适配/拒绝策略。[编码代码](/Users/ikaleio/Projects/monoize/src/urp/encode/anthropic/messages_part2.inc.rs:185)、[官方 PDF source 类型](https://raw.githubusercontent.com/anthropics/anthropic-sdk-python/main/src/anthropic/types/base64_pdf_source_param.py)。探针 `gemini-inline-json-*`。 |
| F5 | P1 | Messages 工具结果中的 URL 图片变成 Gemini `functionResponse.parts[].fileData`。 | 普通 Part 与 FunctionResponsePart 的结构不同；后者当前只定义 `inlineData`。[嵌套编码](/Users/ikaleio/Projects/monoize/src/urp/encode/gemini.rs:819)、[官方 FunctionResponsePart](https://ai.google.dev/api/generate-content#FunctionResponsePart)。探针 `messages-tool-result-media-*`。 |
| F6 | P1 | Responses 流工具结果把媒体编码为 `output_image` / `output_file`；非流工具结果则使用 `input_image` / `input_file`。 | 存在独立的流/非流编码分叉。[流工具结果编码](/Users/ikaleio/Projects/monoize/src/urp/stream_encode/openai_responses/encode_loop_part1.inc.rs:3505)、[官方结果类型](https://raw.githubusercontent.com/openai/openai-python/main/src/openai/types/responses/response_function_call_output_item_list_param.py)。本项为静态调用链证据；本次没有运行新增失败 SSE 样本。详见 OpenAI 分报告。 |
| F7 | P2 | 原始 OpenAI `file_data` Base64 即使附带 `filename=report.pdf`，也得到 `application/octet-stream`；Chat 生成音频的格式变成 `audio/unknown`。 | 缺少可靠 MIME 信息：文件的 octet-stream 会发给 Gemini/Messages；音频的 audio/unknown 会发给 Gemini，Messages 则省略 Audio 节点。文件 data URL 携带 MIME 时没有此问题。[文件默认值](/Users/ikaleio/Projects/monoize/src/urp/decode/mod.rs:544)、[音频默认值](/Users/ikaleio/Projects/monoize/src/urp/decode/openai_chat.rs:303)。探针 `*-pdf-raw-base64-*`、`chat-generated-audio`。未验证 OpenAI 是否仍能凭 filename 识别文件；这里不将其断言为 OpenAI 拒绝。 |
| F8 | P2 | Gemini HEIC 图片会携带 `image/heic` 原样发给 Messages。 | 两端图片格式集合不同；改 MIME 标签不能转码。[图片编码](/Users/ikaleio/Projects/monoize/src/urp/encode/anthropic/messages_part2.inc.rs:150)、[Anthropic 图片格式](https://platform.claude.com/docs/en/build-with-claude/vision)。需转码或明确报错。 |
| F9 | P2 | Responses input_file 同时含 file_url/file_id 与 filename 时，同协议解码/重编码也丢失 filename；文件 detail、Messages 文档 title/context/citations 等依赖 extras。 | filename 只在 Base64 source 中有 typed 位置。这是 schema 字段丢失；未验证服务端对 filename 与 URL/FileId 组合的具体语义。extras 在跨协议清理时会丢弃；关闭清理又可能泄漏目标不支持的字段。详见 OpenAI/Messages 分报告。[URP 文件类型](/Users/ikaleio/Projects/monoize/src/urp/mod.rs:677)、[条件清理](/Users/ikaleio/Projects/monoize/src/handlers/nonstream.rs:282)。 |
| F10 | P2 | file_id 仅按 OpenAI/Messages 两种来源区分；Gemini 私有文件 URI 被视为普通 URL。 | 没有 Provider、账户、项目绑定或转存。协议族相同不保证文件对当前凭据可见；跨协议引用可能被省略，或转成目标无法访问的 URL。详见下表。[Files API](https://platform.claude.com/docs/en/build-with-claude/files)、[Gemini 文件输入](https://ai.google.dev/gemini-api/docs/file-input-methods) |

Responses assistant 历史和各协议原生响应还存在输入/输出形状混用：Chat assistant content 被写入 image/file，Responses 写出 `output_image/output_file`，Messages 输出顶层 image/document。不能把“输入允许图片”推导成“响应允许同形状图片”。Responses stable SDK 的 easy-input assistant 类型定义包含 input_image/input_file；本次未验证真实模型接受性。应先选合法的目标结构，不能一律当成不支持。详细代码和官方类型见 [OpenAI 分报告](/Users/ikaleio/Projects/monoize/artifacts/file-protocol-audit-20260912/openai-audit.md) 与 [Messages 分报告](/Users/ikaleio/Projects/monoize/artifacts/file-protocol-audit-20260912/messages-audit.md)。

其他较小问题：Gemini 特殊音频 MIME `video/audio/s16le` 被归为 File；functionResponse 父 Part 元数据未保留；typed source 从视频改为 PDF 后仍带旧 videoMetadata。见 [Gemini 分报告](/Users/ikaleio/Projects/monoize/artifacts/file-protocol-audit-20260912/gemini-audit.md)。其中父 Part 签名只证明 schema 允许的字段会丢失，没有验证模型实际会生成这种函数结果。

## Base64、URL 与 MIME 的实际边界

| 输入/目标路径 | 当前结果 |
| --- | --- |
| OpenAI 图片 data URL → URP → Gemini | 错误选择 fileData；应拆出 MIME 与 Base64。 |
| OpenAI 图片 data URL → Messages | 正确转换为原始 Base64 与 media_type。 |
| OpenAI PDF file_data data URL → 四协议 | 文件字节和 application/pdf 均保留；Chat/Responses 同时保留 filename。 |
| OpenAI 原始 file_data Base64 | 数据保留，但 MIME 默认 octet-stream；没有从调用上下文或字节确定类型。 |
| Gemini 原生 inlineData | 常见图片、音频、PDF、视频的 MIME 和 Base64 保留。 |
| Gemini 原生 fileData | URI 与显式 MIME 保留。当前官方规定 fileData.mimeType 可省略，缺省本身不是格式错误。[FileData](https://ai.google.dev/api/generate-content#FileData) |
| URP URL 上的 typed media_type → Gemini | 文件、图片、音频三类均验证保留；未发现统一漏 MIME。 |
| Gemini inlineData 工具结果的 displayName / $ref | 对照样本保留；不能将它们笼统列为漏传。 |
| 公开 HTTPS URL | 可按目标协议能力传递；不等于自动下载、解码或转存。 |
| 私有 Files URI / gs:// | 需要来源、凭据和目标支持，当前 canonical URL 没有充分表达这些条件。 |

## 非图片文件支持

OpenAI 当前文件指南列出 PDF、文本/代码、Office 文档、演示文稿和表格。表格最多解析每张表前 1000 行；非 PDF 文档中的图片/图表不会自动进入模型上下文。Responses PDF 还支持 detail，而 Chat file 不支持此字段。[OpenAI 文件指南](https://developers.openai.com/api/docs/guides/file-inputs)。这些是 Provider 的文件处理能力，codec 保留字节不等于已经实现所有格式间转换。

| 文件形态 | 当前支持 | 缺口 |
| --- | --- | --- |
| 普通 user 消息中的 PDF，Base64 + 已知 MIME | 四种目标可形成对应输入结构。 | 未测试文件大小、页数、损坏文件、真实模型接受性。 |
| PDF，公开 URL | Responses / Messages / Gemini 有映射。 | Chat 文件输入没有 URL 分支，当前省略；未实现下载为 Base64。 |
| 文本 / JSON / CSV | Base64 文件可进入 FileSource，部分目标保留字节；Messages 原生 text source 可往返。 | 跨 Messages 转换缺少字节解码与 text source 映射；text/content 源向外丢失。 |
| DOCX / PPTX / XLSX | Responses 可以保留 data URI、filename 和 MIME。 | 不能原样映射成 Messages PDF document；没有文档解析、PDF 转换或其他适配。 |
| 音频 | Gemini 原生 MIME、参数及数据可保留；Chat 自身生成音频可往返。 | 不同目标的格式集合、响应结构和格式上下文未统一；没有转码。 |
| 视频 | Gemini 使用 FileSource 可保存媒体与原生处理配置。 | 其他目标不能靠通用 file 标签获得等价视频能力；typed 替换后需清理不兼容配置。 |
| 混合工具结果 | Responses 非流和 Messages 能保存支持的媒体输入结构。 | Gemini URL 媒体嵌套格式错误；Chat tool message 仅有文本分支，当前只留文本。 |

Anthropic 的 PDF/text、自定义文档与 Office/code execution 路径彼此不同。自动切到 code execution 会改变工具语义，不能作为无条件的文件适配。[Anthropic PDF 文档](https://platform.claude.com/docs/en/build-with-claude/pdf-support)。Gemini 的 PDF 与其他文本文档处理方式也不同。[Gemini 文档理解](https://ai.google.dev/gemini-api/docs/document-processing)。

## file_id 与资源来源

| 原始资源 | Chat 输出 | Responses 输出 | Messages 输出 | Gemini 输出 |
| --- | --- | --- | --- | --- |
| OpenAI 文档 file_id | 保留 ID | 保留 ID | 省略 | 省略 |
| OpenAI 图片 file_id | 省略 | 保留 ID | 省略 | 省略 |
| Messages 文档/图片 file_id | 省略 | 省略 | 保留 ID | 省略 |
| 来源未知的 URP FileId | 省略 | 省略 | 省略 | 省略 |
| Gemini 私有文件 URI，作为 PDF File | 省略 | 原样作为 file_url | 原样作为文档 URL | 原样作为 fileUri |

此表描述当前行为，不表示省略是理想策略。没有 file_id → bytes → target upload 的实现。协议来源标记只能防止明显的跨协议误用，不能保证同协议不同 Provider/项目的引用可用。Gemini/Anthropic 用户上传文件也不能被假定为可通过下载接口取回，因此转存层需要在上传时保存字节，或使用独立可访问的资源。不能凭现有 ID 推导出可移植文件内容。

## 验证证据与限制

- 主探针执行 **82 条**：40 个请求场景分别设置 stream=false/true，另有 2 个非流响应场景。输入见 [fixtures.jsonl](/Users/ikaleio/Projects/monoize/artifacts/file-protocol-audit-20260912/fixtures.jsonl)，完整 URP 与四目标输出见 [observations.jsonl](/Users/ikaleio/Projects/monoize/artifacts/file-protocol-audit-20260912/observations.jsonl)。
- 44 项重点检查中，17 项满足、27 项不满足。重复计算了 stream 标志变体及不同目标；这不是独立缺陷数，也不是产品通过率。包含官方结构检查、正向对照和明确标记的能力/错误策略检查。见 [check-results.json](/Users/ikaleio/Projects/monoize/artifacts/file-protocol-audit-20260912/check-results.json)。
- 请求上的 stream 标志不等于 SSE 测试。额外复跑了 Messages 文档媒体、Responses 图片文件、Chat 生成音频、Gemini inline file 的现有测试，共 **5 项通过**。各执行日志位于本目录。新增失败场景仅做静态共享函数/调用链分析，未动态执行对应的 SSE fixture。
- Gemini 另有 17 条补充探针；OpenAI 的额外样本和 Messages 的交叉检查见各分报告。补充样本包括负向输入与 URP 修改场景，不能全部称为合法原生输入。
- 样本 Base64 是短合成字节，只检查传输结构。没有声称这些字节是完整 PDF/图片/音频，也没有声称 Provider 已接受这些请求。
- `cargo build --lib --offline` 成功；本目录 Rust 探针链接当前 libmonoize。构建与现有测试成功不能消除上面的文档合同问题。

复现主结构检查：

```sh
bun artifacts/file-protocol-audit-20260912/generate-probes.mjs
artifacts/file-protocol-audit-20260912/probe < artifacts/file-protocol-audit-20260912/fixtures.jsonl > artifacts/file-protocol-audit-20260912/observations.jsonl
bun artifacts/file-protocol-audit-20260912/analyze-probes.mjs
```

`probe` 是当前机器上的构建产物；更改实现后应先重建库和探针。源码见 [probe.rs](/Users/ikaleio/Projects/monoize/artifacts/file-protocol-audit-20260912/probe.rs)。

## 修复顺序

1. 在共享解析层规范化图片 data URL，补 Messages 字符串文档分支。
2. 给文本文档和复合文档定义跨协议展开规则，明确不可转换内容的错误行为，停止静默丢弃。
3. 将 MIME 未知作为状态处理；根据目标协议验证类型，必要时转换内容或返回错误。
4. 分离普通 Part、工具结果 Part、assistant 历史和输出响应的合法结构；统一流/非流映射。
5. 给文件名、PDF detail、文档语义元数据增加合适的 typed 所有者；按目标过滤原生形状信息。
6. 定义 Provider/账户/项目级资源引用与文件解析层，处理路由、过期和不可下载来源。
7. 先更正规格中的遗漏和过宽条款，再增加官方格式断言与失败场景流测试。已有不合法响应样本应改成明确的扩展测试或移除。

以上修复尚未实施。
