---
name: monoize-dashboard-design
description: 用于 Monoize 控制台的管理列表页、目录页、概览页与钱包页：Provider 列表的 RPM/TPM 区域、令牌管理、支付管理、分组、订阅套餐、模型广场、系统仪表盘和钱包。读者是管理员与普通用户；目标是在明暗主题及 390–1440px 视口中查找、比较、判断并操作条目。
version: 2026-09-26
---

# 1. 范围与优先级

[必须] 仅将本文件用于以下页面：

|页面|路由|读者|
|---|---|---|
|Provider 列表（仅 RPM、TPM 区域）|`/dashboard/providers`|管理员|
|令牌管理|`/dashboard/tokens`|全部用户|
|支付管理|`/dashboard/payments`|管理员|
|分组|`/dashboard/groups`|管理员|
|钱包|`/dashboard/wallet`|全部用户|
|订阅套餐|`/dashboard/plans`|管理员|
|模型广场|`/dashboard/marketplace`|全部用户|
|系统仪表盘|`/dashboard/admin`|管理员|

[必须] 不将本文件用于登录页、文档站、Playground、请求日志、首页仪表盘、系统设置和其他未列出页面。
[必须] 沿用 `DESIGN_SYSTEM.md` 与 `spec/frontend-design-system.spec.md` 的全局规则。
[必须] 以 `spec/dashboard-ui-layout.spec.md`、`spec/recharge-system.spec.md`、`spec/admin-dashboard.spec.md`、`spec/model-marketplace.spec.md`、`spec/billing-plan-subscriptions.spec.md` 的页面条目为行为依据。
[必须] 冲突时依次保护：spec 与用户要求、无障碍与可用性、读者任务、本文件、装饰细节。

# 2. 品牌与读者

[必须] 让读者在首屏看到条目列表或余额，并能直接执行一次操作：启停、编辑、删除、复制、退款或充值。
[必须] 系统仪表盘首屏回答“是否有异常渠道”：渠道健康区块排在第一位，并在区块头给出渠道总数与异常数。
[建议] 以“克制的控制台”呈现品牌。可观察表现：

- 表面只用中性灰与白；彩色只出现在主要操作、焦点、状态徽标和带符号金额上。
- 每页只有一个实心主要操作按钮；行内操作全部是幽灵图标按钮。
- 页面标题使用衬线 `font-display`；其余文字使用无衬线正文字体或等宽字体。

[建议] 使用陈述语气。标题写名词短语，按钮写动词短语，说明写一句完整句子。

# 3. 页面结构与构图

## 3.1 管理列表页（令牌管理、支付管理、分组、订阅套餐）

[必须] 按以下顺序组成页面：

1. `PageHeader`：标题、一句说明、主要操作；批量操作也放在这里。
2. 工具栏行：左侧放搜索、筛选或 Tab；右侧放条目计数或当前 Tab 专属操作。
3. 列表表面：`DataTableShell` 包裹 `DataList`。
4. 分页页脚：仅在接口分页时出现，位于列表表面内部底部。

[必须] 让列表高度随行数变化；不为列表容器设置固定视口高度。
[必须] 让主内容区 `<main>` 成为唯一纵向滚动容器；列表内部不产生第二个纵向滚动条。
[必须] 不添加复述页面标题的章节标题或卡片标题。
[必须] 条目计数写在工具栏右侧，形如“共 7 个”，不另起一段。
[建议] 工具栏行使用 `flex flex-wrap items-center justify-between gap-3`，窄屏时换行而不压缩控件。

## 3.2 各管理页的首屏内容

