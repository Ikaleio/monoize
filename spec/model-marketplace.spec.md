# Model Marketplace Page Specification

## 1. Purpose

The Model Marketplace page presents all registered model metadata to logged-in dashboard users in a read-only, searchable card catalog. It differs from the Model Database (admin-only, CRUD) in that it exposes no mutation controls and is accessible to every authenticated role (`user`, `admin`, `super_admin`).

## 2. Routing

| Property           | Value                         |
| ------------------ | ----------------------------- |
| Path               | `/dashboard/marketplace`      |
| Parent layout      | `DashboardLayout`             |
| Auth required      | Yes (any role)                |
| Navigation section | Common (non-admin) `navItems` |

## 3. Data Source

- Uses a dedicated `GET /api/dashboard/marketplace/models` endpoint (via the `useMarketplaceModels()` SWR hook).
- This endpoint requires login (any role) but NOT admin.
- Server-side: returns metadata whose model ID is offered by at least one enabled Provider through an enabled Channel whose weight is greater than zero.
- The endpoint MUST obtain the metadata result through one set-based query that joins `model_metadata_records`, `monoize_channel_models`, `monoize_channels`, and `monoize_providers`.
- The metadata query MUST select only metadata columns, MUST use `DISTINCT`, and MUST order results by `model_id ASC`.
- Each returned row MUST additionally carry `input_usd_per_1m` and `output_usd_per_1m` from the enabled `model_prices` row with the same `model_id` (`model-pricing.spec.md` §2.1), or `null` for each field when no enabled row exists.
- The endpoint MUST NOT hydrate Provider or Channel objects and MUST NOT return a Provider or Channel secret.
- The page renders the filtered `MarketplaceModelRecord[]` array.

## 4. UI Structure

### 4.1 Page Shell

```
PageWrapper
├── motion.div (header)
│   └── PageHeader
│       ├── h1: page title
│       ├── p: page description
│       └── Badge: filtered and total model counts
├── Search Field
│   ├── sr-only label
│   └── Input
└── motion.div (catalog, delay=0.1)
    ├── EmptyState (when filtered.length === 0)
    └── section (when filtered.length > 0)
        └── ModelMarketplaceCard[]
```

### 4.2 Card Content

Each card MUST use `Card`, `CardHeader`, `CardContent`, `Separator`, and
`CardFooter`. The card MUST render the following fields.

| Region  | Label key                     | Data accessor                | Format                                     |
| ------- | ----------------------------- | ---------------------------- | ------------------------------------------ |
| Header  | `modelMarketplace.modelId`    | `record.model_id`            | Model icon plus complete model ID          |
| Header  | `modelMarketplace.mode`       | `record.mode`                | Outline Badge; em dash when absent         |
| Header  | `modelMarketplace.provider`   | `record.models_dev_provider` | Text; em dash when absent                  |
| Content | `modelMarketplace.inputCost`  | `record.input_usd_per_1m`    | `$X / 1M`; em dash when null               |
| Content | `modelMarketplace.outputCost` | `record.output_usd_per_1m`   | `$X / 1M`; em dash when null               |
| Footer  | `modelMarketplace.context`    | `record.max_tokens`          | Human-readable, for example `128K` or `1M` |
| Footer  | `modelMarketplace.maxOutput`  | `record.max_output_tokens`   | Human-readable, for example `16K`          |

### 4.3 Non-linear Grid Contract

- The DOM order MUST equal the endpoint result order.
- At viewport widths below `768px`, the grid MUST contain one column.
- At viewport widths from `768px` through `1023px`, the grid MUST contain two
  equal columns. Items at repeating-pattern positions 0, 5, and 6 MUST span both
  columns. Other items MUST span one column.
- At viewport widths at or above `1024px`, the grid MUST contain 12 equal
  columns. Card spans MUST repeat in this order: `7, 5, 4, 4, 4, 5, 7`.
- The layout MUST NOT use CSS dense packing. Visual order MUST equal DOM order.
- The page MUST render the finite result set without pagination or infinite scroll.

### 4.4 Search

- Single text input filters on `model_id` (case-insensitive `includes`)
- Debounce: none required (client-side filter on already-loaded data)

### 4.5 Loading State

When `isLoading` is true, render:

```
Page title and description skeletons
Search field skeleton
Seven card skeletons using the same responsive span pattern as the catalog
```

### 4.6 Empty State

When `filtered.length === 0`:

- Icon: muted `Store` icon (or `Database`)
- Primary text: `modelMarketplace.noModels`
- Secondary text: `modelMarketplace.noModelsDesc`

## 5. Invariants

1. The page MUST NOT expose any mutation controls (no create, edit, delete, sync buttons).
2. The page MUST use `useMarketplaceModels()` from `@/lib/swr` — which calls `GET /api/dashboard/marketplace/models`.
3. The backend endpoint MUST only return models present in at least one enabled Provider and at least one enabled Channel whose weight is greater than zero.
4. The page MUST render the card grid defined in sections 4.2 and 4.3.
5. All user-visible strings MUST go through `t()` (i18next). Keys live under `modelMarketplace.*`.
6. Navigation entry MUST appear in the common `navItems` array (visible to all roles).

## 6. i18n Keys

Keys to add under `modelMarketplace`:

| Key                 | en                                                                           | zh                                             |
| ------------------- | ---------------------------------------------------------------------------- | ---------------------------------------------- |
| `title`             | Model Marketplace                                                            | 模型广场                                       |
| `description`       | Browse available models, pricing and specifications                          | 浏览可用模型、定价和规格                       |
| `searchPlaceholder` | Search models...                                                             | 搜索模型...                                    |
| `modelId`           | Model                                                                        | 模型                                           |
| `mode`              | Mode                                                                         | 模式                                           |
| `inputCost`         | Input Cost                                                                   | 输入价格                                       |
| `outputCost`        | Output Cost                                                                  | 输出价格                                       |
| `context`           | Context                                                                      | 上下文                                         |
| `maxOutput`         | Max Output                                                                   | 最大输出                                       |
| `provider`          | Provider                                                                     | 提供者                                         |
| `resultCount`       | Showing {{filtered}} of {{total}} models                                     | 显示 {{filtered}} / {{total}} 个模型           |
| `noModels`          | No models available                                                          | 暂无可用模型                                   |
| `noModelsDesc`      | Model data will appear here once the administrator syncs the model database. | 管理员同步模型数据库后，模型数据将显示在此处。 |

Nav key `nav.marketplace`: en = `Models`, zh = `模型广场`
