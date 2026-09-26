# Model Marketplace Page Specification

## 1. Purpose

The Model Marketplace page presents all registered model metadata to logged-in dashboard users in a read-only, searchable catalog list. It differs from the Model Database (admin-only, CRUD) in that it exposes no mutation controls and is accessible to every authenticated role (`user`, `admin`, `super_admin`).

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
├── PageHeader
│   ├── h1: page title (display font)
│   └── p: page description
├── QueryError (when the fetch failed; MM-ERR1, MM-ERR2)
└── DataTableShell (when data exists)
    ├── toolbar
    │   ├── TableToolbarSearch (inline start)
    │   └── p: filtered and total model counts (inline end)
    ├── EmptyState card (when the catalog is empty)
    └── DataList (when the catalog is non-empty)
        ├── inline EmptyState (when zero records match the query)
        └── one DataListRow per filtered record
```

MM-UI1. The page header MUST NOT contain actions or badges. The title MUST use the shared display font (`frontend-design-system.spec.md` DS38).

MM-UI2. The result count MUST render as muted plain text, not as a badge.

### 4.2 Row Content

Each row MUST render through the `DataList` primitives (`frontend-design-system.spec.md` §6.1). In wide mode, the columns MUST be, in order:

| Column | Label key | Data accessor | Format | Alignment |
| ------ | --------- | ------------- | ------ | --------- |
| Model | `modelMarketplace.modelId` | `record.model_id`, `record.models_dev_provider` | Line 1: model icon, complete model ID in monospace, copy button. Line 2: provider as muted text; em dash when absent | start |
| Mode | `modelMarketplace.mode` | `record.mode` | Plain text; em dash when absent | start |
| Input | `modelMarketplace.inputPerMillion` | `record.input_usd_per_1m` | `$X` with 3 fractional digits; em dash when null | end |
| Output | `modelMarketplace.outputPerMillion` | `record.output_usd_per_1m` | `$X` with 3 fractional digits; em dash when null | end |
| Context | `modelMarketplace.context` | `record.max_tokens` | Human-readable, for example `128K` or `1M`; em dash when null | end |
| Max output | `modelMarketplace.maxOutput` | `record.max_output_tokens` | Human-readable, for example `16K`; em dash when null | end |

MM-UI3. The model icon MUST render without a background plate and MUST be hidden from assistive technology.

MM-UI4. The copy button MUST copy the complete `model_id` to the clipboard. Its accessible name MUST include the model ID. After a successful copy, its icon MUST change to a check mark for 2 seconds. The copy button is the only row action.

### 4.3 List Contract

- The DOM order MUST equal the endpoint result order.
- In wide mode, every row MUST align to the same column tracks, so that the input price cells of all rows share one left edge.
- The page MUST render the finite result set without pagination or infinite scroll.
- The list MUST NOT establish its own scroll container (`frontend-design-system.spec.md` DS22j).

### 4.4 Search

- Single text input filters on `model_id` (case-insensitive `includes`)
- Debounce: none required (client-side filter on already-loaded data)

### 4.5 Loading State

When `isLoading` is true and no data exists, render inside `PageWrapper`: a page header skeleton, a toolbar skeleton, and a list surface with six row skeletons.

### 4.6 Empty State

When the successful catalog is empty:

- Icon: muted `Store` icon
- Primary text: `modelMarketplace.noModels`
- Secondary text: `modelMarketplace.noModelsDesc`

## 5. Invariants

1. The page MUST NOT expose any mutation controls (no create, edit, delete, sync buttons). Copying a model ID is not a mutation.
2. The page MUST use `useMarketplaceModels()` from `@/lib/swr` — which calls `GET /api/dashboard/marketplace/models`.
3. The backend endpoint MUST only return models present in at least one enabled Provider and at least one enabled Channel whose weight is greater than zero.
4. The page MUST render the catalog list defined in sections 4.2 and 4.3.
5. All user-visible strings MUST go through `t()` (i18next). Keys live under `modelMarketplace.*`.
6. Navigation entry MUST appear in the common `navItems` array (visible to all roles).

## 6. i18n Keys

Keys under `modelMarketplace`:

| Key                 | en                                                                           | zh                                             |
| ------------------- | ---------------------------------------------------------------------------- | ---------------------------------------------- |
| `title`             | Model Marketplace                                                            | 模型广场                                       |
| `description`       | Browse available models, pricing and specifications                          | 浏览可用模型、定价和规格                       |
| `searchPlaceholder` | Search models...                                                             | 搜索模型...                                    |
| `modelId`           | Model                                                                        | 模型                                           |
| `mode`              | Mode                                                                         | 模式                                           |
| `context`           | Context                                                                      | 上下文                                         |
| `maxOutput`         | Max Output                                                                   | 最大输出                                       |
| `resultCount`       | Showing {{filtered}} of {{total}} models                                     | 显示 {{filtered}} / {{total}} 个模型           |
| `noModels`          | No models available                                                          | 暂无可用模型                                   |
| `noModelsDesc`      | Model data will appear here once the administrator syncs the model database. | 管理员同步模型数据库后，模型数据将显示在此处。 |
| `inputPerMillion`   | Input (per 1M tokens)                                                        | 输入（每 1M tokens）                           |
| `outputPerMillion`  | Output (per 1M tokens)                                                       | 输出（每 1M tokens）                           |
| `copyModelId`       | Copy {{model}}                                                               | 复制 {{model}}                                 |

Nav key `nav.marketplace`: en = `Models`, zh = `模型广场`

## 7. Fetch Failure and Search States

MM-ERR1. A fetch failure without cached records MUST render DS54 failure feedback.
It MUST NOT render a zero result count or the no-models state. Hide catalog search
until data exists. Retry MUST revalidate the marketplace SWR key.

MM-ERR2. A refresh failure with cached records MUST preserve the searchable catalog
and show DS54 refresh-failure feedback. A pending retry MUST disable its button.

MM-ERR3. A successful empty catalog MUST use the no-models state. A non-empty catalog
with zero search matches MUST instead show a no-matches message and an action that
clears the query. Both states MUST use localized text.