|页面|标题区主要操作|工具栏左侧|工具栏右侧|列表列（宽模式，按顺序）|
|---|---|---|---|---|
|令牌管理|创建密钥；有选中时加“删除所选”|全选、按名称或密钥前缀搜索|计数|名称与密钥（行首为行选择框）、余额、限制、过期时间、状态、操作|
|支付管理·渠道|无|Tab|添加渠道|名称、类型、币种、汇率、充值范围、启用、操作|
|支付管理·订单|无|Tab|状态筛选、用户名筛选|创建时间、用户、支付方式、到账、支付、状态、订单号、退款|
|分组|新建分组|无|计数|位置、名称与描述、允许用户自选、操作|
|订阅套餐|新建套餐|无|计数|名称与描述、滑动窗口限额、可用分组、价格、倍率、上架状态、操作|

[必须] 令牌行把名称与分组徽标放在第一行，把密钥前缀与复制按钮放在第二行。
[必须] 分组行把名称与默认徽标放在第一行，把描述放在第二行。
[必须] 分组的上移、下移按钮与编辑、删除按钮同组，排在编辑按钮之前。
[必须] 支付管理只在“渠道”Tab 激活时显示“添加渠道”。
[必须] 套餐行把名称放在第一行，把描述放在第二行；限额每个已配置窗口一行，窗口标签保留 `5h`、`24h`、`7d`、`30d` 原文。
[必须] 套餐行内的文字左对齐；不居中。

## 3.3 钱包页

[必须] 按以下顺序组成页面：`PageHeader`（标题与说明，无操作）、余额摘要、下划线 Tab、Tab 面板。
[必须] 余额摘要保留唯一的主要操作“充值”。
[必须] 资金记录面板的卡片头只放“充值订单 / 余额流水”切换，不重复“资金记录”标题与说明。
[建议] 概览与充值面板保持现有结构（`spec/recharge-system.spec.md` RC-W2a、RC-W3、RC-W7）。

## 3.4 Provider 列表（仅 RPM、TPM）

[必须] 保留现有页面标题、Provider 顺序、元数据和操作。
[必须] 在桌面开关左侧放置 RPM、TPM 两行文字。
[必须] 让所有 Provider 的指标列拥有相同宽度和相同左边缘。
[建议] 在窄屏中将指标移至身份信息下方。决策来源：窄屏不得横向溢出。

## 3.5 目录页（模型广场）

[必须] 按以下顺序组成页面：`PageHeader`（标题与说明，无操作、无徽标）、工具栏行（左侧搜索，右侧“显示 x / y 个模型”）、`DataTableShell` 包裹的 `DataList`。
[必须] 宽模式列依次为：模型、模式、输入价格、输出价格、上下文、最大输出。
[必须] 模型单元格第一行为 `ModelIcon`、等宽模型 ID 与复制按钮；第二行为 Provider 原文的次要文字。
[必须] 目录只读；唯一的行内操作是复制模型 ID。
[建议] 用对齐的列表而不是卡片网格呈现目录。决策来源：基线中跨列网格让前 7 张卡片的“输入价格”左边缘落在 5 个不同位置（`artifacts/redesign-2026-09-24/measurements-2.md`），读者无法逐行比较价格。

## 3.6 概览页（系统仪表盘）

[必须] 按以下顺序组成页面：`PageHeader`（操作区为“刷新”按钮与“10 秒自动刷新”次要文字）、渠道健康区块（全宽）、下方两列区块。
[必须] 视口 ≥ `lg` 时，下方左列占 7/12，放用户使用排名；右列占 5/12，依次放系统状态与从机状态。两列各自向下堆叠，互不对齐高度。
[必须] 视口 < `lg` 时单列顺序为：渠道健康、用户使用排名、系统状态、从机状态。
[必须] 每个区块使用 `Card`：`CardHeader` 放 `CardTitle` 与一句 `CardDescription`，内容放 `CardContent`。
[必须] 渠道健康区块头左侧放标题与“N 个渠道 · M 个异常”；右侧放消费窗口分段控件；下一行放窗口消费总额、调用次数与滚动窗口说明。
[必须] 系统状态与从机状态使用键值行：每行左侧标签、右侧值，行间 `divide-y`。
[必须] 页面不使用固定页脚；原页脚的健康状态条目与亲和绑定数并入系统状态。

