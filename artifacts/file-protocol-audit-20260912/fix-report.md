# 文件与媒体传输修复

日期：2026-09-12。范围：Chat Completions、Responses、Messages、Gemini 的 URP 转换、请求路由及响应流。

这是上一轮修复记录。下文的 32 组图片矩阵仅覆盖普通 user 消息，没有覆盖 Chat 工具结果图片。后续审计发现解码和请求清理仍会丢失部分内容；更正及修复见 [follow-up-report.md](/Users/ikaleio/Projects/monoize/artifacts/file-protocol-audit-20260912/follow-up-report.md)。

原始审计的 F1–F10 已对应到实现和回归测试。这些结果只证明所列测试覆盖的路径。

## 审计问题与修复

| 问题 | 修复后的行为 |
| --- | --- |
| F1：图片 data URL 误入远程 URL 分支 | 解码分离 MIME 与原始 Base64。Gemini 使用 inlineData；Messages 使用 base64 source。图片 MIME 参数不进入原生固定枚举字段。 |
| F2：Messages 字符串复合文档丢失 | 字符串转换为一个有序文本块。仅原生字符串形状保留为适配元数据，文本由 URP 持有。 |
| F3：text/content 文档跨协议丢失 | 向其他协议展开为有序文本和媒体。title/context 在没有对应字段时成为模型可见文本，重复准备不会重复添加。 |
| F4：非 PDF 被当作 Messages PDF | PDF 保持 PDF；UTF-8 文本、JSON 等转换为 text source。不支持的二进制文件返回错误。URL 文档必须有 PDF 类型依据。 |
| F5：Gemini 工具结果使用非法 fileData | FunctionResponsePart 使用其独立结构。内联媒体使用 inlineData，嵌套 URL 媒体返回错误。 |
| F6：Responses 流式工具结果使用 output_image/output_file | 非流、真实 SSE 和合成 SSE 一致使用 input_text/input_image/input_file。工具结果有独立的 item 生命周期。 |
| F7：MIME 默认值掩盖未知类型 | 原始文件先根据字节特征，再根据文件扩展名确定 MIME。仍无法确定所需类型时返回错误。Chat 音频使用请求中的输出格式；未知格式不伪装成已知音频。 |
| F8：HEIC 等格式错误地发给不支持的目标 | 编码前检查目标格式集合。修改 MIME 标签不能替代转码。 |
| F9：文件和文档元数据丢失或重复持有 | filename、detail、title、context、citations、资源来源进入 typed MediaMetadata。工具结果媒体同样携带这些字段；typed 修改和删除覆盖原生重放值。 |
| F10：私有 file_id/URI 被当作通用引用 | 记录来源协议，并绑定 Provider、Channel、凭据范围。候选歧义、跨来源重试和历史回放不兼容均返回错误。没有创建或猜测目标 file_id。 |

Gemini 的 FileData.mimeType 是可选字段。未知 URL MIME 保持省略；有 typed MIME 时保留。辅助函数也不再生成 image/*、audio/* 或 application/octet-stream 占位值。[官方字段定义](https://github.com/googleapis/googleapis/blob/master/google/ai/generativelanguage/v1beta/content.proto)

## 集成修复

- 区分输入、assistant 历史和原生响应。普通 Chat、Messages、Responses 响应不再输出输入专用媒体形状。合法的 Chat audio 和 Responses image_generation_call 保留。
- Gemini 保留工具结果父 Part 的签名、未知父字段和合法音频 MIME 别名。视频替换为其他媒体后清除不再适用的视频配置。
- 检查完整请求后才生成上游请求体。非流响应在计费前检查目标表示；流式失败返回错误并停止成功结束流程。
- 记录编码器是否已发送终止错误序列，防止生产 SSE 包装层重复发送错误或结束帧。
- 成功编码后才写入托管响应历史。缓存保留输入和输出的资源范围，不能重新绑定到另一个账户。
- 原生解码时把复合文档和私有 URI 的来源提升到 typed 元数据。删除来源后，编码或路由不得从原生内容重新生成未绑定来源。
- 路由和流上下文直接从 URP 生成，避免通过 Responses 编码合法的 Chat/Gemini 音频或私有文件时误报模型缺失。

## 文件能力边界

| 内容 | 当前处理 |
| --- | --- |
| PDF Base64 | 在支持的请求位置映射四协议原生文件输入，保留字节与 MIME。 |
| 文本、JSON、CSV、代码 | 保留文本或文件字节；向 Messages 使用 UTF-8 text source。 |
| Messages 复合文档 | 保留文本和图片顺序；不支持的子块明确报错。 |
| Office 文件 | OpenAI 文件载体保留原始内容与类型。Messages/Gemini 不支持的二进制格式明确报错。 |
| 公共文件 URL | 使用目标支持的 URL 字段。Chat 文件 URL、Gemini 嵌套工具结果 URL 明确报错。 |
| 私有文件引用 | 只允许兼容且来源明确的路由。历史回放与重试保留同一资源范围。 |
| 音频、视频 | 使用协议原生载体和格式集合；目标没有对应能力时返回错误。 |

本次没有新增文件下载、跨账户上传、Office 转 PDF、图像或音视频转码。这些操作需要独立的数据与执行能力，不能由原生引用或 MIME 标签替代。

## 验证

- `cargo test --all-targets`：**231 passed，0 failed**。日志：[all-fix-tests.log](/Users/ikaleio/Projects/monoize/artifacts/file-protocol-audit-20260912/all-fix-tests.log)。
- 生产请求编码入口图片矩阵：**4 个来源 × 4 个目标 × 2 个 stream 标志，共 32 组通过**。每组包含两张 PNG 和中间文本，断言图片数量、完整字节和顺序。[专项日志](/Users/ikaleio/Projects/monoize/artifacts/file-protocol-audit-20260912/png-production-matrix.log)。
- SSE 终止专项：**6 项通过**，已包含在全量测试中。覆盖真实流、合成流、包装层、普通传输错误和已断开的客户端。[专项日志](/Users/ikaleio/Projects/monoize/artifacts/file-protocol-audit-20260912/stream-terminal-fix-tests.log)。
- `git diff --check`：通过。

回归覆盖协议到 URP、URP 到协议、typed 修改与删除、两个请求 stream 标志、非流响应、真实 SSE 和合成 SSE。另有资源候选筛选、凭据变更和历史保留测试。

这些检查验证离线协议结构及处理行为。没有调用真实模型服务，没有验证 Provider 的文件提取结果、大小限制或服务端接受性。

图片矩阵调用生产字段清理、资源绑定和上游请求编码函数。Gemini 来源先通过原生解码器进入同一编码入口；本次没有新增 Gemini HTTP 入口。

Cargo 提示已有依赖 proc-macro-error2 v2.0.1 的未来 Rust 兼容性问题。当前编译和全部测试通过。

原始审计样本和 observations.jsonl 保持为修复前证据，不代表修复后的结果。

对应规格：[media-transport.spec.md](/Users/ikaleio/Projects/monoize/spec/media-transport.spec.md)。共享回归：[media_transport_tests.rs](/Users/ikaleio/Projects/monoize/src/urp/media_transport_tests.rs)。
