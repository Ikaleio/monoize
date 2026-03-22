# Model Marketplace Page Specification

## 1. Purpose

The Model Marketplace page presents all registered model metadata to logged-in dashboard users in a read-only, searchable, filterable virtual table. It differs from the Model Database (admin-only, CRUD) in that it exposes no mutation controls and is accessible to every authenticated role (`user`, `admin`, `super_admin`).

## 2. Routing

| Property | Value |
|----------|-------|
| Path | `/dashboard/marketplace` |
| Parent layout | `DashboardLayout` |
| Auth required | Yes (any role) |
| Navigation section | Common (non-admin) `navItems` |

## 3. Data Source

- Reuses the existing `GET /api/dashboard/model-metadata` endpoint (via the `useModelMetadata()` SWR hook).
- No new backend endpoint is introduced.
- The page renders the full `ModelMetadataRecord[]` array returned by the hook.

## 4. UI Structure

### 4.1 Page Shell

```
PageWrapper
├── motion.div (header)
│   ├── h1: page title
│   └── p: page description (muted)
└── motion.div (card, delay=0.1)
    └── Card
        ├── CardHeader
        │   ├── CardTitle + icon
        │   ├── Search input (right-aligned, 64-wide)
        │   └── Filter controls (provider, mode) — optional inline selects
        └── CardContent
            ├── Empty state (when filtered.length === 0)
            └── TableVirtuoso (when filtered.length > 0)
```

### 4.2 Table Columns

| # | Header key | Data accessor | Cell format | Min width |
|---|-----------|--------------|-------------|-----------|
| 1 | `modelMarketplace.modelId` | `record.model_id` | `<ModelBadge>` with provider icon | 200px |
| 2 | `modelMarketplace.mode` | `record.mode` | Badge (`chat` / `embedding` / …) | — |
| 3 | `modelMarketplace.inputCost` | `record.input_cost_per_token_nano` | `$X.XXXX / 1M` (formatted from nano) | — |
| 4 | `modelMarketplace.outputCost` | `record.output_cost_per_token_nano` | `$X.XXXX / 1M` (formatted from nano) | — |
| 5 | `modelMarketplace.context` | `record.max_tokens` | Human-readable (e.g. `128K`, `1M`) | — |
| 6 | `modelMarketplace.maxOutput` | `record.max_output_tokens` | Human-readable (e.g. `16K`) | — |
| 7 | `modelMarketplace.provider` | `record.models_dev_provider` | Text, lowercase | — |

### 4.3 Virtualized Table Contract

- Component: `TableVirtuoso` from `react-virtuoso`
- `style.height`: `calc(100vh - 280px)`, `minHeight: 400`
- Custom `components`: `Table`, `TableHead`, `TableRow`, `TableBody` matching existing shadcn table classes
- `fixedHeaderContent`: renders `<tr>` with `<th>` elements, sticky via `bg-background`
- `itemContent`: renders `<td>` cells per column spec above
- No `endReached` / infinite scroll (dataset is finite and fully loaded)

### 4.4 Search

- Single text input filters on `model_id` (case-insensitive `includes`)
- Debounce: none required (client-side filter on already-loaded data)

### 4.5 Loading State

When `isLoading` is true, render:
```
<Skeleton h-8 w-48 />
<Skeleton h-12 w-full />
<Skeleton h-64 w-full />
```

### 4.6 Empty State

When `filtered.length === 0`:
- Icon: muted `Store` icon (or `Database`)
- Primary text: `modelMarketplace.noModels`
- Secondary text: `modelMarketplace.noModelsDesc`

## 5. Invariants

1. The page MUST NOT expose any mutation controls (no create, edit, delete, sync buttons).
2. The page MUST reuse `useModelMetadata()` from `@/lib/swr` — no duplicate fetcher.
3. The page MUST use `TableVirtuoso` with the same component override pattern as `model-metadata.tsx`.
4. All user-visible strings MUST go through `t()` (i18next). Keys live under `modelMarketplace.*`.
5. Navigation entry MUST appear in the common `navItems` array (visible to all roles).

## 6. i18n Keys

Keys to add under `modelMarketplace`:

| Key | en | zh |
|-----|----|----|
| `title` | Model Marketplace | 模型广场 |
| `description` | Browse available models, pricing and specifications | 浏览可用模型、定价和规格 |
| `searchPlaceholder` | Search models... | 搜索模型... |
| `modelId` | Model | 模型 |
| `mode` | Mode | 模式 |
| `inputCost` | Input Cost | 输入价格 |
| `outputCost` | Output Cost | 输出价格 |
| `context` | Context | 上下文 |
| `maxOutput` | Max Output | 最大输出 |
| `provider` | Provider | 提供者 |
| `noModels` | No models available | 暂无可用模型 |
| `noModelsDesc` | Model data will appear here once the administrator syncs the model database. | 管理员同步模型数据库后，模型数据将显示在此处。 |

Nav key `nav.marketplace`: en = `Models`, zh = `模型广场`