# 4. 视觉规则

## 4.1 字体

|角色|实现|使用条件|
|---|---|---|
|页面标题|`PageHeader` 内置：`font-display text-2xl font-semibold`（24/32px）|每页唯一 `h1`|
|页面说明|`PageHeader` 内置：`text-sm text-muted-foreground`（14/20px）|标题下一句话|
|卡片标题|`CardTitle`：`text-base font-semibold`|卡片内分区|
|列表表头|`DataListHead`：`text-xs font-medium text-muted-foreground`|宽模式表头|
|行主文字|`text-sm font-medium`|名称、渠道名|
|行正文|`text-sm`|其余单元格|
|行次要文字|`text-sm text-muted-foreground`|描述、日期、密钥前缀|
|标识符|`font-mono text-sm`|密钥前缀、`type_id`、订单号|
|余额主数值|`text-4xl font-semibold tabular-nums`|钱包余额摘要|
|键值行|标签 `text-sm text-muted-foreground`；值 `text-sm text-end`|系统状态、从机状态|
|分段控件|`Button size="sm"`，`text-sm tabular-nums`|消费窗口|

[必须] 页面标题一律通过 `PageHeader` 渲染；钱包页不再使用 30px 自写 `h1`。
[必须] 列表行内除徽标外全部使用 `text-sm`；不在行内使用 `text-xs`、`text-[10px]` 或 `text-[11px]`。
[必须] 为金额、数量、汇率、日期使用 `tabular-nums`。
[必须] 列表行内的金额使用无衬线字体；不使用 `font-display`。
[必须] Provider 指标标签与数值统一设为 `font-mono font-normal text-sm leading-5 text-muted-foreground`。
[必须] 键值行的值只对技术标识（监听地址、指标路径、数据库 DSN、路由配置版本、版本号）使用 `font-mono`；数量与时长使用无衬线 `tabular-nums`。

## 4.2 颜色

[必须] 只使用 `frontend/src/index.css` 的语义 token，不写色值字面量或 Tailwind 调色板色名。
[必须] 管理页主要操作使用 `Button` 默认 variant（`bg-foreground`）。
[必须] 钱包主要操作使用 `bg-wallet-action text-wallet-action-foreground`。
[必须] 状态色只用于 `OrderStatusBadge`、`StatusBadge`、账本带符号金额和错误反馈。
[必须] 账本金额为正时使用 `text-success`，为负时使用 `text-error-foreground`。
[必须] 充值订单行的到账金额使用 `text-foreground`，不带正负号。
[必须] 停用或已过期条目的名称使用 `text-muted-foreground`；不降低整行 `opacity`。
[必须] 删除类图标按钮的图标使用 `text-error-foreground`；按钮本身保持 `ghost`。
[必须] 不把 `text-destructive` 用作文字或图标颜色。`destructive` 只作实心按钮背景。来源：暗色主题下 `StatusBadge` 的“失败”文字对比度实测为 1.84:1（`/dashboard/wallet?tab=activity`，1440 × 900）。适用：本文件范围内的全部页面。不适用：`Button variant="destructive"`。
[必须] 渠道状态徽标：健康用 `StatusBadge variant="success"`，异常用 `variant="destructive"`，冷却中用 `variant="warning"`，已停用用 `Badge variant="secondary"`。
[必须] 从机状态徽标：在线用 `StatusBadge variant="success"`，过期用 `variant="warning"`；上报已启用用 `variant="success"`，未启用用次要文字。
[必须] 套餐已上架用 `StatusBadge variant="success"`；未上架用次要文字。

## 4.3 容器、栅格与间距

