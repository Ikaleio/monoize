import { describe, expect, test } from 'bun:test'
import { resolveLocalizedText } from '../src/components/transforms/localized-text'
import {
	buildDraftConfig,
	buildDraftValue,
	buildTypedJsonDraft,
	convertTypedJsonDraft,
	defaultDraftForProperty,
	resolveWidgetKind,
	serializeDraftConfig,
	serializeDraftValue,
	validateTransformRule,
	widgetTypeBadge,
	type DraftValue,
	type JsonSchemaObject,
	type JsonSchemaProperty
} from '../src/components/transforms/transform-schema'
import type { TransformRegistryItem } from '../src/lib/api'

describe('localized text resolution (TCU-2)', () => {
	const map = { en: 'Set field', zh: '设置字段' }

	test('uses the exact language key when present', () => {
		expect(resolveLocalizedText(map, 'zh', 'field_set')).toBe('设置字段')
		expect(resolveLocalizedText(map, 'en', 'field_set')).toBe('Set field')
	})

	test('resolves regional variants to the base language', () => {
		expect(resolveLocalizedText(map, 'zh-TW', 'field_set')).toBe('设置字段')
		expect(resolveLocalizedText(map, 'zh-HK', 'field_set')).toBe('设置字段')
	})

	test('falls back to en when neither exact nor base matches', () => {
		expect(resolveLocalizedText(map, 'ja', 'field_set')).toBe('Set field')
	})

	test('falls back to the lexicographically smallest key without en', () => {
		expect(resolveLocalizedText({ zh: '乙', de: 'De' }, 'ja', 'x')).toBe('De')
	})

	test('falls back to the provided id for empty or absent maps', () => {
		expect(resolveLocalizedText({}, 'en', 'field_set')).toBe('field_set')
		expect(resolveLocalizedText(undefined, 'en', 'field_set')).toBe('field_set')
	})
})

describe('widget mapping (TCU-3, TCU-4)', () => {
	test('selects widgets by the first matching rule', () => {
		expect(resolveWidgetKind({ type: 'string', enum: ['a', 'b'] })).toBe('enum')
		expect(resolveWidgetKind({ type: 'boolean' })).toBe('boolean')
		expect(resolveWidgetKind({ type: 'integer' })).toBe('integer')
		expect(resolveWidgetKind({ type: 'number' })).toBe('number')
		expect(resolveWidgetKind({ type: 'string', format: 'multiline' })).toBe('string-multiline')
		expect(resolveWidgetKind({ type: 'string' })).toBe('string')
		expect(resolveWidgetKind({ type: 'array', items: { type: 'string' } })).toBe('array')
		expect(resolveWidgetKind({ type: 'object', properties: { a: { type: 'string' } } })).toBe('object-fields')
		expect(resolveWidgetKind({ type: 'object' })).toBe('object-map')
		expect(resolveWidgetKind({})).toBe('json')
		expect(resolveWidgetKind(undefined)).toBe('json')
	})

	test('produces the eight type badge labels', () => {
		expect(widgetTypeBadge({ enum: ['a'] })).toBe('enum')
		expect(widgetTypeBadge({ type: 'boolean' })).toBe('boolean')
		expect(widgetTypeBadge({ type: 'integer' })).toBe('integer')
		expect(widgetTypeBadge({ type: 'number' })).toBe('number')
		expect(widgetTypeBadge({ type: 'string', format: 'multiline' })).toBe('string')
		expect(widgetTypeBadge({ type: 'array' })).toBe('array')
		expect(widgetTypeBadge({ type: 'object' })).toBe('object')
		expect(widgetTypeBadge({})).toBe('json')
	})
})

