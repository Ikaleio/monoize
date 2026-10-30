---
name: monoize-dashboard-design
description: 用于 Monoize 控制台的管理列表页、目录页、概览页、设置页与钱包页：Provider 列表的 RPM/TPM 区域、API 密钥、用户管理、模型定价列表、支付管理、分组、订阅套餐、模型广场、系统仪表盘、系统设置和钱包。读者是管理员与普通用户；目标是在明暗主题及 390–1440px 视口中查找、比较、判断并操作条目。
version: 2026-10-30
---

# 1. 范围与优先级

[必须] 仅将本文件用于以下页面：

|页面|路由|读者|
|---|---|---|
|Provider 列表（仅 RPM、TPM 区域）|`/dashboard/providers`|管理员|
|API 密钥|`/dashboard/tokens`|全部用户|
|用户管理|`/dashboard/users`|管理员|
|模型定价（仅“模型定价”“未定价模型”Tab 的列表）|`/dashboard/models`|管理员|
|支付管理|`/dashboard/payments`|管理员|
|分组|`/dashboard/groups`|管理员|
|钱包|`/dashboard/wallet`|全部用户|
|订阅套餐|`/dashboard/plans`|管理员|
|模型广场|`/dashboard/marketplace`|全部用户|
|系统仪表盘|`/dashboard/admin`|管理员|
|系统设置|`/dashboard/admin-settings`|管理员|

[必须] 不将本文件用于登录页、文档站、Playground、请求日志、首页仪表盘、用户设置（`/settings`）和其他未列出页面。
[必须] 沿用 `DESIGN_SYSTEM.md` 与 `spec/frontend-design-system.spec.md` 的全局规则。未保存修改保护、字段校验与错误提示、数值与列表输入、加载失败与无权限、URL 状态、颜色语义、确认与撤销、焦点顺序（DS72–DS81）适用于控制台全部页面，包括上一条排除的页面；第 4.6 节只摘录这些规则。
[必须] 以 `spec/dashboard-ui-layout.spec.md`、`spec/recharge-system.spec.md`、`spec/admin-dashboard.spec.md`、`spec/model-marketplace.spec.md`、`spec/billing-plan-subscriptions.spec.md`、`spec/system-settings-ui.spec.md` 的页面条目为行为依据。
[必须] 冲突时依次保护：spec 与用户要求、无障碍与可用性、读者任务、本文件、装饰细节。

# 2. 品牌与读者

[必须] 让读者在首屏看到条目列表或余额，并能直接执行一次操作：启停、编辑、删除、复制、退款或充值。设置页的首屏要求见第 3.7 节。
[必须] 系统仪表盘首屏回答“是否有异常渠道”：渠道健康区块排在第一位，并在区块头给出渠道总数与异常数。
[建议] 以“克制的控制台”呈现品牌。可观察表现：

- 表面只用中性灰与白；彩色只出现在主要操作、焦点、状态徽标和带符号金额上。
- 每页只有一个实心主要操作按钮；行内操作全部是幽灵图标按钮。
- 页面标题使用衬线 `font-display`；其余文字使用无衬线正文字体或等宽字体。

[建议] 使用陈述语气。标题写名词短语，按钮写动词短语，说明写一句完整句子。

# 3. 页面结构与构图

## 3.1 管理列表页（API 密钥、用户管理、模型定价列表、支付管理、分组、订阅套餐）

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
|API 密钥|创建密钥；有选中时加“删除所选”|全选、按名称或密钥前缀搜索|计数|名称（行首为行选择框，含分组徽标）、密钥、限制、过期时间、独立余额（仅当有密钥开启独立余额）、状态、操作|
|支付管理·支付渠道|无|Tab|添加支付渠道|名称、类型、币种、汇率、充值范围、启用、操作|
|支付管理·订单|无|Tab|状态筛选、用户名筛选|创建时间、用户、支付渠道、到账、支付、状态、订单号、退款|
|用户管理|添加用户|按用户名或邮箱搜索（`?q=`）|计数与今日消耗、调用次数|用户（头像；第一行用户名，第二行角色与分组徽标）、余额、今日（消耗与调用两行）、套餐（名称与到期两行）、日期（创建与最后登录两行）、状态、操作（编辑与“更多操作”菜单）|
|模型定价·模型定价|无|Tab（`?tab=` 取 pricing、unpriced、tools、sync；未定价 Tab 带数量徽标）、搜索|添加定价|模型、计费模式、输入价格、输出价格、来源、状态（启用/不完整/停用与锁定数）、更新时间、操作|
|模型定价·未定价模型|无|搜索|计数|模型（第二行为 `Provider / Channel` 来源）、操作|
|分组|新建分组|无|计数|位置、名称与描述、计费倍率、允许用户自选、操作|
|订阅套餐|新建套餐|无|计数|名称与描述、滑动窗口限额、可用分组、价格、用量倍率、上架状态、操作|