[必须] 页面宽度沿用 `layout.tsx` 的 `max-w-6xl`；页面不再设置更窄的最大宽度。
[必须] 页面一级区块之间使用 24px（`PageWrapper className="space-y-6"`）。
[必须] 工具栏行与列表表面之间使用 12px（`DataTableShell` 内置 `space-y-3`）。
[必须] 行内边距使用 `DataListRow` 内置的 `px-4 py-3`；页面不覆盖。
[必须] `DataList` 按自身宽度切换布局：

|列表宽度|布局|
|---|---|
|< 36rem|单列堆叠：主单元格、各“标签—值”行、操作行|
|36rem–56rem|主单元格与操作占满一行，其余字段两列排布|
|≥ 56rem|按 `columns` 对齐成列，显示表头|

[必须] 宽模式下，同一列的表头与单元格使用相同的 `align`。
[必须] 金额、汇率和数量列使用 `align="end"`。
[必须] 不通过表格内部横向滚动隐藏状态或操作。
[必须] `columns` 只使用 `rem` 定宽轨道或 `minmax(0,Nfr)` 弹性轨道；不使用 `auto`、`min-content`、`max-content` 或 `fit-content`。原因：表头与每一行是独立的栅格，内容尺寸轨道会让各行列宽不同。
[必须] 操作列宽度按该列最多按钮数计算：按钮数 × 2.25rem + (按钮数 − 1) × 0.25rem，再向上取整到 0.5rem。
[必须] 列数 ≤ 4 且全部列在 20rem 内放得下的紧凑表，使用 `Table` 原语，不使用 `DataList`。适用：系统仪表盘用户使用排名。原因：该表放在 7/12 列内（约 640px），永远达不到 `DataList` 的 56rem 宽模式阈值。
[必须] `Table` 使用 `className="table-fixed"`：定宽列写 `w-*`，名称列不写宽度并在单元格内 `truncate`；表头加 `whitespace-nowrap`。来源：系统仪表盘用户排名首轮渲染中，“调用数”表头被挤成三行（1440 × 900）。适用：全部 `Table`。
[必须] 卡片内的列表不再包 `DataTableShell`；卡片自身使用 `overflow-clip`。

## 4.4 边框、圆角、阴影与表面

[必须] 列表表面使用 `DataTableShell` 的 `Card`：`rounded-lg border bg-card`，无阴影。
[必须] 行之间使用 `divide-y` 分隔；悬停只改变背景为 `bg-muted/50`。
[必须] 不在卡片内部再放卡片。
[必须] 徽标形状使用 `rounded-md`；`rounded-full` 只用于圆点、头像、开关和加载指示。

## 4.5 图标与数据表达

[必须] 只使用 `lucide-react` 图标。
[必须] 图标只出现在按钮、Tab 触发器和空状态中；唯一例外是模型广场模型 ID 前的 `ModelIcon` 品牌标识，它不加底板，并设 `aria-hidden`，因为 Provider 已以文字显示。
[必须] 卡片标题前不放图标。
[必须] 不在列表行首放装饰性图标底板；不在日期前放日历图标。
[必须] 缺失值显示 `—`。
[必须] Provider 指标以本地化整数显示，不用 K 或 M 缩写；空窗口显示 `0`，未知值显示 `—`。

## 4.6 控件与状态

[必须] 启停使用 `Switch`，其 `aria-label` 包含条目名称。
[必须] 行内图标按钮使用 `variant="ghost" size="icon"`，尺寸 `size-11 sm:size-9`，并提供 `aria-label`。
[必须] 每个可聚焦元素保留 `focus-visible:ring-2 ring-ring`。
[必须] 首次加载显示与就绪布局相同结构的骨架；后台刷新时保留缓存数据。
[必须] 空列表使用 `EmptyState`，并在有权限时提供创建操作。
[必须] 不可执行的操作使用 `disabled`，并用 `title` 或提示说明原因。
[必须] 令牌过期时，过期时间单元格显示 `StatusBadge variant="warning"`“已过期”。
[必须] 紧跟标识符的复制按钮使用 `variant="ghost" size="icon"`、`size-11 sm:size-7`；复制后图标切换为 `Check` 2 秒。
[必须] 分段控件的每个按钮设 `aria-pressed`，选中项使用 `bg-accent text-accent-foreground`。
[必须] 首次加载失败使用 `QueryError`；有缓存时在缓存数据上方显示 `QueryError stale`，不显示原始错误信息。