describe('draft initialization (TCU-7a, TCU-8)', () => {
	const schema: JsonSchemaObject = {
		type: 'object',
		properties: {
			path: { type: 'string', minLength: 1 },
			value: {},
			when_equals: {}
		},
		required: ['path', 'value']
	}

	test('present keys initialize from the stored JSON value', () => {
		const { drafts } = buildDraftConfig(schema, { path: 'a.b', value: 'normal' })
		expect(drafts.path).toEqual({ kind: 'string', text: 'a.b' })
		expect(drafts.value).toEqual({ kind: 'string', text: 'normal' })
	})

	test('absent keys initialize as unset, distinct from null and empty string', () => {
		const { drafts } = buildDraftConfig(schema, { path: 'a', value: null })
		expect(drafts.when_equals).toEqual({ kind: 'unset' })
		expect(drafts.value).toEqual({ kind: 'null' })
		const withEmpty = buildDraftConfig(schema, { path: 'a', value: '' })
		expect(withEmpty.drafts.value).toEqual({ kind: 'string', text: '' })
	})

	test('keys outside the schema are kept in extra (TCU-9 rule 5)', () => {
		const { extra } = buildDraftConfig(schema, { path: 'a', value: 1, legacy: true })
		expect(extra).toEqual({ legacy: true })
	})

	test('typed JSON drafts infer the initial kind from the value', () => {
		expect(buildTypedJsonDraft('x')).toEqual({ kind: 'string', text: 'x' })
		expect(buildTypedJsonDraft(3.5)).toEqual({ kind: 'number', text: '3.5' })
		expect(buildTypedJsonDraft(false)).toEqual({ kind: 'boolean', value: false })
		expect(buildTypedJsonDraft(null)).toEqual({ kind: 'null' })
		expect(buildTypedJsonDraft({ a: 1 })).toEqual({ kind: 'json', text: '{\n  "a": 1\n}' })
	})

	test('schema/value type mismatches fall back to typed JSON drafts', () => {
		expect(buildDraftValue({ type: 'boolean' }, 'yes')).toEqual({ kind: 'string', text: 'yes' })
		expect(buildDraftValue({ type: 'integer' }, 'ten')).toEqual({ kind: 'string', text: 'ten' })
		expect(buildDraftValue({ type: 'string' }, 7)).toEqual({ kind: 'number', text: '7' })
	})

	test('activating an unset untyped field starts in string kind (TCU-7a)', () => {
		expect(defaultDraftForProperty({})).toEqual({ kind: 'string', text: '' })
	})

	test('activating an unset field applies the schema default', () => {
		expect(defaultDraftForProperty({ type: 'boolean', default: true })).toEqual({
			kind: 'boolean',
			value: true
		})
		expect(defaultDraftForProperty({ type: 'integer', default: 80 })).toEqual({
			kind: 'number',
			text: '80'
		})
		expect(
			defaultDraftForProperty({ type: 'string', enum: ['a', 'b'], default: 'b' })
		).toEqual({ kind: 'enum', value: 'b' })
	})
})

describe('saved config production (TCU-9)', () => {
	const schema: JsonSchemaObject = {
		type: 'object',
		properties: {
			name: { type: 'string' },
			strict: { type: 'string', minLength: 1 },
			count: { type: 'integer', minimum: 1, maximum: 100 },
			flag: { type: 'boolean' },
			mode: { type: 'string', enum: ['a', 'b'] },
			value: {}
		},
		required: ['strict']
	}

	const base = (): ReturnType<typeof buildDraftConfig> =>
		buildDraftConfig(schema, { strict: 'keep' })

	test('unset fields are omitted and required unset fields error', () => {
		const draft = base()
		const ok = serializeDraftConfig(schema, draft)
		expect(ok.errors).toEqual([])
		expect(ok.config).toEqual({ strict: 'keep' })

		draft.drafts.strict = { kind: 'unset' }
		const bad = serializeDraftConfig(schema, draft)
		expect(bad.errors).toEqual([{ path: 'strict', message: 'is required' }])
	})

	test('plain strings are saved verbatim without JSON double-encoding', () => {
		const draft = base()
		draft.drafts.name = { kind: 'string', text: 'say "hi"' }
		const { config, errors } = serializeDraftConfig(schema, draft)
		expect(errors).toEqual([])
		expect(config.name).toBe('say "hi"')
	})

	test('an optional empty plain-text string is omitted', () => {
		const draft = base()
		draft.drafts.name = { kind: 'string', text: '' }
		const { config, errors } = serializeDraftConfig(schema, draft)
		expect(errors).toEqual([])
		expect('name' in config).toBe(false)
	})

	test('a required empty string saves as "" without minLength and errors with it', () => {
		const relaxed: JsonSchemaObject = {
			type: 'object',
			properties: { name: { type: 'string' } },
			required: ['name']
		}
		const draft = buildDraftConfig(relaxed, { name: 'x' })
		draft.drafts.name = { kind: 'string', text: '' }
		expect(serializeDraftConfig(relaxed, draft)).toEqual({
			config: { name: '' },
			errors: []
		})

		const strictDraft = base()
		strictDraft.drafts.strict = { kind: 'string', text: '' }
		const { errors } = serializeDraftConfig(schema, strictDraft)
		expect(errors).toEqual([{ path: 'strict', message: 'must be at least 1 characters' }])
	})

	test('numbers parse, enforce integrality and bounds, and empty numbers unset', () => {
		const draft = base()
		draft.drafts.count = { kind: 'number', text: '42' }
		expect(serializeDraftConfig(schema, draft).config.count).toBe(42)

		draft.drafts.count = { kind: 'number', text: '2.5' }
		expect(serializeDraftConfig(schema, draft).errors).toEqual([
			{ path: 'count', message: 'must be an integer' }
		])

		draft.drafts.count = { kind: 'number', text: '0' }
		expect(serializeDraftConfig(schema, draft).errors).toEqual([
			{ path: 'count', message: 'must be >= 1' }
		])

		draft.drafts.count = { kind: 'number', text: '101' }
		expect(serializeDraftConfig(schema, draft).errors).toEqual([
			{ path: 'count', message: 'must be <= 100' }
		])

		draft.drafts.count = { kind: 'number', text: '' }
		const { config, errors } = serializeDraftConfig(schema, draft)
		expect(errors).toEqual([])
		expect('count' in config).toBe(false)
	})

	test('enum membership is enforced', () => {
		const draft = base()
		draft.drafts.mode = { kind: 'enum', value: 'b' }
		expect(serializeDraftConfig(schema, draft).config.mode).toBe('b')
		draft.drafts.mode = { kind: 'enum', value: 'z' }
		expect(serializeDraftConfig(schema, draft).errors).toEqual([
			{ path: 'mode', message: 'must be one of: a, b' }
		])
	})

	test('explicit null is saved as JSON null (TCU-7 kind null)', () => {
		const draft = base()
		draft.drafts.value = { kind: 'null' }
		expect(serializeDraftConfig(schema, draft).config.value).toBeNull()
	})

	test('json kind must parse and never coerces bare words into strings', () => {
		const draft = base()
		draft.drafts.value = { kind: 'json', text: '{"a": [1, 2]}' }
		expect(serializeDraftConfig(schema, draft).config.value).toEqual({ a: [1, 2] })

		draft.drafts.value = { kind: 'json', text: 'normal' }
		expect(serializeDraftConfig(schema, draft).errors).toEqual([
			{ path: 'value', message: 'Invalid JSON' }
		])

		draft.drafts.value = { kind: 'json', text: '' }
		expect(serializeDraftConfig(schema, draft).errors).toEqual([
			{ path: 'value', message: 'Invalid JSON' }
		])
	})

	test('unknown incoming keys are preserved unchanged', () => {
		const draft = buildDraftConfig(schema, { strict: 'keep', legacy: { deep: true } })
		expect(serializeDraftConfig(schema, draft).config).toEqual({
			strict: 'keep',
			legacy: { deep: true }
		})
	})
})