[必须] API 密钥列表使用 `DataList dense`，宽模式每行只有一行文字：名称与分组徽标一列，密钥前缀与复制按钮一列。来源：两行布局在 1440 × 900 下行高 72–75px，19 个密钥只能看到 8 行，名称列右侧大片空白。
[必须] API 密钥列表在宽模式以下分两部分：名称与启用开关一行；其下是可换行的密钥、限制、过期与独立余额，操作按钮靠右排在同一行，该组不足 14rem 时操作另起一行。不使用逐字段的“标签—值”行。来源：逐字段排布在 1024 × 768 下每行 182px，在 390 × 844 下每行 262px。
[必须] API 密钥名称列可用宽度不足 12rem 时不显示分组徽标，名称优先。来源：1280 宽时徽标把名称挤到 0px。
[必须] API 密钥页在页头下方放一行只读“API 基址”、等宽 URL 与复制按钮，不加边框；创建结果面板重复这一行。未配置时显示当前站点地址并注明。
[必须] 令牌对话框只把名称、有效期、分组、独立余额放在基础区；其余字段收进默认折叠的“高级设置”，折叠标题显示“已配置 N 项”。
[必须] 令牌子账户余额的管理员调整使用行内“调整余额”对话框，放在“划转”旁边；编辑对话框不含余额字段。
[必须] 用户行把用户名放在第一行，把角色与分组徽标放在第二行。
[必须] 用户的套餐管理使用行内“套餐”对话框；用户编辑对话框只含点“保存”才生效的字段。
[必须] 用户的状态开关在当前登录账户那一行禁用并说明原因；停用他人前确认，重新启用不确认。
[必须] 分组行把名称与默认徽标放在第一行，把描述放在第二行。
[必须] 分组的上移、下移按钮与编辑、删除按钮同组，排在编辑按钮之前。
[必须] 分组“位置”列显示 `index + 1` 与拖拽手柄，不显示 `sort_order` 原值；分组对话框不含排序字段，新分组追加到末尾，排序只用拖拽与上移/下移。
[必须] 分组“计费倍率”列右对齐、`tabular-nums`，写成 `1×`、`1.5×`；倍率只在分组页编辑，模型定价页不再有分组定价 Tab。
[必须] 新建分组默认“允许用户自选”= 关。
[必须] 删除分组前用已加载的套餐检查：有套餐的可用分组恰为该分组时，确认框列出这些套餐并链接到订阅套餐页，禁用“删除”。
[必须] 支付管理只在“支付渠道”Tab 激活时显示“添加支付渠道”。
[必须] 支付管理的 Tab 与订单筛选写入 URL：`?tab=<channels|orders>&status=&username=`。
[必须] 支付管理“退款”列只在已到账（`succeeded`）的行显示退款按钮，其余行显示“—”；退款确认框用键值块列出完整订单号（带 `CopyButton`）、支付渠道、实付金额与到账金额。
[必须] 支付渠道对话框使用 `<form>`，底部为 outline“取消”与提交“保存”；汇率与充值范围的错误显示在字段下方；已保存的 Stripe 支付渠道显示只读“回调地址”与 `CopyButton`。
[必须] `recharge_public_origin` 为空时，支付渠道 Tab 顶部显示 `Alert`“未设置充值公网 Origin，用户无法下单”，并链接到系统设置对应项。
[必须] 套餐行把名称放在第一行，把描述放在第二行；限额每个已配置窗口一行，窗口标签保留 `5h`、`24h`、`7d`、`30d` 原文。
[必须] 套餐行内的文字左对齐；不居中。
[必须] 套餐对话框的限额区标题写“滑动窗口限额（至少填写一项，USD）”，输入框不写“可选”；名称、可用分组、用量倍率标 `*`；价格行上方有“价格（USD） / 有效期（天）”列标签，有效期只接受正整数天。
[必须] 模型定价列表中，状态为启用但缺少必填价格（MP-R3）的行显示 `StatusBadge variant="warning"`“不完整”。
[必须] 定价编辑器新建时输入已有模型 ID，在字段下方提示“该模型已有定价”并给出“改为编辑”，禁用“保存”。
[必须] 工具价格有修改时显示与设置页相同的 `SettingsSaveBar`；切换 Tab 保留草稿，离开路由前确认。
[必须] 上游同步卡片只放“预览”；“应用 N 项变更”只在预览对话框底部。预览默认只列变更（`SegmentedControl`：变更 / 删除 / 跳过），更新行写 `字段: 旧值 → 新值`；含删除时按钮用 `destructive`，并用确认框写明删除条数。卡片摘要写“新增 n · 更新 n · 删除 n”。