## 4.7 动效

[必须] 沿用 `PageWrapper` 的页面进入动效和 `AnimatedButton` 的按钮缩放。
[必须] 钱包资金记录行保留 RC-W8 的弹簧进入动效。
[必须] 管理列表行不添加进入位移或缩放动效。
[必须] 减少动效时只允许透明度变化。

# 5. 可用原语

| 角色 | 实现名称 | 来源或路径 | 使用条件 | 状态 |
|---|---|---|---|---|
|页面容器|`PageWrapper`|`frontend/src/components/ui/motion.tsx`|每页根节点|已实现|
|页面标题区|`PageHeader`（`title`、`description`、`actions`）|`frontend/src/components/ui/page-header.tsx`|每页唯一标题|已实现|
|列表外壳|`DataTableShell`（`toolbar`、`isEmpty`、`emptyState`）|`frontend/src/components/ui/data-table-shell.tsx`|管理列表表面|已实现|
|工具栏搜索|`TableToolbarSearch`|同上|令牌搜索|已实现|
|响应式列表|`DataList`（`columns`）、`DataListHeader`、`DataListHead`（`align`）、`DataListBody`、`DataListRow`（`asChild`）、`DataListCell`（`label`、`align`、`primary`）、`DataListActions`|`frontend/src/components/ui/data-list.tsx`|四个页面的全部列表|已实现|
|滚动容器|`DashboardScrollParentContext`|`frontend/src/lib/dashboard-scroll.ts`（由 `pages/layout.tsx` 提供）|`Virtuoso` 的 `customScrollParent`|已实现|
|虚拟列表|`Virtuoso`|`react-virtuoso`|令牌列表|已实现|
|空状态|`EmptyState`（`card`、`inline`）|`frontend/src/components/ui/empty-state.tsx`|空列表|已实现|
|页面骨架|`TablePageSkeleton`、`PageHeaderSkeleton`、`Skeleton`|`frontend/src/components/ui/page-skeleton.tsx`、`skeleton.tsx`|首次加载|已实现|
|按钮|`Button`（`default`、`outline`、`ghost`、`destructive`）、`AnimatedButton`|`frontend/src/components/ui/button.tsx`、`motion.tsx`|操作|已实现|
|开关与选择|`Switch`、`Checkbox`、`Select`、`Input`|`frontend/src/components/ui/`|表单与行内控件|已实现|
|徽标|`Badge`、`StatusBadge`、`OrderStatusBadge`、`GroupsBadge`、`BadgeOverflowList`|`components/ui/badge.tsx`、`status.tsx`、`components/recharge/`、`components/`|状态与限制|已实现|
|区块卡片|`Card`、`CardHeader`、`CardTitle`、`CardDescription`、`CardContent`|`frontend/src/components/ui/card.tsx`|系统仪表盘区块、钱包资金记录|已实现|
|紧凑表|`Table`、`TableHeader`、`TableBody`、`TableRow`、`TableHead`、`TableCell`|`frontend/src/components/ui/table.tsx`|≤ 4 列且放得进 20rem 的表|已实现|
|虚拟列表适配|`virtualDataListComponents`|`frontend/src/components/ui/data-list-virtual.tsx`|`Virtuoso` 的 `components`；列表 `aria-label` 通过 `context.label` 传入|已实现|
|加载失败|`QueryError`（`onRetry`、`retrying`、`stale`）|`frontend/src/components/ui/query-error.tsx`|数据加载或刷新失败|已实现|
|模型标识|`ModelIcon`|`frontend/src/components/ModelIcon.tsx`|模型广场模型 ID 前|已实现|
|消费窗口|`SpendWindowControl`|`frontend/src/pages/admin-dashboard/spend-window-control.tsx`|渠道健康区块头|已实现|
|提示|`TooltipProvider`、`Tooltip`、`TooltipTrigger`、`TooltipContent`|`frontend/src/components/ui/tooltip.tsx`|完整订单号、禁用原因、指标说明|已实现|
|切换|`Tabs`、`TabsList`、`TabsTrigger`、`TabsContent`|`frontend/src/components/ui/tabs.tsx`|支付管理、钱包|已实现|
|对话框|`Dialog`、`AlertDialog`|`frontend/src/components/ui/`|编辑与破坏性确认|已实现|
|颜色与字体|语义 token、`font-display`、`font-mono`|`frontend/src/index.css`、`tailwind.config.cjs`|全部页面|已实现|
|取数|`useApiKeys`、`usePaymentChannels`、`useRechargeOrders`、`useDashboardGroups`、`useProviders`、`useBillingPlans`、`useMarketplaceModels`、`useAdminOverview` 与对应乐观更新函数|`frontend/src/lib/swr.ts`|数据与变更|已实现|

