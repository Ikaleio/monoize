# Transform Config UI Specification

## 0. Status

- Version: `1.0.0`
- Scope: Dashboard transform registry display metadata consumption, and the schema-driven transform rule config editor used by the provider, global-settings, and API-key transform chain editors.
- Dependencies: `urp-transform-system.spec.md` (TF-7 through TF-8a, TF-14), `dashboard-ui-layout.spec.md` (PL7 through PL10, ST4, ST5, AK1 through AK3b).

## 1. Registry metadata consumption

TCU-1. The frontend transform registry item type MUST carry `type_id: string`, `supported_phases: Phase[]`, `supported_scopes: TransformScope[]`, `config_schema: object`, `name: Record<string, string>`, and `description: Record<string, string>`. `name` and `description` are localized-text objects per `urp-transform-system.spec.md` TF-8a.

TCU-2. Localized-text resolution for a localized-text object `M` and active UI language `L` is exact and MUST apply this order:
1. if `M[L]` exists, use `M[L]`;
2. otherwise, if `L` contains `-`, take the substring of `L` before the first `-`; if `M` has that key, use that value (e.g. `zh-TW` resolves to `M["zh"]`);
3. otherwise, if `M["en"]` exists, use `M["en"]`;
4. otherwise, use the value at the lexicographically smallest key of `M`;
5. if `M` is empty or absent, use the transform `type_id`.

TCU-2a. The chain editors and the config dialog MUST display the resolved `name` as the primary label for a registered transform. The raw `type_id` MUST remain visible as secondary monospace text. The resolved `description` MUST be displayed in the config dialog header area and in the add-transform selector list.

TCU-2b. A closed add-transform selector MUST render the selected transform as one line of resolved `name` text. The text MUST truncate inside the trigger when it exceeds the available width. The closed trigger MUST NOT render the selected transform's `type_id` or description. The open selector list MUST continue to render the resolved name, `type_id`, and description required by TCU-2a.

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
1. the schema `title` when present, otherwise the property key;
2. a type badge naming the widget data type (`string`, `number`, `integer`, `boolean`, `enum`, `array`, `object`, or `json`);
3. a required badge when the key is listed in schema `required`, otherwise an optional badge;
4. the schema `description` when present; and
5. the schema `default` value when present and the field is currently unset.

TCU-5. Validation MUST run before save and MUST block save while any field is invalid. Validation MUST enforce: `required` presence, `enum` membership, string `minLength`, numeric `minimum`/`maximum`, integer integrality, array `minItems`, and nested object required properties for rule-6 rows.

## 3. Unset, null, empty-string, and JSON semantics

TCU-6. The key/value map editor (TCU-3 rule 9) MUST render one row per existing entry with a key text input and a value editor equivalent to TCU-7. It MUST support adding and removing rows. Rows whose key is empty after trimming MUST be excluded from the saved value. Duplicate keys MUST be a validation error.

TCU-7. The typed JSON value editor edits one arbitrary JSON value through an explicit value-kind selector with exactly these kinds: `string`, `number`, `boolean`, `null`, and `json`. Behavior per kind:
1. `string` → plain-text input stored verbatim as a JSON string (TCU-3a applies);
2. `number` → numeric input stored as a JSON number;
3. `boolean` → switch stored as JSON `true`/`false`;
4. `null` → no input; stored as JSON `null`;
5. `json` → monospace textarea whose trimmed content MUST parse with `JSON.parse`; a parse failure is a validation error that blocks save. The `json` mode MUST provide a format action (re-serialize with two-space indentation) and a copy-to-clipboard action.

TCU-7a. When the typed JSON value editor opens over an existing value, the initial kind MUST be inferred from the value: JSON string → `string`, JSON number → `number`, JSON boolean → `boolean`, JSON `null` → `null`, JSON object or array → `json`. When the field is unset and the user activates it, the initial kind MUST be `string`.

TCU-8. Field initialization when the dialog opens is exact:
1. a key present in the rule config initializes its editor from the stored JSON value;
2. a key absent from the rule config initializes as the unset state — visually distinct from every present value, including JSON `null`, `""`, `0`, and `false`;
3. no editor may initialize an absent key as the text `null` or as an empty JSON string.

TCU-8a. Every optional field MUST expose a clear-field action whenever it holds a present value. Activating it returns the field to the unset state. The unset state MUST display an explicit not-set indicator; for fields with a schema `default`, the indicator MUST include that default.

TCU-9. Saved config production is exact:
1. a field in the unset state MUST be omitted from the saved `config` object;
2. a present field MUST be saved as its typed JSON value: strings are not JSON-double-encoded, numbers are JSON numbers, booleans are JSON booleans, explicit `null` (TCU-7 kind `null`) is saved as JSON `null`;
3. an optional plain-text string field whose input is empty MUST be treated as unset and omitted;
4. a required plain-text string field whose input is empty MUST be saved as the empty JSON string `""` unless `minLength >= 1` makes it a validation error;
5. keys not described by `config_schema.properties` but present in the incoming rule config MUST be preserved unchanged on save.

## 4. Layout, data flow, and resilience

TCU-10. The config dialog MUST remain operable at viewport width `375px`: field labels stack above widgets, the dialog body scrolls vertically inside the viewport, and touch targets for add/remove/move/clear actions are at least `44px` on coarse pointers.

TCU-10a. Each phase section's add-transform controls MUST form one vertical group below the `sm` breakpoint. The selector and add button MUST each use the full group width. At the `sm` breakpoint and above, the controls MUST form one horizontal row with a `260px` selector and a content-width add button. Selector content MUST remain inside the selector border at every supported width.

TCU-11. The transform registry MUST be fetched through the shared SWR hook (`useTransformRegistry`). Surfaces that render transform chains while the registry is loading MUST render skeleton placeholders instead of empty chain state, and chain mutations MUST keep the existing optimistic-update behavior of their host forms (dialog-local draft state applied to the parent form value on save).

TCU-12. A transform absent from the registry (unknown `type_id`) MUST render its config read-only as pretty-printed JSON with a copy action, per `dashboard-ui-layout.spec.md` PL10.