## 3.3 钱包页

[必须] 按以下顺序组成页面：`PageHeader`（标题与说明，无操作）、余额摘要、下划线 Tab、Tab 面板。
[必须] 余额摘要保留唯一的主要操作“充值”。
[必须] 资金记录面板的卡片头只放“充值订单 / 余额流水”切换，不重复“资金记录”标题与说明。
[必须] 资金记录子 Tab 写入 URL：`?activity=<orders|ledger>`（默认 `orders` 时省略），与 `?tab=` 并存。
[必须] 余额流水主单元格第二行用次要文字显示上下文：备注、API 密钥名或“订单 {前 8 位}”，依次取第一个可用值。
[必须] 充值表单按“支付渠道（每项显示‘名称 · 币种 · $min–$max’）→ 预设金额 → 自定义金额”的顺序排列；切换支付渠道后金额越界时夹到最近边界并显示一行说明（RC-W3）。
[必须] 窄于 `lg` 时付款明细压成一行“实付 X 币种”，紧贴提交按钮上方。
[必须] 套餐价格按钮只打开购买确认框，确认框写明扣除金额、购买后余额、立即生效与到期时间；余额不足时禁用“确认购买”并提供“去充值”（BP-UI4）。
[建议] 概览与充值面板其余结构保持不变（`spec/recharge-system.spec.md` RC-W2a、RC-W3、RC-W7）。

## 3.4 Provider 页（列表与编辑器）

[必须] 页面标题为“Provider”，说明写明路由模型：按列表顺序尝试 Provider，同一 Provider 内按权重随机选择 Channel，失败后回退；有 Provider 绑定分组时补一句分组优先级先于此顺序。
[必须] 卡片头不放装饰图标底板；名称下方固定一行次要文字“第 N 位 · 尝试全部 Channel / 最多 N 次”，不用方括号，不显示 0 基数字或 -1。
[必须] 卡片头徽标顺序：不可用、部分可用、未定价（完整显示，计数为 0 时不显示）→ 协议 → 分组（可折叠进 +N）。禁用的 Provider 名称旁显示“已禁用”，指标与模型区降低对比度，操作按钮不降。
[必须] 只有拖拽手柄可拖动；手柄是可聚焦按钮。展开控制是带 `aria-expanded` 的真实按钮。
[必须] 在桌面开关左侧放置 RPM、TPM 两行文字。
[必须] 让所有 Provider 的指标列拥有相同宽度和相同左边缘。
[必须] Channel 行使用固定列栅格：有效状态徽标、名称、Base URL、“N 个模型 · 权重 W（P%）”、启用开关、测试操作。窄屏（< `sm`）分两行：第一行状态与名称，第二行文字、开关与测试。
[必须] Channel 只显示一个有效状态徽标，优先级：已禁用 > 不接收流量（权重 0）> 不健康 > 探测中 > 健康。
[必须] 列表中的 Provider、Channel 开关立即生效；Provider 启停给出带“撤销”的 toast。编辑器内的开关只改草稿，文案为“保存后启用/保存后禁用”。
[必须] 编辑器校验错误显示在字段下方并聚焦字段，分区栏与 Channel 行显示错误点；不只用 toast。
[必须] 文案不出现 `null`、`W:`、`5M` 之类缩写或内部阶段名。
[建议] 在窄屏中将指标移至身份信息下方。决策来源：窄屏不得横向溢出。

## 3.5 目录页（模型广场）

[必须] 按以下顺序组成页面：`PageHeader`（标题与说明，无操作、无徽标）、工具栏行（左侧搜索，右侧“显示 x / y 个模型”）、`DataTableShell` 包裹的 `DataList`。
[必须] 宽模式列依次为：模型、模式、输入价格、输出价格、上下文、最大输出。
[必须] 模型单元格第一行为 `ModelIcon`、等宽模型 ID 与复制按钮；第二行为 Provider 原文与“分组：…”的次要文字；普通用户自己的分组到不了该模型时，第三行写“需在 API 密钥中选择 {分组} 分组”。
[必须] 普通用户只看到自己分组（本人分组 ∪ 可自选分组）可达的模型；价格已乘实际计费分组的倍率（本人分组能到时用本人分组，否则用该行要求选择的分组）。行内倍率 ≠ 1 时，模型单元格以次要文字写“已按 {分组} 分组倍率 {n}× 计算”，工具栏单独一行说明，并提示还会乘以 Channel 倍率。管理员看到全部模型与基础价。
[必须] 阶梯计费显示第 1 档价格加次要文字“起 · n 档”，Tooltip 列出全部档位与 Fast 价格；按次计费在输入列写“$x / 次”，输出列写“—”。
[必须] 目录只读；唯一的行内操作是复制模型 ID。
[建议] 用对齐的列表而不是卡片网格呈现目录。决策来源：基线中跨列网格让前 7 张卡片的“输入价格”左边缘落在 5 个不同位置（`artifacts/redesign-2026-09-24/measurements-2.md`），读者无法逐行比较价格。