`DataList` 最小用法：

```tsx
<DataTableShell toolbar={toolbar} isEmpty={rows.length === 0} emptyState={empty}>
  <DataList columns="minmax(0,1.5fr) 7rem 5rem">
    <DataListHeader>
      <DataListHead>{t("common.name")}</DataListHead>
      <DataListHead align="end">{t("wallet.credit")}</DataListHead>
      <DataListHead align="end">{t("common.actions")}</DataListHead>
    </DataListHeader>
    <DataListBody aria-label={t("groups.title")}>
      {rows.map((row) => (
        <DataListRow key={row.id}>
          <DataListCell primary>{row.name}</DataListCell>
          <DataListCell label={t("wallet.credit")} align="end">${row.credit_usd}</DataListCell>
          <DataListActions>{/* ghost icon buttons */}</DataListActions>
        </DataListRow>
      ))}
    </DataListBody>
  </DataList>
</DataTableShell>
```

[必须] 页面只使用本章列出的原语名称，不猜测未列出的名称。
[建议] 页面自有样式命名空间不限制；只使用 Tailwind 工具类，不新增全局 CSS 类。
[必须] 页面自有样式不改变 `DataList`、`PageHeader`、`Card` 的内边距、字号、边框和表面。
[必须] 沿用项目 `sm` 640px、`lg` 1024px 视口断点；`DataList` 使用第 4.3 节的容器宽度阈值。

# 6. 文案与数据

[必须] 所有可见文字通过 i18n 提供 `en`、`zh`、`zh-TW`、`ja` 四种语言。
[必须] Provider、Channel、`type_id`、环境变量与端点路径在所有语言中保持英文原文。
[必须] 页面说明写一句话，说明本页管理的对象。
[必须] 金额以 `$` 加后端规范字符串显示；余额使用 `formatUsdDecimal(value, 2)`；账本金额使用 `formatNanoUsd(value, 4)`。
[必须] 汇率写成“1 USD = {rate} {currency}”。
[必须] 充值范围写成“$min – $max”。
[必须] 订单与账本时间使用 `formatTime`；令牌过期日期使用 `formatDate`。
[必须] 订单号显示前 8 个字符，完整订单号与 `error_code` 放在提示中。
[必须] 令牌余额列在子账户关闭时显示普通次要文字“使用账户余额”，不使用徽标。
[必须] 钱包错误状态不显示原始错误信息。
[必须] Provider 指标帮助文字说明最近 60 秒、已落库请求及输入加输出 token。
[必须] 模型价格使用 `formatUsdDecimal(value, 3)`；单位“每 1M tokens”写在表头与窄屏标签中，单元格不重复“/ 1M”。
[必须] 上下文与最大输出以 `K`、`M` 缩写显示；缺失显示 `—`。
[必须] 套餐限额显示为 `$` 加精确十进制金额（去掉末尾 0，例如 `$5`、`$0.005`），不四舍五入。
[必须] 套餐价格写成“$价格 / N 天”；没有价格时显示 `—`。
[必须] 系统仪表盘时间使用本地 `YYYY-MM-DD HH:mm:ss`；运行时长写成 `2d 4h 12m`；字节写成 `B`、`KB`、`MB`、`GB`。
[必须] 消费窗口标签 `24h`、`3d`、`7d`、`14d`、`30d` 在所有语言中保持原文。
[必须] 不补造数据、状态或结论。

