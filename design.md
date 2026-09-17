---
name: monoize-provider-design
description: 用于管理员查看 Provider 列表吞吐量；沿用现有明暗主题与管理操作。
version: 2026-09-18
---

# 1. 范围与优先级

[必须] 仅将本次规则用于 Provider 列表及其 RPM、TPM 区域。
[必须] 优先保留用户指定的位置、左对齐和现有操作。
[必须] 沿用 `DESIGN_SYSTEM.md` 与 `spec/frontend-design-system.spec.md` 的全局规则。
[建议] 不将本次局部提取用于营销页面或文档站。

# 2. 品牌与读者

[必须] 让管理员可以逐行比较 Provider 最近一分钟的请求量和 token 量。
[建议] 用中性色文字呈现指标，不增加装饰色或新表面。

# 3. 页面结构与构图

[必须] 保留现有页面标题、Provider 顺序、元数据和操作。
[必须] 在桌面开关左侧放置 RPM、TPM 两行文字。
[必须] 让所有 Provider 的指标列拥有相同宽度和相同左边缘。
[建议] 在不足以容纳完整标题与操作的窄屏中，将指标移至身份信息下方。决策来源：用户要求保留现有列表，且窄屏不得横向溢出。

# 4. 视觉规则

[必须] 将指标标签和数值分别左对齐。
[必须] 为数值使用 `tabular-nums`。
[建议] 为指标使用 `text-sm leading-5`。
[必须] 将指标标签与数值统一设为 `font-mono font-normal text-muted-foreground`，作为次级信息呈现。
[建议] 为桌面指标区域预留 `w-40 shrink-0`，按标签、数值组成两列。决策来源：用户红框的两行内容需求与本页密度；以真实渲染确认。
[必须] 保留主题 token，不硬编码明暗颜色。
[必须] 保留键盘焦点与操作可访问名称。
[必须] 保留页面初次加载骨架。
[必须] 将缺失指标显示为 `—`。
[必须] 保留后台刷新前的缓存数据。
[必须] 保留减少动效设置。

# 5. 可用原语

| 角色 | 实现名称 | 来源或路径 | 使用条件 | 状态 |
|---|---|---|---|---|
| Provider 行 | `ProviderCard` | `frontend/src/pages/providers/ProviderCard.tsx` | 列表条目 | 已实现 |
| 表面与标题 | `Card`、`CardHeader`、`CardTitle` | `frontend/src/components/ui/card.tsx` | 保留现有调用 | 已实现 |
| 正文、主题 | `foreground`、`muted-foreground`、`card`、`border`、字体变量 | `frontend/src/index.css` | 明暗主题 | 已实现 |
| 控件 | `Switch`、`Button` | `frontend/src/components/ui/` | 现有管理操作 | 已实现 |
| 帮助 | `TooltipProvider`、`Tooltip`、`TooltipTrigger`、`TooltipContent` | `frontend/src/components/ui/tooltip.tsx` | 可聚焦的指标说明 | 已实现 |
| 取数 | `useProviders` | `frontend/src/lib/swr.ts` | Provider 页面轮询 | 已实现 |
| 布局与次级字体 | Tailwind `grid`、`flex`、`gap-*`、`text-left`、`tabular-nums`、`font-mono`（`--font-code`） | `frontend/tailwind.config.cjs` | 指标与响应式布局 | 已实现 |

[建议] 页面自有样式命名空间不限制。
[必须] 仅在 Provider 调用点调整布局，不更改共享原语外观。
[建议] 沿用项目 `sm` 640px、`md` 768px、`lg` 1024px 断点。

# 6. 文案与数据

[必须] 从 Provider 的 `live_usage` 读取 `rpm`、`tpm`。
[必须] 以本地化整数显示数值，不用 K 或 M 缩写。
[必须] 在帮助文字说明最近 60 秒、已落库请求及输入加输出 token。
[必须] 通过 i18n 提供四种语言的帮助与可访问名称。
[必须] 让 Provider 页面每 10 秒刷新一次。
[必须] 不将空窗口的零值与未知值混淆。

# 7. 反模式

[建议] 不将列表改成居中标题加卡片网格。
[建议] 不为指标嵌套新卡片。
[建议] 不为指标增加装饰图标底板。
[建议] 不增加原语之外的颜色或字体。
[建议] 不用低对比小字承载指标值。
[建议] 不把指标包装成胶囊标签。

# 8. 实现与接入

[必须] 使用现有 React、Tailwind、SWR 和 i18n 入口。
[必须] 在 `frontend/src/pages/providers.tsx` 配置本页轮询。
[必须] 保持现有 Provider 操作和乐观更新。
[必须] 按 `spec/channel-management.spec.md` CM-LU 与 `spec/dashboard-ui-layout.spec.md` PL12b 实现数据及布局。

| 概念 | 名称 |
|---|---|
| 上游配置集合 | Provider |
| Provider 内的连接 | Channel |
| 最近一分钟请求数 | RPM |
| 最近一分钟输入与输出 token 总数 | TPM |