## 3.6 概览页（系统仪表盘）

[必须] 按以下顺序组成页面：`PageHeader`（操作区为“刷新”按钮与“10 秒自动刷新”次要文字）、渠道健康区块（全宽）、下方两列区块。
[必须] 视口 ≥ `lg` 时，下方左列占 7/12，放用户使用排名；右列占 5/12，依次放系统状态与从机状态。两列各自向下堆叠，互不对齐高度。
[必须] 视口 < `lg` 时单列顺序为：渠道健康、用户使用排名、系统状态、从机状态。
[必须] 每个区块使用 `Card`：`CardHeader` 放 `CardTitle` 与一句 `CardDescription`，内容放 `CardContent`。
[必须] 渠道健康区块头左侧放标题与“N 个 Channel · M 个异常”，M > 0 时“M 个异常”可点击并开启过滤；右侧放“仅看异常 (M)”切换与消费窗口分段控件；下一行放窗口消费总额、调用次数与滚动窗口说明。
[必须] 消费窗口只有 `24h`、`7d`、`30d`，同时作用于渠道健康与用户使用排名，写入 URL `?window=`。排名标题不写窗口值，空状态写明窗口。
[必须] 渠道健康行按状态严重度排序：不健康 → 冷却中 → 已停用 → 健康，同级内保持 Provider 优先级。来源：25 个 Channel 时，异常行排在第 21 行，桌面端要滚动约 1343px。
[必须] 仪表盘直达处置页面：Channel 名链接到 `/dashboard/providers?provider=&channel=`；不健康与冷却中的行带“查看日志”幽灵图标按钮，链接到 `/dashboard/logs?channel_id=&status=error`；排名用户名链接到 `/dashboard/logs?username=`。
[必须] 窄屏渠道健康行 ≤ 2 行文字：第一行 Channel 名、状态徽标、日志按钮；第二行 Provider、消耗、调用次数。权重、亲和、最近探测只在宽模式显示。来源：展开全部键值对时每行约 275px，排名被推到 6000px 以下。
[必须] 系统状态与从机状态使用键值行：每行左侧标签、右侧值，行间 `divide-y`。
[必须] 页面不使用固定页脚；原页脚的健康状态条目与亲和绑定数并入系统状态。

## 3.7 设置页（系统设置）

