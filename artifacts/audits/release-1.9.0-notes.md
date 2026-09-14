推理概要和原始 CoT 现在统一通过类型化 URP 传递。修复协议转换和原生重放副本导致的概要丢失、变换失效及已删除内容恢复。

- 在 Chat Completions 和 Responses 之间保留 `reasoning.summary` 请求模式。
- 从当前 URP 内容构建 Chat reasoning details、Responses 推理项和 instructions，移除旧正文副本。
- 将推理启停、预算、展示选项、传输 ID、引用增量、起始用量和 Gemini 文本签名迁入类型字段。
- 将缓存身份信息移出供应方 extra_body。
- 更新字段变换和四种语言的相关文档，明确禁止内部字段绕过 URP。

验证：现有 Rust 测试、本地协议转换探针和四语言文档构建通过。生产 OpenWebUI 完整链路尚未验收。

升级提示：自定义 JavaScript 变换应使用类型化 reasoning 配置和 metadata，不再依赖已移除的内部字段。
