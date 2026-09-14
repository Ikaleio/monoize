**Gemini 文件与媒体协议只读审计 — 2026-09-12**

审计对象是当前工作区的 GenerateContent 编解码器。检查前已阅读 URPV2-S1/S2/S9/S11、URPV2-12/13、PC2.5/2.6、PM2b/c、PG1–PG8。没有修改代码、规范或永久测试。

执行了 17 个离线结构探针，发现图片 data URL、文本文档、嵌套 URL 媒体、未知 MIME、父 Part 元数据和媒体替换后的残留元数据问题。实际 Google API 接受性未验证。样本中的短 Base64 字符串仅验证 JSON 映射，不代表完整可解码的媒体文件。

探针输入：[gemini-fixtures.jsonl](/Users/ikaleio/Projects/monoize/artifacts/file-protocol-audit-20260912/gemini-fixtures.jsonl)。完整 canonical 与四协议输出：[gemini-probe.jsonl](/Users/ikaleio/Projects/monoize/artifacts/file-protocol-audit-20260912/gemini-probe.jsonl)。探针由根代理提供的 `probe` 可执行程序运行。

**当前官方边界**

| 主题 | 官方要求与审计含义 |
| --- | --- |
| Part/Blob/FileData | `inlineData.data` 是原始字节的 Base64，必须配实际 MIME。`fileData.fileUri` 必填，`mimeType` 可省略。`videoMetadata` 只属于视频；当前参考页将它标为 deprecated。[GenerateContent API](https://ai.google.dev/api/generate-content#Part) |
| FunctionResponsePart | 当前 GenerateContent 参考结构仅列 `inlineData`，与普通 Part 的 `fileData` 分支不同。参考页提到 `$ref` 关联 `inlineData.display_name`，但 Blob 字段表未列 displayName，属于官方文档不一致。[FunctionResponse](https://ai.google.dev/api/generate-content#FunctionResponse) |
| 文档 | PDF 支持视觉理解，其他文本文档主要按纯文本处理。PDF 上限为 50 MB 或 1000 页。[文档理解](https://ai.google.dev/gemini-api/docs/document-processing#document-types) |
| 图片 | 指南列出 PNG/JPEG/WebP/HEIC/HEIF。各协议可接受的 MIME 集合不同。[图片格式](https://ai.google.dev/gemini-api/docs/image-understanding#supported-image-formats) |
| 音频/视频 | 音频指南列出具体音频 MIME；视频指南支持文件 URI、内联数据、公开 YouTube URL 和处理配置。指南现以 Interactions 示例为主，不能将其 JSON 直接当成 GenerateContent JSON。[音频](https://ai.google.dev/gemini-api/docs/audio#supported-audio-formats)、[视频](https://ai.google.dev/gemini-api/docs/video-understanding) |
| URL | 当前文件输入指南明确覆盖所有 Gemini API endpoint。公开 HTTPS/签名 URL 可直接使用；Gemini 2.0 不支持该方式。登录页面不可用，私有对象需要有效授权。[文件输入方式](https://ai.google.dev/gemini-api/docs/file-input-methods#external-http-signed-urls) |
| 上传/私有资源 | Files 资源归请求项目所有。用户上传文件不能通过 Files API 下载；生成文件另有下载方法。上传文件 48 小时后过期。GCS 需要注册与访问权限。[Files API](https://ai.google.dev/api/files)、[Files 指南](https://ai.google.dev/gemini-api/docs/files)、[GCS 注册](https://ai.google.dev/gemini-api/docs/file-input-methods#register-google-cloud-storage-files) |

**逐项发现**

1. **G1 / P1 / 代码映射缺口：图片 data URL 被编码成远程 URI。** `g01` 输入 Chat `image_url.url="data:image/png;base64,..."`；canonical 是 ImageSource::Url；Gemini 输出 `{"fileData":{"fileUri":"data:image/png;base64,..."}}`。Responses 的 `input_image.image_url` 使用相同解析器。这里应拆成 `inlineData` 的 MIME 和 Base64，不能依赖 Gemini 下载 data scheme。

   证据：[通用图片解码](/Users/ikaleio/Projects/monoize/src/urp/decode/mod.rs:424)、[Gemini 图片编码](/Users/ikaleio/Projects/monoize/src/urp/encode/gemini.rs:542)、[实际输出第 1 行](/Users/ikaleio/Projects/monoize/artifacts/file-protocol-audit-20260912/gemini-probe.jsonl:1)。官方依据是上表的 Blob/文件输入方式。最小修复：共享解析器规范化合法 Base64 data URL；无效 data URL 返回明确错误。增加对应 Gemini 映射规范，沿用 Messages 已有 PM2b.1 的语义。

2. **G2 / P1 / 能力缺口被静默处理：Messages 文本文档与复合文档消失。** `g03/g04` 含有完整文本内容，解码后分别是 FileSource::Text/Content；Gemini 输出 `contents:[]`。用户数据已在 URP 中，无需下载或上传就能转成普通 text/媒体 Parts。

   证据：[文档解码](/Users/ikaleio/Projects/monoize/src/urp/decode/mod.rs:604)、[明确返回 None](/Users/ikaleio/Projects/monoize/src/urp/encode/gemini.rs:554)、[上层直接跳过](/Users/ikaleio/Projects/monoize/src/urp/encode/gemini.rs:618)。官方依据：[文档理解](https://ai.google.dev/gemini-api/docs/document-processing#document-types)。最小修复：Text 转原生 text；Content 按顺序展开已支持的文本和媒体。明确文档标题、引用边界的损失策略，无法保持的部分应报错。PG4/PG5 当前没有定义这两种 source 的跨协议行为，需先补规范。

3. **G3 / P1 / 协议结构错误：工具结果复用普通 Part 编码器。** `g06` 中 Messages tool_result 的 URL 图片变成 `functionResponse.parts:[{"fileData":...}]`。当前 GenerateContent 的 FunctionResponsePart 只定义 inlineData。相同路径也会把 URL 文件放进该位置。

   证据：[嵌套媒体调用普通节点编码器](/Users/ikaleio/Projects/monoize/src/urp/encode/gemini.rs:819)、[写入 functionResponse.parts](/Users/ikaleio/Projects/monoize/src/urp/encode/gemini.rs:922)、[实际输出第 6 行](/Users/ikaleio/Projects/monoize/artifacts/file-protocol-audit-20260912/gemini-probe.jsonl:6)。官方依据：[FunctionResponsePart](https://ai.google.dev/api/generate-content#FunctionResponsePart)。最小修复：单独编码 FunctionResponsePart；URL 需要受控转换成 Base64，或返回不支持错误。不要把 Interactions 的 function_result 内容块规则套用到 GenerateContent。

4. **G4 / P2 / 规范与编码策略缺口：未知 MIME 被当成确定 MIME 发送。** `g02` 的原始 PDF Base64 带 `filename:"report.pdf"`，仍输出 `mimeType:"application/octet-stream"`。`g10/g13` 的 Chat 生成音频则输出 `mimeType:"audio/unknown"`。`g14` 对照样本提供 `data:application/pdf;base64,...` 后可正确编码。

   证据：[原始文件默认值](/Users/ikaleio/Projects/monoize/src/urp/decode/mod.rs:572)、[Chat 生成音频占位 MIME](/Users/ikaleio/Projects/monoize/src/urp/decode/openai_chat.rs:303)、[Gemini 原样输出 MIME](/Users/ikaleio/Projects/monoize/src/urp/encode/gemini.rs:559)。官方依据：[音频 MIME](https://ai.google.dev/gemini-api/docs/audio#supported-audio-formats)、[PDF 输入](https://ai.google.dev/gemini-api/docs/document-processing)。这证明当前输出没有可靠类型信息，不证明 Google 实际返回了某个错误码。PC2.5 目前要求原始文件使用 octet-stream，因此不能只改单个 decoder。最小修复：把 MIME 未知作为状态处理；保留调用时的音频格式，必要时使用字节识别。仍无法确定时拒绝发送；不要把占位值冒充真实类型。

5. **G5 / P2 / 资源能力边界：Gemini 私有 Files URI 没有来源约束。** `g17` 的 Files URI 解码为普通 FileSource::Url；Responses 将其作为 `file_url`，Messages 将其作为 document URL；Chat 则丢弃。canonical URL 没有记录项目/Provider 来源。其他 Provider 无法从该 URI 得到用户上传文件字节，且 Files 指南明确不提供用户上传文件的下载能力。

   证据：[FileData 解码](/Users/ikaleio/Projects/monoize/src/urp/decode/gemini.rs:804)、[URL source 结构](/Users/ikaleio/Projects/monoize/src/urp/mod.rs:677)、[Responses 原样传 URL](/Users/ikaleio/Projects/monoize/src/urp/encode/openai_responses/request_response.inc.rs:227)、[实际输出第 17 行](/Users/ikaleio/Projects/monoize/artifacts/file-protocol-audit-20260912/gemini-probe.jsonl:17)。官方依据：[文件所属项目](https://ai.google.dev/api/files#method:-files.list)、[上传文件不可下载](https://ai.google.dev/gemini-api/docs/files)。最小修复：区分公开 URL、GCS 资源和 Provider 私有文件引用。私有引用只允许匹配来源的路由；跨来源需要原始字节或独立可访问的签名 URL。普通公开 HTTPS URL 直传本身不是缺陷。

6. **G6 / P2 / canonical 分类缺口：Google 特殊音频 MIME 被分类成 File。** `g09` 的 `video/audio/s16le` 可原样回到 Gemini，但它的 canonical 类型是 File。当前分类只检查 `audio/` 前缀。参考页将 `video/audio/s16le`、`video/audio/wav` 列在音频类别中。[Blob](https://ai.google.dev/api/generate-content#Blob)

   证据：[分类条件](/Users/ikaleio/Projects/monoize/src/urp/decode/gemini.rs:783)、[未匹配类型转 File](/Users/ikaleio/Projects/monoize/src/urp/decode/gemini.rs:792)。最小修复：共享 MIME 分类器识别这些原生音频别名并保留原始 MIME。没有转码时，不要把原始 PCM 假称 WAV。PG5e 对这些特殊 MIME 没有显式定义，需补映射条款。

7. **G7 / P2 / 原生回放缺陷：functionResponse 外层 Part 元数据被丢弃。** `g12` 外层 `partMetadata` 和 `thoughtSignature` 均未进入 canonical，回放时消失。原因是 decoder 只把内部 functionResponse 对象传给 helper。Part 的通用字段在这里同样存在；尚未验证 Google 模型是否实际生成带签名的函数结果。

   证据：[输入提前返回](/Users/ikaleio/Projects/monoize/src/urp/decode/gemini.rs:613)、[输出使用同样分支](/Users/ikaleio/Projects/monoize/src/urp/decode/gemini.rs:625)、[实际输出第 12 行](/Users/ikaleio/Projects/monoize/artifacts/file-protocol-audit-20260912/gemini-probe.jsonl:12)。官方依据：[Part](https://ai.google.dev/api/generate-content#Part)。最小修复：保留父 Part 的未知元数据；为确需共享的签名建立唯一 typed 所有者。不能把父 Part 字段混入 functionResponse 对象。该问题与合法嵌套 inlineData 的 MIME/Base64 保存是两件事。

8. **G8 / P2 / typed 修改后未使原生形状失效：PDF 仍带视频裁剪配置。** `g16` 从 `g07` 的 canonical 视频节点出发，把 source 改成 PDF Base64。Gemini 输出新的 `application/pdf`，却仍带原来的 `videoMetadata.startOffset/endOffset/fps`。这违反 URPV2-S2 的不兼容回放形状失效规则。

   证据：[保留父 Part extras](/Users/ikaleio/Projects/monoize/src/urp/decode/gemini.rs:814)、[无媒体类型条件地合并 extras](/Users/ikaleio/Projects/monoize/src/urp/encode/gemini.rs:778)、[实际输出第 16 行](/Users/ikaleio/Projects/monoize/artifacts/file-protocol-audit-20260912/gemini-probe.jsonl:16)。官方依据：[VideoMetadata](https://ai.google.dev/api/generate-content#VideoMetadata)。最小修复：目标 source 不再是视频时移除 videoMetadata；对其他媒体专属处理配置采取相同兼容性检查。

**确认可保留的行为与路径覆盖**

| 路径/功能 | 当前结果 |
| --- | --- |
| 原生 inlineData 图片/音频/PDF/视频 | MIME 和原始 Base64 保留；图片是 Image、普通 audio MIME 是 Audio、PDF/视频是 File。`g15` 验证结构。视频使用 File 是当前 URP 设计，不单独报错。 |
| 原生 fileData + MIME | URI 与 typed MediaMetadata.media_type 保留。省略 MIME 时回放继续省略，`g08` 通过。helper 中的 image/*、audio/* 默认值会被 typed overlay 移除；它们不是实际最终输出。 |
| 原生 videoMetadata、mediaResolution、partMetadata | 未修改媒体时，`g07` 完整保留。没有自动转换为新的 Interactions processing JSON。 |
| 嵌套 inlineData + displayName/$ref | `g05` 完整保留 PDF MIME、数据、displayName 和 response 内 `$ref`。顶层 displayName 扩展 `g11` 也保留。displayName 不等于 FileSource.filename，不应直接视为共享文件名。 |
| Chat/Responses 文件 data URI → Gemini | `g14` 正确拆成 MIME 与 Base64。Messages 的原生 Base64 PDF 也使用相同 FileSource。 |
| 请求 stream=false/true | 共用 encode_request；stream 标志改变上游 endpoint，不改变媒体 Parts。未发现独立的流请求媒体分支。 |
| Gemini 非流响应/上游 SSE | [SSE 解码](/Users/ikaleio/Projects/monoize/src/urp/stream_decode/gemini.rs:100) 复用 [decode_stream_part](/Users/ikaleio/Projects/monoize/src/urp/decode/gemini.rs:878)，生成媒体 source delta 与完整终态。此前 55 项 Gemini 测试覆盖原生媒体双向 SSE；本次未新增永久测试。 |
| Gemini URP → 原生 SSE | [流编码器](/Users/ikaleio/Projects/monoize/src/urp/stream_encode/gemini.rs:116) 复用同一个媒体 Part 编码器，因此上述源类型/MIME问题也影响该路径。乱序完成按索引缓冲；无独立网络文件处理。 |
| 实际下游/合成流 | [生产分发](/Users/ikaleio/Projects/monoize/src/urp/stream_encode/mod.rs:13) 仅含 Chat、Responses、Messages。Gemini SSE 编码器是可调用模块，尚无 Gemini 下游 HTTP endpoint 或 synthetic dispatch。这是 PG5e 明确的范围边界。 |

**尚不支持或不能从离线探针证明的能力**

跨 Provider 私有 file_id/Files URI 转存、文件上传、GCS 注册、等待 PROCESSING→ACTIVE、文件过期刷新、媒体转码、模型 MIME 能力查询和大小校验均不在现有 codec 中。没有调用在线 Provider，不能宣称超限文件或特定 MIME 已被线上接受。公开 URL 的可用性取决于模型、网络可达性和授权；不能把它与 JSON 结构正确性混为一谈。

建议先补 G1–G4 的协议边界与失败行为，再定义私有资源和复合文档的 canonical 表示。G7/G8 已有 PG7、URPV2-S2 的约束，可直接按对应缺陷完善实现与回归测试。所有建议均未实施。
