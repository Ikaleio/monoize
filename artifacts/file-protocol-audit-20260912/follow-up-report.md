# 文件传输后续审计

日期：2026-09-12。范围：四协议解码器、目标编码器、真实 SSE 事件转换、请求清理和共享媒体准备。

上一轮的 32 组图片矩阵覆盖普通 user 消息。它没有证明 Chat 工具结果图片可用，也没有覆盖单个内容对象。先前据此作出的全面结论过宽。

本轮人工检查远端 `fd7edf2` 后，确认它修复了 Chat 工具结果媒体的解码。该语义已纳入本地修复。没有执行 merge、commit、push、发布或部署。

## 本轮修复

| 问题 | 修复后的行为 |
| --- | --- |
| Chat tool/function 图片被拒绝或变成文本 | 兼容的文本、图片、文件和音频进入 typed URP。工具结果保持独立，后续 user 图片不并入结果。未知 JSON 保留为 JSON 文本。 |
| 单个内容对象或字符串数组变成空内容 | 四协议在支持的内容位置处理字符串、单个对象和有序数组。已识别但畸形的媒体返回错误。 |
| 内容解码受源协议原生输出枚举限制 | 解码保留有明确语义的兼容媒体。目标编码器检查自身载体，不在源解码阶段丢弃。 |
| Responses 扫描任意 JSON，误把参数当作音频 | 校验仅检查内容位置。工具参数、custom input 和未知项的内部对象不触发媒体校验。 |
| 请求清理删除缺少本地调用的工具结果 | 只清理未获回答的 ToolCall。保留 ToolResult 的文本、图片和文件。 |
| URL MIME、内联图片 detail 丢失 | 显式 MIME 和 detail 进入 typed metadata。修改与删除覆盖原始字段。显式文档 MIME 优先于 PDF URL 默认值。 |
| 音频格式或来源被误判 | 区分 base64/url 判别字段；data URL 转为原始 Base64。使用格式或字节依据，保留私有 URL 来源。 |
| 复合文档文本块的引用和扩展字段丢失 | 原协议保留文本块字段；跨协议展开时，把引用转入普通 Text 的 typed citations。 |
| MIME 大小写影响分类或视频元数据 | 分类忽略类型和子类型大小写。目标固定字段使用合法值，原始 URP 保持不变。 |
| Gemini 签名使兼容媒体提前变成 Reasoning | 先解析媒体，再把签名放入该媒体的 typed metadata。 |
| Responses 流完成阶段合并或遗漏混合内容 | 完整内容按 content_index 保留 typed 节点，覆盖文本、媒体、未知块及空文本。 |
| 共享签名转换层重排终端控制节点 | 保持最终 URP 节点顺序；原始流编号不再被当成最终数组位置。签名传输按 call_id 关联，保留 typed 删除语义。 |
| 签名插入改变控制字段的目标 | 控制节点与其调用成组；迟到签名保持已发送内容顺序，终端才出现的签名不拆开其他调用组。 |
| Messages 立即输出的调用签名丢失绑定 | 调用签名传输与排队输出使用同一绑定格式。真实流到客户端历史回传的测试检查签名恢复。 |
| Gemini 末帧引用只进入最终快照 | 为已有 Text 发送引用增量，同时更新最终 typed 输出。无法修改已关闭文本块的目标明确报错。 |
| 目标编码器静默过滤不支持的工具结果 | Messages 原生输出拒绝 client ToolResult；Gemini 拒绝未实现桥接的 Custom call/result 及无法表示的已识别嵌套内容。 |

## 验证

最终 `cargo test --all-targets`：**266 passed，0 failed**。完整日志见 [all-follow-up-tests.log](/Users/ikaleio/Projects/monoize/artifacts/file-protocol-audit-20260912/all-follow-up-tests.log)。`git diff --check` 通过。

| 定向检查 | 结果 |
| --- | --- |
| Chat 编解码与流 | 32/32 |
| Responses 编解码与流 | 52/52 |
| Messages 编解码与流 | 40/40 |
| Gemini 编解码与流 | 78/78 |
| 签名关联与客户端历史回放 | 12/12 |

这些定向测试已包含在最终全量测试中。

- 共享媒体专项：17 项通过，见 [shared-follow-up-tests.log](/Users/ikaleio/Projects/monoize/artifacts/file-protocol-audit-20260912/shared-follow-up-tests.log)。
- 生产请求准备与资源路由：7 项通过，见 [production-follow-up-tests.log](/Users/ikaleio/Projects/monoize/artifacts/file-protocol-audit-20260912/production-follow-up-tests.log)。其中包括原来的 32 组 user 图片矩阵，以及 2 个来源 × 2 个 stream 标志的工具结果回归。
- 真实 SSE 转换检查包括 Messages 媒体到 Gemini 的原始字节一致性，以及 Gemini 迟到引用增量的发送与目标错误路径。

第一次全量检查是 262 通过、1 失败。失败项揭示了签名传输与控制节点关联的问题，保留于 [all-follow-up-first-run.log](/Users/ikaleio/Projects/monoize/artifacts/file-protocol-audit-20260912/all-follow-up-first-run.log)。修复没有放宽原来的签名顺序或未知内容位置断言。

修复前 Chat 最小回归有两项失败，见 [compat-chat-before.log](/Users/ikaleio/Projects/monoize/artifacts/file-protocol-audit-20260912/compat-chat-before.log)。后续测试同时检查协议到 URP 和 URP 到协议的内容、字节、类型、顺序及失败行为。

本轮使用离线协议 fixtures 和实际解码、请求准备、编码函数。没有验证真实 Provider 接受性、文件提取质量或模型理解结果。兼容形状也不代表源 Provider 的官方 API 承诺支持这些形状。

官方原生载体依据沿用 [media-transport.spec.md](/Users/ikaleio/Projects/monoize/spec/media-transport.spec.md) 的来源清单，并由协议代理复核。URL 及 Base64 不互相伪装，私有 file_id 不转换为另一个 Provider 的文件 ID。