describe('array editor semantics (TCU-3b, TCU-5)', () => {
	const rulesProperty: JsonSchemaProperty = {
		type: 'array',
		items: {
			type: 'object',
			properties: {
				pattern: { type: 'string', minLength: 1 },
				suffix: { type: 'string', minLength: 1 }
			},
			required: ['pattern', 'suffix'],
			additionalProperties: false
		},
		minItems: 1
	}

	test('saved item order equals displayed order', () => {
		const draft = buildDraftValue(rulesProperty, [
			{ pattern: 'a*', suffix: '-x' },
			{ pattern: 'b*', suffix: '-y' }
		])
		const result = serializeDraftValue(rulesProperty, draft, 'rules', true)
		expect(result.errors).toEqual([])
		expect(result.value).toEqual([
			{ pattern: 'a*', suffix: '-x' },
			{ pattern: 'b*', suffix: '-y' }
		])
	})

	test('minItems and nested required properties are enforced', () => {
		const empty = serializeDraftValue(
			rulesProperty,
			{ kind: 'array', items: [] },
			'rules',
			true
		)
		expect(empty.errors).toEqual([
			{ path: 'rules', message: 'must have at least 1 item(s)' }
		])

		const missing: DraftValue = {
			kind: 'array',
			items: [
				{
					kind: 'object',
					fields: { pattern: { kind: 'string', text: 'a*' }, suffix: { kind: 'unset' } },
					extra: {}
				}
			]
		}
		const bad = serializeDraftValue(rulesProperty, missing, 'rules', true)
		expect(bad.errors).toEqual([{ path: 'rules.0.suffix', message: 'is required' }])
	})

	test('rows of an untyped array use typed JSON value semantics', () => {
		const property: JsonSchemaProperty = { type: 'array' }
		const draft: DraftValue = {
			kind: 'array',
			items: [
				{ kind: 'string', text: 'plain' },
				{ kind: 'number', text: '2' },
				{ kind: 'null' }
			]
		}
		const result = serializeDraftValue(property, draft, 'list', false)
		expect(result.errors).toEqual([])
		expect(result.value).toEqual(['plain', 2, null])
	})
})

