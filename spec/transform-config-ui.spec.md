# Transform Config UI Specification

## 0. Status

- Version: `1.1.0`
- Scope: Dashboard transform registry display metadata consumption, the transform chain editor (`TransformChainEditor`) used by the provider, global-settings, and API-key surfaces, and the schema-driven transform rule config dialog.
- Dependencies: `urp-transform-system.spec.md` (TF-7 through TF-8d, TF-14, PIPE-1), `custom-js-transforms.spec.md` (CJS-REG-2, CJS-REG-4), `dashboard-ui-layout.spec.md` (PL7 through PL10, ST4, ST5, AK1 through AK3b), `frontend-design-system.spec.md` (dirty dialog guard of `DialogContent`).

## 1. Registry metadata consumption

TCU-1. The frontend transform registry item type MUST carry `type_id: string`, `supported_phases: Phase[]`, `supported_scopes: TransformScope[]`, `config_schema: object`, `name: Record<string, string>`, and `description: Record<string, string>`. `name` and `description` are localized-text objects per `urp-transform-system.spec.md` TF-8a.

TCU-1a. The registry item type additionally carries two optional fields present only on custom `js:` transforms (`custom-js-transforms.spec.md` CJS-REG-2): `custom?: boolean` and `visibility?: "admin" | "user"`. Chain editors treat custom items identically to built-ins: phase and scope filtering, localized-text resolution (custom items resolve to the same plain string for every UI language because both locale keys mirror it), and the config dialog schema mapping are unchanged.

TCU-2. Localized-text resolution for a localized-text object `M` and active UI language `L` is exact and MUST apply this order:
1. if `M[L]` exists, use `M[L]`;
2. otherwise, if `L` contains `-`, take the substring of `L` before the first `-`; if `M` has that key, use that value (e.g. `zh-TW` resolves to `M["zh"]`);
3. otherwise, if `M["en"]` exists, use `M["en"]`;
4. otherwise, use the value at the lexicographically smallest key of `M`;
5. if `M` is empty or absent, use the transform `type_id`.

TCU-1b. A `config_schema` property object (at any nesting depth) MAY carry `x-localized-description`, a localized-text object per `urp-transform-system.spec.md` TF-8a/TF-8d. The displayed field description of a property is: the TCU-2 resolution of `x-localized-description` with fallback `description` when `x-localized-description` is a non-empty object; otherwise the string `description`; otherwise no description. A property object MAY likewise carry `x-localized-title`; the displayed field label of a property is the TCU-2 resolution of `x-localized-title` with fallback `title` when it is a non-empty object; otherwise the string `title`; otherwise the property key.

TCU-2a. The chain editors and the config dialog MUST display the resolved `name` as the primary label for a registered transform. The raw `type_id` MUST remain visible as secondary monospace text. The resolved `description` MUST be displayed in the config dialog header area and in the add-transform selector list.

TCU-2b. Each phase section MUST provide exactly one add-transform control: a trigger button labelled "Add transform" (localized) that opens a searchable list (combobox). The control holds no selected value between uses, no transform is added until the user selects a list entry, and no separate add button exists. The list MUST contain exactly the registry items whose `supported_phases` contains the section's phase. Each list entry MUST render the resolved `name`, the `type_id` in monospace, and the resolved `description` when non-empty. The search input MUST match, case-insensitively by substring or fuzzy match, the resolved `name`, the `type_id`, and the resolved `description`. When no entry matches, the list MUST show a localized "No matching transform" text. When the phase has zero registry items, the trigger MUST be disabled.

TCU-2c. The list MUST partition its entries into two groups in this order: "Built-in" (entries whose `type_id` does not start with `js:`) and "Custom (js:)" (entries whose `type_id` starts with `js:`). Each non-empty group MUST render a visible group heading. A group with zero entries (before or after search filtering) MUST NOT render.

TCU-2d. Selecting a list entry MUST close the list and create the rule `{ transform: type_id, enabled: true, models: null, phase: <section phase>, config: {} }`. Postconditions:
1. if the item's `config_schema.required` is absent or an empty array, the rule is appended to the end of the section's phase chain immediately;
2. otherwise, the rule is not appended; the config dialog opens for this pending rule; activating Save with a valid draft appends the saved rule to the end of the section's phase chain; every other dialog close (Cancel, Esc, outside click, close button, discard confirmation) leaves the chain unchanged.

## 2. Config dialog widget mapping