# 7. 反模式

[必须] 不为列表设置固定视口高度而在行下方留出空白。
[必须] 不在窄屏通过表格内部横向滚动隐藏状态和操作。
[建议] 不使用居中标题区加卡片网格作为默认页面结构。
[建议] 不在卡片内嵌套卡片。
[建议] 不使用装饰性图标块或彩色图标底板。
[建议] 不使用原语之外的字号、字重或颜色字面值。
[建议] 不用小号低对比文字承载主要信息。
[建议] 不为普通元数据使用胶囊标签。
[建议] 不添加复述页面标题或 Tab 名称的标题。
[建议] 不为未入账订单金额添加“+”号。
[必须] 不在页面内再放一个 `<main>` 或自建纵向滚动区；主内容区是唯一纵向滚动容器。
[必须] 不让宽表在桌面视口依靠横向滚动显示状态列。来源：基线系统仪表盘渠道健康表在 1440 × 900 下隐藏 185px，“最近探测”列不可见。
[建议] 不在卡片描述中重复卡片内已显示的值。
[建议] 不为结果数量、模式等元数据使用胶囊徽标。

# 8. 实现与接入

[必须] 使用现有 React、Tailwind 3.4、shadcn/ui、SWR 与 i18n 入口。
[必须] `DataList` 以 `container-type: inline-size` 与 `[@container(min-width:…)]:` 任意变体实现宽度阈值；不新增 Tailwind 插件。
[必须] 令牌列表与渠道健康列表使用 `Virtuoso` 与 `virtualDataListComponents`，并把 `DashboardScrollParentContext` 提供的 `<main>` 元素传给 `customScrollParent`。
[必须] 列表表头在宽模式下使用 `sticky top-0`，相对 `<main>` 固定；`DataTableShell` 的卡片使用 `overflow-clip`，不建立滚动容器。
[必须] 每个用户触发的变更保留现有乐观更新与失败回滚。
[必须] 在 `frontend/src/pages/providers.tsx` 配置 Provider 页面 10 秒轮询。
[必须] 按 `spec/channel-management.spec.md` CM-LU 与 `spec/dashboard-ui-layout.spec.md` PL12b 实现 Provider 指标。

| 概念 | 名称 |
|---|---|
|上游配置集合|Provider|
|Provider 内的连接|Channel|
|访问 Monoize API 的凭据|API 密钥（路由 `/dashboard/tokens`，代码 `ApiKey`）|
|充值收款配置|支付渠道（代码 `PaymentChannel`）|
|充值请求|充值订单（代码 `RechargeOrder`）|
|钱包余额变动|账本条目（代码 `billing_ledger`）|
|路由分组|分组（代码 `Group`）|
|最近一分钟请求数|RPM|
|最近一分钟输入与输出 token 总数|TPM|
|响应式列表原语|`DataList`|
|管理员定义的滑动窗口额度与售价|订阅套餐（代码 `BillingPlan`）|
|模型广场中的一条模型记录|目录条目（代码 `MarketplaceModelRecord`）|
|消费统计的滚动时间段|消费窗口|
|≤ 4 列的紧凑表原语|`Table`|