describe('key/value map editor semantics (TCU-6)', () => {
	const property: JsonSchemaProperty = { type: 'object' }

	test('rows with empty keys are excluded and duplicate keys error', () => {
		const draft: DraftValue = {
			kind: 'map',
			entries: [
				{ key: 'model', value: { kind: 'string', text: 'gpt' } },
				{ key: '  ', value: { kind: 'string', text: 'dropped' } },
				{ key: 'model', value: { kind: 'string', text: 'dup' } }
			]
		}
		const result = serializeDraftValue(property, draft, 'extra', false)
		expect(result.value).toEqual({ model: 'gpt' })
		expect(result.errors).toEqual([
			{ path: 'extra.2', message: 'duplicate key "model"' }
		])
	})

	test('map values keep their JSON types', () => {
		const draft: DraftValue = {
			kind: 'map',
			entries: [
				{ key: 'a', value: { kind: 'number', text: '1' } },
				{ key: 'b', value: { kind: 'boolean', value: true } },
				{ key: 'c', value: { kind: 'null' } },
				{ key: 'd', value: { kind: 'json', text: '[1]' } }
			]
		}
		const result = serializeDraftValue(property, draft, 'extra', false)
		expect(result.errors).toEqual([])
		expect(result.value).toEqual({ a: 1, b: true, c: null, d: [1] })
	})
})

describe('typed JSON kind switching (TCU-7)', () => {
	test('string to json quotes the text as a JSON string literal', () => {
		expect(convertTypedJsonDraft({ kind: 'string', text: 'a"b' }, 'json')).toEqual({
			kind: 'json',
			text: '"a\\"b"'
		})
	})

	test('numeric strings carry over to number kind', () => {
		expect(convertTypedJsonDraft({ kind: 'string', text: '12' }, 'number')).toEqual({
			kind: 'number',
			text: '12'
		})
		expect(convertTypedJsonDraft({ kind: 'string', text: 'abc' }, 'number')).toEqual({
			kind: 'number',
			text: ''
		})
	})

	test('null and boolean kinds convert to their JSON literals', () => {
		expect(convertTypedJsonDraft({ kind: 'null' }, 'json')).toEqual({ kind: 'json', text: 'null' })
		expect(convertTypedJsonDraft({ kind: 'boolean', value: true }, 'json')).toEqual({
			kind: 'json',
			text: 'true'
		})
	})
})

describe('transform rule validation (TCU-5)', () => {
	const registryItem: TransformRegistryItem = {
		type_id: 'reasoning_effort_to_model_suffix',
		supported_phases: ['request'],
		supported_scopes: ['provider'],
		name: { en: 'Reasoning: effort to model suffix', zh: '推理：effort 转模型后缀' },
		description: { en: 'x', zh: 'y' },
		config_schema: {
			type: 'object',
			properties: {
				rules: {
					type: 'array',
					items: {
						type: 'object',
						properties: {
							pattern: { type: 'string', minLength: 1 },
							suffix: { type: 'string', minLength: 1 }
						},
						required: ['pattern', 'suffix'],
						additionalProperties: false
					},
					minItems: 1
				}
			},
			required: ['rules'],
			additionalProperties: false
		}
	}

	test('accepts a valid rule', () => {
		expect(
			validateTransformRule(
				{
					transform: 'reasoning_effort_to_model_suffix',
					enabled: true,
					models: null,
					phase: 'request',
					config: { rules: [{ pattern: 'gpt*', suffix: '-high' }] }
				},
				registryItem
			)
		).toEqual([])
	})

	test('rejects unsupported phases, missing required keys, and bad nested rows', () => {
		const phaseErrors = validateTransformRule(
			{
				transform: 'reasoning_effort_to_model_suffix',
				enabled: true,
				models: null,
				phase: 'response',
				config: { rules: [{ pattern: 'gpt*', suffix: '-high' }] }
			},
			registryItem
		)
		expect(phaseErrors.map(e => e.field)).toEqual(['phase'])

		const missingErrors = validateTransformRule(
			{
				transform: 'reasoning_effort_to_model_suffix',
				enabled: true,
				models: null,
				phase: 'request',
				config: {}
			},
			registryItem
		)
		expect(missingErrors).toEqual([{ field: 'rules', message: 'is required' }])

		const nestedErrors = validateTransformRule(
			{
				transform: 'reasoning_effort_to_model_suffix',
				enabled: true,
				models: null,
				phase: 'request',
				config: { rules: [{ pattern: 'gpt*' }] }
			},
			registryItem
		)
		expect(nestedErrors).toEqual([
			{ field: 'rules.0.suffix', message: 'is required' }
		])
	})
})