TCU-3. The config dialog MUST derive one field editor per property of `config_schema.properties`. The widget is selected by the first matching rule:
1. property has a non-empty `enum` array → single-select control listing exactly the enum members;
2. `type = "boolean"` → switch control;
3. `type = "integer"` or `type = "number"` → numeric input with `min`, `max`, and step derived from `minimum`, `maximum`, and integer-ness;
4. `type = "string"` with `format = "multiline"` → multi-line plain-text textarea;
5. `type = "string"` → single-line plain-text input;
6. `type = "array"` with `items.type = "object"` and `items.properties` → item-list editor whose rows render nested fields per this same mapping;
7. `type = "array"` → item-list editor whose rows render one nested field per this same mapping applied to `items` (a missing `items` schema renders each row as a TCU-7 typed value editor);
8. `type = "object"` with `properties` → nested group of fields per this same mapping;
9. `type = "object"` without `properties` → key/value map editor per TCU-6;
10. no `type` and no `enum` → typed JSON value editor per TCU-7.

TCU-3a. Plain-text string inputs (rules 4 and 5 of TCU-3) MUST store the entered text verbatim as a JSON string. The user MUST NOT be required to enter surrounding JSON quotes, and entered quote characters MUST be preserved literally as part of the string value.

TCU-3b. Array item-list editors MUST support appending an item, deleting an item, and moving an item up or down. Saved item order MUST equal displayed order.

TCU-4. Each field editor MUST display:
1. the field label per TCU-1b;
2. a type badge naming the widget data type (`string`, `number`, `integer`, `boolean`, `enum`, `array`, `object`, or `json`);
3. a required badge when the key is listed in schema `required`, otherwise an optional badge;
4. the field description per TCU-1b when present; and
5. the schema `default` value when present and the field is currently unset.

TCU-4a. Each field label MUST be programmatically bound to its primary input control (`<label for>` matching the control `id`) when the widget has a single primary control (TCU-3 rules 1 through 5).

TCU-5. Validation MUST run before save and MUST block save while any field is invalid. Validation MUST enforce: `required` presence, `enum` membership, string `minLength`, numeric `minimum`/`maximum`, integer integrality, array `minItems`, and nested object required properties for rule-6 rows.

TCU-5a. Validation results MUST NOT carry display text. Each validation error is `{ path | field: string, code: TransformValidationCode, params?: Record<string, string | number> }` where `code` is exactly one of: `required`, `minLength` (`params.min`), `minimum` (`params.min`), `maximum` (`params.max`), `minItems` (`params.min`), `enum` (`params.values`, the members joined by `, `), `notString`, `notNumber`, `notInteger`, `notBoolean`, `notArray`, `notObject`, `invalidJson`, `duplicateKey` (`params.key`), `phaseUnsupported` (`params.phase`), `configNotObject`. The UI MUST render each error through the `transforms.validation.<code>` i18n key with `params` interpolated, in every supported locale.