[必须] 按以下顺序组成页面：`PageHeader`（标题与说明，无操作）、设置主体。
[必须] 设置主体宽度 ≥ 56rem 时分两列：左列 13rem，依次放搜索框与分类导航，并在 `<main>` 内 `sticky top-0`；右列放分类区块与保存条。宽度 < 56rem 时单列：搜索框、单行横向滚动的分类导航、分类区块、保存条。
[必须] 首屏同时出现搜索框、全部分类入口（宽屏）和当前分类的第一组设置项。
[必须] 分类按管理意图组织，固定 8 类，依次为：站点（含首页性能目标）、访问与会话、模型映射、全局变换、路由与可靠性、计费、隐私与抓包、协议兼容。来源：原“健康监测”分类混入计费、隐私、超时和首页配置，高 2368px，按分类找不到这些设置。
[必须] 分类导航只写分类名；不加序号、图标或描述。分类含错误时，分类名旁显示错误圆点。
[必须] 当前分类写入 URL `?category=`；`?setting=<设置项 id>` 选中所在分类、滚动到 `#setting-<id>` 并高亮 2 秒。设置项 id 是对外约定，不得改名。
[必须] 分类区块依次为：`h2` 分类名、一句分类说明、各分组。分组有标题时，标题是分组 `Card` 上方的 `h3`，需要时在标题下放一句分组说明。
[必须] 允许注册且关闭人机验证时，访问与会话分类顶部显示警告 `StatusAlert`。
[必须] 表单类分组把设置项放在一张 `Card` 内，行间 `divide-y`；编辑器类分组（Codex 模型、推理后缀、模型重定向、全局变换、首页性能目标）不加 `Card`，因为编辑器自带边框。
[必须] 数值设置使用 `NumberField`：输入过程中不改写用户输入，越界值在行内报错并阻止保存；输入框后显示单位，说明中写明默认值与最小值。列表设置使用 `ListField`。来源：受控回填让管理员无法“清空后重输”，逗号被吞掉后保存了不存在的字段名。
[必须] 设置行：开关行左侧放标签与说明，右侧放 `Switch`；其他行在分组宽度 ≥ 40rem 时分两列 `minmax(0,1fr) 20rem`，控件在右列，窄时上下堆叠。
[必须] 数值字段每行一个；不再排成 2–3 列网格。原因：多列网格让说明文字高度不同，标签与输入框无法逐行对齐。
[必须] 搜索在全部分类中逐项过滤设置行；结果行保留可编辑控件。导航显示各分类的匹配数，零匹配的分类按钮 `disabled`。
[必须] 保存条只在草稿与服务器值存在差异时出现，`sticky bottom-0`，放“{n} 项未保存的修改”文字、“查看更改”、“放弃修改”`outline` 按钮与“保存更改”默认按钮。它是本页唯一的实心主要操作。来源：基线在 1440 × 900 下健康监测分类高 2100px，页首保存按钮在编辑底部字段时不可见。
[必须] “查看更改”用 `Popover` 逐项列出“设置项：旧值 → 新值”，点击跳到该设置项。
[必须] 保存前对高风险变更（开放注册、关闭人机验证、关闭脱敏、开启免费结算、开启请求抓包）弹 `AlertDialog` 列出风险，确认按钮用 `destructive`。
[必须] 有未保存修改时，离开页面必须二次确认。
[必须] 保存失败时跳到出错设置项，行内显示错误，toast 只写本地化文案，不显示服务器原始错误。
[建议] 不为分类区块再加卡片或大号衬线标题。来源：基线 36px 衬线分类标题与导航中已选分类名重复。

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
|设置分类标题|`text-lg font-semibold`|设置页分类区块 `h2`|
|设置分组标题|`text-sm font-semibold`|分组 `Card` 上方的 `h3`|
|设置项标签|`Label`：`text-sm font-medium`|设置行|
|设置项说明|`text-sm text-muted-foreground`|设置行标签下方|
|设置导航项|`text-sm`；当前项 `font-medium text-foreground`|分类导航|

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
[建议] 属性类标签（流式/非流、协议、类型、角色）使用中性 `bg-muted text-muted-foreground` 或 `Badge variant="secondary"`；状态色只表达结果或风险。一个条目只显示一个有效状态，优先级：已停用 > 不接流量 > 不健康 > 健康。
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
[必须] 行内边距使用 `DataListRow` 内置的 `px-4 py-3`；页面不覆盖。宽模式下每个单元格只有一行文字的列表设置 `DataList dense`，宽模式行内边距变为 `py-1.5`（行高 48px）；窄于宽模式时仍为 `py-3`。
[必须] 操作列宽度与可选列由已加载的数据决定：按实际出现的最多按钮数计算宽度；某列在全部行中都只是默认值时不显示该列。
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
[必须] 横向滚动容器内的可聚焦元素加 `focus-visible:ring-inset`。原因：`overflow-x-auto` 同时裁掉纵向溢出，外扩焦点环的外沿不可见。来源：设置页分类导航在 1024 × 800 下的键盘焦点检查。适用：所有横向滚动的导航与 Tab 行。
[必须] 首次加载显示与就绪布局相同结构的骨架；后台刷新时保留缓存数据。
[必须] 空列表使用 `EmptyState`，并在有权限时提供创建操作。
[必须] 不可执行的操作使用 `disabled`，并用 `title` 或提示说明原因。
[必须] 令牌过期时，过期时间单元格显示 `StatusBadge variant="warning"`“已过期”。
[必须] 紧跟标识符的复制按钮使用 `CopyButton`；复制成功后图标切换为 `Check` 2 秒，失败时显示 `common.copyFailed` 错误提示，成功时不弹提示。
[必须] 分段控件的每个按钮设 `aria-pressed`；选中项使用 `bg-foreground text-background font-semibold`，与未选中项的非文本对比度 ≥ 3:1。来源：基线选中项与未选中项背景只差 `rgb(245,245,245)` 对 `rgb(250,250,250)`（`/dashboard`，1440 × 900，light）。
[必须] 首次加载失败使用 `QueryError`；有缓存时在缓存数据上方显示 `QueryError stale`，不显示原始错误信息。
[必须] 先判断加载错误，再判断空数据；加载失败不显示空状态。403 显示 `QueryError forbidden`（无重试按钮）；管理页的无权限状态由路由统一渲染，页面不自建。
[必须] 有未保存修改的编辑器对话框向 `DialogContent` 传 `dirty`：点外部、Esc 和关闭按钮不关闭，改为询问“放弃未保存的修改？”（继续编辑 / 放弃）。页面级草稿渲染 `useUnsavedChangesGuard(dirty)` 返回的节点。对话框关闭后，焦点回到打开它的元素。
[必须] 校验错误显示在字段下方：`FieldError` 加控件 `aria-invalid`，保存时跳到第一个错误字段所在分区并聚焦。toast 只用于与字段无关的服务端错误，并通过 `toastApiError` 显示 i18n 文案。
[必须] 数值用 `NumberField`、列表用 `ListField`：键入过程中不改写用户输入；失焦或按 Enter 时校验，越界时内联报错并阻止保存；单位写成输入框后的后缀。金额以 USD 十进制输入。
[必须] Tab 写入 `?tab=`，筛选写入 search params，均用 replace，等于默认值时移除。
[必须] 扣款、批量写库、删除、停用他人之前，用 `AlertDialog` 写出具体后果（金额、条数、受影响的密钥数）。可逆的立即生效操作不弹确认，改为带“撤销”的 toast。
[必须] 实体编辑器用 `<form onSubmit>` 包裹字段，主要按钮为 `type="submit"`，单行输入框内按 Enter 即提交；`Label` 用 `htmlFor` 绑定 `useId` 生成的 id。
[必须] 动效包裹层（`AnimatedButton`、品牌标识）不成为 Tab 停靠点。
[必须] toast 显示在顶部居中，不遮挡对话框底栏和吸底保存条。
[必须] “已自定义”“筛选生效”标记使用 `ModifiedDot`，相邻控件的可访问名称说明该状态。