TCU-5b. The config dialog MUST render each field error inline below the field it belongs to (the error path's field) and MUST NOT render a raw English validation string.

TCU-5c. A host form that saves a chain MUST, when `findFirstInvalidTransformRule` returns `{ index, errors }`, block the save and show exactly one message `"{rule} · {field}: {reason}"` (localized separator per locale) where `rule` is the TCU-2 resolution of the rule's registry `name` (or its `type_id` when unregistered), `field` is the label of `errors[0]` (each path segment rendered as the TCU-1b field label of the schema property it names; numeric array indexes rendered as `#<index + 1>`; segments joined by ` › `; code `phaseUnsupported` renders the localized word "Phase"; code `configNotObject` renders the localized word "Config"), and `reason` is the TCU-5a message of `errors[0]`. The host MUST then make the editor visible and call `revealTransformRule(layer, index)`, which scrolls the row with `data-transform-rule-index = index` inside the editor root with `data-transform-layer = layer` to the vertical center of the viewport and focuses that row's configure button.

## 3. Unset, null, empty-string, and JSON semantics

TCU-6. The key/value map editor (TCU-3 rule 9) MUST render one row per existing entry with a key text input and a value editor equivalent to TCU-7. It MUST support adding and removing rows. Rows whose key is empty after trimming MUST be excluded from the saved value. Duplicate keys MUST be a validation error.

TCU-7. The typed JSON value editor edits one arbitrary JSON value through an explicit value-kind selector with exactly these kinds: `string`, `number`, `boolean`, `null`, and `json`. Behavior per kind:
1. `string` → plain-text input stored verbatim as a JSON string (TCU-3a applies);
2. `number` → numeric input stored as a JSON number;
3. `boolean` → switch stored as JSON `true`/`false`;
4. `null` → no input; stored as JSON `null`;
5. `json` → monospace textarea whose trimmed content MUST parse with `JSON.parse`; a parse failure is a validation error that blocks save. The `json` mode MUST provide a format action (re-serialize with two-space indentation) and a copy-to-clipboard action.

TCU-7a. When the typed JSON value editor opens over an existing value, the initial kind MUST be inferred from the value: JSON string → `string`, JSON number → `number`, JSON boolean → `boolean`, JSON `null` → `null`, JSON object or array → `json`. When the field is unset and the user activates it, the initial kind MUST be `string`.

TCU-8. Field initialization when the dialog opens is exact, at every nesting depth (top-level properties and rule-6/rule-8 nested properties):
1. a key present in the rule config initializes its editor from the stored JSON value;
2. a key absent from the rule config and listed in the enclosing schema's `required` array initializes as an active editor holding the property's default draft: the schema `default` when present, otherwise the first `enum` member for enums, `false` for booleans, an empty input for numbers and plain-text strings, an empty item list for arrays, an empty map for key/value maps, recursively initialized fields for rule-8 objects, and the `string` kind with an empty input for typed JSON values (TCU-7a);
3. a key absent from the rule config and not listed in `required` initializes as the unset state — visually distinct from every present value, including JSON `null`, `""`, `0`, and `false`;
4. no editor may initialize an absent key as the text `null` or as an empty JSON string literal (`""` typed into a JSON textarea).

TCU-8a. Only optional fields have an unset state. A required field MUST NOT render the unset indicator, the not-set explanation, or the "Set value" action, and MUST NOT expose a clear-field action. Every optional field MUST expose a clear-field action whenever it holds a present value; activating it returns the field to the unset state. The unset state MUST display an explicit not-set indicator stating that the field is omitted on save and a "Set value" action that activates the field with its TCU-8 rule 2 default draft; for fields with a schema `default`, the indicator MUST include that default.

TCU-9. Saved config production is exact:
1. a field in the unset state MUST be omitted from the saved `config` object;
2. a present field MUST be saved as its typed JSON value: strings are not JSON-double-encoded, numbers are JSON numbers, booleans are JSON booleans, explicit `null` (TCU-7 kind `null`) is saved as JSON `null`;
3. an optional plain-text string field whose input is empty MUST be treated as unset and omitted;
4. a required plain-text string field whose input is empty MUST be saved as the empty JSON string `""` unless `minLength >= 1` makes it a validation error;
5. keys not described by `config_schema.properties` but present in the incoming rule config MUST be preserved unchanged on save.

## 4. Layout, data flow, and resilience

TCU-10. The config dialog MUST remain operable at viewport width `375px`: field labels stack above widgets, the dialog body scrolls vertically inside the viewport, and touch targets for add/remove/move/clear actions are at least `44px` on coarse pointers.

TCU-10a. Each phase section header MUST stack the section title and the add-transform trigger vertically below the `sm` breakpoint, with the trigger using the full header width. At the `sm` breakpoint and above, the title and the content-width trigger MUST form one horizontal row with the trigger right-aligned. The open list MUST be at least `min(320px, viewport width − 32px)` wide and its entries MUST wrap long names and descriptions instead of overflowing the list border.

TCU-10c. A chain row MUST contain, in order: a drag handle, the name block (resolved name, TCU-12 badges, and one secondary line `type_id · models summary`), the enable switch, and the actions move up, move down, configure, delete. Layout is exact:
1. at the `sm` breakpoint and above, all parts form one horizontal line; the name block takes the remaining width;
2. below the `sm` breakpoint, the row has two lines: line 1 holds the drag handle and the name block at full width; line 2 holds the enable switch and the actions, right-aligned;
3. the name MUST wrap at any character (`overflow-wrap: anywhere`) and MUST NOT be truncated; the secondary line MAY truncate;
4. the enable switch MUST carry an accessible name "Enable {name}" (localized); every action button MUST carry an accessible name; action buttons MUST be at least `44px × 44px` below the `sm` breakpoint and `28px × 28px` at `sm` and above.

TCU-10d. Rule reordering within a phase chain MUST be possible both by buttons and by drag and drop:
1. move up swaps the row with the previous row of the same phase; it is disabled on the first row; move down swaps with the next row; it is disabled on the last row; after a button move, keyboard focus MUST stay on the moved rule: on its button of the same direction, or on the opposite button when the same direction became disabled;
2. only the drag handle is draggable; text in the rest of the row stays selectable;
3. while a drag is in progress, the source row renders at `50%` opacity, and the row under the pointer renders a `2px` primary-color insertion line on the edge where the source will land (top edge when the source comes from below, bottom edge when it comes from above); dropping moves the source to the target row's position; ending the drag anywhere clears both indicators;
4. dragging across phase sections MUST NOT move a rule.

TCU-10e. The editor root element MUST carry `data-transform-layer` equal to its `layer` prop (`provider`, `global`, or `apiKey`). Every chain row MUST carry `data-transform-rule-index` equal to the rule's position in the editor's full `value` array (request rules and response rules share one index space).

TCU-10b. A scalar editor and its clear or array-item action buttons MUST use the same rendered height at each viewport. Multi-line and structured editors MUST keep their actions top-aligned.

TCU-11. The transform registry MUST be fetched through the shared SWR hook (`useTransformRegistry`). Surfaces that render transform chains while the registry is loading MUST render skeleton placeholders instead of empty chain state, and chain mutations MUST keep the existing optimistic-update behavior of their host forms (dialog-local draft state applied to the parent form value on save).

TCU-12. A transform absent from the registry (unknown `type_id`) MUST render its config read-only as pretty-printed JSON with a copy action, per `dashboard-ui-layout.spec.md` PL10.

TCU-12a. Row status badges are exact, evaluated per row:
1. registered rule for which `validateTransformRule(rule, registryItem)` returns at least one error → a warning-variant status badge "Needs configuration" (zh "需配置") immediately after the name;
2. unregistered rule whose `type_id` starts with `js:` → a warning-variant status badge "Custom transform disabled or deleted" (zh "已停用或已删除的自定义变换"), because CJS-REG-4 removes disabled custom transforms from the registry; the destructive unknown badge MUST NOT render;
3. unregistered rule whose `type_id` does not start with `js:` → a destructive-variant badge "Unknown" (zh "未知");
4. otherwise no status badge.

## 5. Layer execution order

TCU-13. `TransformChainEditor` requires the prop `layer ∈ { "provider", "global", "apiKey" }`. The editor MUST render, above both phase sections, one read-only line: "Execution order: Provider → Global → API key" (zh "执行顺序：Provider → 全局 → API 密钥"; zh-TW "執行順序：Provider → 全域 → API 金鑰"; ja "実行順序：Provider → グローバル → API キー"). The order reflects `urp-transform-system.spec.md` PIPE-1 steps 8–10 and 14–16. The segment equal to `layer` MUST render with `font-medium text-foreground` and `aria-current="true"` and MUST NOT be a link. For a signed-in user whose role is `admin` or `super_admin`, each other segment MUST be an in-app link: Provider → `/dashboard/providers`, Global → `/dashboard/admin-settings?setting=global_transforms`, API key → `/dashboard/tokens`. For every other user, the other segments render as plain muted text.

TCU-13a. The two phase sections MUST be titled "Request phase" and "Response phase" (zh "请求阶段" / "响应阶段"; zh-TW "請求階段" / "回應階段"; ja "リクエストフェーズ" / "レスポンスフェーズ") followed by the rule count of that phase.

## 6. Config dialog behavior

TCU-14. The config dialog header MUST show the resolved transform name as the title, the `type_id` in monospace, and the resolved description when non-empty. It MUST NOT show the internal phase value (`request` / `response`) or any sentence containing it.

TCU-15. The config dialog fields MUST be wrapped in one `<form>`; Save is the form's `type="submit"` button, so pressing Enter in a single-line input submits. Submitting runs TCU-5 validation; on failure it keeps the dialog open, shows inline errors, and focuses the first invalid field's control when that control is focusable.

TCU-16. The config dialog is dirty when any of these differs from its value at dialog open (after TCU-8 initialization): the serialized `models` filter, the draft config (deep structural comparison of `DraftConfig`), or the raw JSON config text (schema-less registered transforms). While dirty:
1. Esc, outside click, and the close button MUST NOT close the dialog; they open the shared discard confirmation of `DialogContent` (`dirty` prop);
2. the Cancel button MUST open a discard confirmation "Discard unsaved changes?" with actions "Keep editing" and "Discard"; "Discard" closes the dialog without saving, "Keep editing" returns to the dialog with the draft unchanged.
While not dirty, Cancel, Esc, outside click, and the close button close the dialog immediately. A successful Save closes the dialog without confirmation.