## 4.7 动效

[必须] 沿用 `PageWrapper` 的页面进入动效和 `AnimatedButton` 的按钮缩放。
[必须] 钱包资金记录行保留 RC-W8 的弹簧进入动效。
[必须] 管理列表行不添加进入位移或缩放动效。
[必须] 设置页保存条与分类切换只使用透明度与 ≤ 8px 纵向位移。
[必须] 减少动效时只允许透明度变化。

# 5. 可用原语

| 角色 | 实现名称 | 来源或路径 | 使用条件 | 状态 |
|---|---|---|---|---|
|页面容器|`PageWrapper`|`frontend/src/components/ui/motion.tsx`|每页根节点|已实现|
|页面标题区|`PageHeader`（`title`、`description`、`actions`）|`frontend/src/components/ui/page-header.tsx`|每页唯一标题|已实现|
|列表外壳|`DataTableShell`（`toolbar`、`isEmpty`、`emptyState`）|`frontend/src/components/ui/data-table-shell.tsx`|管理列表表面|已实现|
|工具栏搜索|`TableToolbarSearch`|同上|令牌搜索|已实现|
|响应式列表|`DataList`（`columns`、`dense`）、`DataListHeader`、`DataListHead`（`align`）、`DataListBody`、`DataListRow`（`asChild`）、`DataListCell`（`label`、`align`、`primary`）、`DataListActions`|`frontend/src/components/ui/data-list.tsx`|管理列表页与目录页的全部列表|已实现|
|滚动容器|`DashboardScrollParentContext`|`frontend/src/lib/dashboard-scroll.ts`（由 `pages/layout.tsx` 提供）|`Virtuoso` 的 `customScrollParent`|已实现|
|虚拟列表|`Virtuoso`|`react-virtuoso`|可能超过 100 行的列表：令牌、用户、模型定价、未定价模型|已实现|
|空状态|`EmptyState`（`card`、`inline`）|`frontend/src/components/ui/empty-state.tsx`|空列表|已实现|
|页面骨架|`TablePageSkeleton`、`PageHeaderSkeleton`、`Skeleton`|`frontend/src/components/ui/page-skeleton.tsx`、`skeleton.tsx`|首次加载|已实现|
|按钮|`Button`（`default`、`outline`、`ghost`、`destructive`）、`AnimatedButton`|`frontend/src/components/ui/button.tsx`、`motion.tsx`|操作|已实现|
|开关与选择|`Switch`、`Checkbox`、`Select`、`Input`|`frontend/src/components/ui/`|表单与行内控件|已实现|
|徽标|`Badge`、`StatusBadge`、`OrderStatusBadge`、`GroupsBadge`、`BadgeOverflowList`|`components/ui/badge.tsx`、`status.tsx`、`components/recharge/`、`components/`|状态与限制|已实现|
|区块卡片|`Card`、`CardHeader`、`CardTitle`、`CardDescription`、`CardContent`|`frontend/src/components/ui/card.tsx`|系统仪表盘区块、钱包资金记录|已实现|
|紧凑表|`Table`、`TableHeader`、`TableBody`、`TableRow`、`TableHead`、`TableCell`|`frontend/src/components/ui/table.tsx`|≤ 4 列且放得进 20rem 的表|已实现|
|虚拟列表适配|`virtualDataListComponents`|`frontend/src/components/ui/data-list-virtual.tsx`|`Virtuoso` 的 `components`；列表 `aria-label` 通过 `context.label` 传入|已实现|
|加载失败|`QueryError`（`onRetry`、`retrying`、`stale`、`forbidden`）|`frontend/src/components/ui/query-error.tsx`|数据加载或刷新失败|已实现|
|模型标识|`ModelIcon`|`frontend/src/components/ModelIcon.tsx`|模型广场模型 ID 前|已实现|
|分段选择|`SegmentedControl`（`value`、`options`、`onChange`、`ariaLabel`、`label`）|`frontend/src/components/ui/segmented-control.tsx`|时间窗口、对话框内视图切换（2–6 个固定短选项）；选中项实心反色|已实现|
|复制|`CopyButton`（`value`、`label`）、`useCopyToClipboard`|`frontend/src/components/ui/copy-button.tsx`、`frontend/src/hooks/use-copy-to-clipboard.ts`|紧跟标识符的复制按钮；带文字的复制按钮|已实现|
|删除确认|`ConfirmDeleteDialog`（`open`、`title`、`description`、`onConfirm`、`onOpenChange`）|`frontend/src/components/ui/confirm-delete-dialog.tsx`|所有删除持久化条目的操作|已实现|
|模型多选|`ModelMultiSelect`（`value`、`options`、`loading`、`error`、`onRetry`、`onChange`、`allowCustom`）|`frontend/src/components/models/model-multi-select.tsx`|编辑精确模型 ID 集合|已实现|
|模型重定向|`ModelRedirectsEditor`|`frontend/src/components/settings/model-redirects-editor.tsx`|全局与令牌模型重定向|已实现|
|时间格式|`formatDateTime`、`formatDate`|`frontend/src/lib/format-time.ts`|全部时间戳与日期|已实现|
|提示|`TooltipProvider`、`Tooltip`、`TooltipTrigger`、`TooltipContent`|`frontend/src/components/ui/tooltip.tsx`|完整订单号、禁用原因、指标说明|已实现|
|切换|`Tabs`、`TabsList`、`TabsTrigger`、`TabsContent`|`frontend/src/components/ui/tabs.tsx`|支付管理、钱包|已实现|
|对话框|`Dialog`、`DialogContent`（`dirty`）、`AlertDialog`|`frontend/src/components/ui/`|编辑与破坏性确认；`dirty` 时拦截关闭并询问放弃|已实现|
|离开保护|`useUnsavedChangesGuard(dirty)`|`frontend/src/hooks/use-unsaved-changes-guard.tsx`|页面级草稿；渲染返回的节点|已实现|
|字段|`Field`、`FieldLabel`、`FieldDescription`、`FieldError`|`frontend/src/components/ui/field.tsx`|表单字段与内联错误|已实现|
|数值输入|`NumberField`（`id`、`value`、`onValueChange`、`min`、`max`、`integer`、`allowEmpty`、`unit`、`onValidityChange`）|`frontend/src/components/ui/number-field.tsx`|全部数值输入|已实现|
|列表输入|`ListField`（`id`、`value`、`onValueChange`、`separator`）|`frontend/src/components/ui/list-field.tsx`|逗号或换行分隔的字符串列表|已实现|
|请求错误|`apiErrorMessage`、`toastApiError`、`apiErrorParam`|`frontend/src/lib/api-error.ts`|按 `apiErrors.<code>` 显示错误；按 `param` 定位字段|已实现|
|URL 状态|`useSearchParamState(key, default, allowed)`|`frontend/src/hooks/use-search-param-state.ts`|Tab 与单值筛选|已实现|
|面板容错|`PanelErrorBoundary`（`title`）|`frontend/src/components/ui/error-boundary.tsx`|页面内相互独立的面板|已实现|
|修改标记|`ModifiedDot`|`frontend/src/components/ui/modified-dot.tsx`|“已自定义”“筛选生效”|已实现|
|颜色与字体|语义 token、`font-display`、`font-mono`|`frontend/src/index.css`、`tailwind.config.cjs`|全部页面|已实现|
|设置模型|`SETTINGS_CATEGORIES`、`SETTINGS_GROUPS`、`SETTING_ENTRIES`|`frontend/src/components/settings/settings-categories.ts`|设置分类、分组与可搜索设置项的唯一来源|已实现|
|设置搜索|`SettingsSearchContext`、`matchSettingEntries`、`normalizeSettingsQuery`|`frontend/src/components/settings/settings-search.ts`|过滤设置行|已实现|
|设置布局|`SettingsBody`、`SettingsCategorySection`、`SettingsGroup`、`SettingRow`（`switchControl`）、`SettingBlock`|`frontend/src/components/settings/settings-layout.tsx`|设置页分类区块、分组、设置行与编辑器|已实现|
|设置导航|`SettingsCategoryNav`|`frontend/src/components/settings/settings-category-nav.tsx`|设置页分类导航|已实现|
|保存条|`SettingsSaveBar`|`frontend/src/components/settings/settings-save-bar.tsx`|设置页未保存修改|已实现|
|取数|`useApiKeys`、`usePaymentChannels`、`useRechargeOrders`、`useDashboardGroups`、`useProviders`、`useBillingPlans`、`useMarketplaceModels`、`useAdminOverview`、`useSettings`、`useTransformRegistry` 与对应乐观更新函数|`frontend/src/lib/swr.ts`|数据与变更|已实现|

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
[必须] 不使用“服务提供者”“提供者”“通道”或单独的“渠道”指代 Provider、Channel；支付收款配置一律写“支付渠道”。
[必须] 导航文字与页面 `h1` 一一对应（`spec/dashboard-ui-layout.spec.md` DL5a）：API 密钥、钱包、请求日志、模型广场、系统仪表盘、Provider、分组、自定义变换、模型定价、订阅套餐、支付管理、用户管理、系统设置；账户页为“账户设置”。zh-TW 写“API 金鑰”，ja 写“API キー”。
[必须] 令牌的分组模式写“跟随账户分组”。
[必须] 界面不显示内部 id（`mono_prov_…`、设置键名、Rust 或 SDK 错误类名）；先解析成名称再显示。
[必须] 错误提示只显示 `apiErrors.<code>` 或 `common.requestFailed` 的 i18n 文案，不显示服务端原始 `message`。
[必须] 页面说明写一句话，说明本页管理的对象。
[必须] 金额以 `$` 加后端规范字符串显示；余额使用 `formatUsdDecimal(value, 2)`；账本金额使用 `formatNanoUsd(value, 4)`。
[必须] 订单到账与支付金额使用 2 位；只有账本使用 4 位。
[必须] 汇率写成“1 USD = {rate} {currency}”。
[必须] 充值范围写成“$min – $max”。
[必须] 订单、账本、用户与定价记录的时间使用 `formatDateTime`；令牌过期日期与用户创建、登录日期使用 `formatDate`。
[必须] 订单号显示前 8 个字符，完整订单号与 `error_code` 放在提示中。紧跟一个 `CopyButton` 复制完整订单号。
[必须] API 密钥列表的独立余额列只在至少一个密钥开启独立余额时出现；未开启的行显示 `—`。不在任何行写“使用账户余额”。未设置过期时间的密钥显示次要文字“永不过期”。
[必须] 钱包错误状态不显示原始错误信息。
[必须] Provider 指标帮助文字说明最近 60 秒、已落库请求及输入加输出 token。
[必须] 模型价格（模型定价列表与模型广场共用一个格式函数）显示精确十进制：去掉末尾 0 但至少保留 2 位小数（`$5.00`、`$0.60306`），不四舍五入；单位“每 1M tokens”写在表头与窄屏标签中，单元格不重复“/ 1M”。按次计费写“$x / 次”。
[必须] 倍率字段（分组计费倍率、套餐用量倍率、Channel 倍率）的帮助文字写出它参与的计算式与作用对象（余额扣费或套餐额度）。
[必须] 上下文与最大输出以 `K`、`M` 缩写显示；缺失显示 `—`。
[必须] 套餐限额显示为 `$` 加精确十进制金额（去掉末尾 0，例如 `$5`、`$0.005`），不四舍五入。
[必须] 套餐价格写成“$价格 / N 天”；时长不是整天时依次改用整小时、整分钟或秒，不四舍五入；没有价格时显示 `—`。
[必须] 系统仪表盘时间使用本地 `YYYY-MM-DD HH:mm:ss`；运行时长写成 `2d 4h 12m`；字节写成 `B`、`KB`、`MB`、`GB`。
[必须] 消费窗口标签 `24h`、`7d`、`30d` 在所有语言中保持原文。
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
[必须] 不在列表每一行重复显示默认状态的整句文字（如“使用账户余额”）；默认状态显示 `—` 或次要短词，只有非默认值使用正文颜色。
[必须] 不只用 toast 承载字段校验错误；toast 消失后错误不能随之消失。来源：Provider 编辑器保存失败只弹 toast，且 toast 盖住底栏（`artifacts/ux-audit-2026-10-30/findings-providers.md` PROVIDERS-3）。
[必须] 不在键入过程中拆分、回填或格式化输入。来源：设置页白名单逐键 split/join 吞掉逗号（ADMINOPS-1）。
[必须] 不把加载失败或无权限显示成空状态或“重试”。
[必须] 不让动效包裹层成为额外的 Tab 停靠点。

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
|系统设置中可单独搜索的最小单元|设置项（代码 `SettingEntry`）|
|设置项的一级归类|设置分类（代码 `SettingsCategory`）|
|个人账户页 `/settings`|账户设置|
|管理员系统配置页 `/dashboard/admin-settings`|系统设置|
|API 密钥沿用账户所属分组的模式|跟随账户分组|
|后端错误码对应的本地化文案|`apiErrors.<code>`|
