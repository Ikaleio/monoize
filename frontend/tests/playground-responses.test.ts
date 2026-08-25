import { describe, expect, test } from 'bun:test'
import type { UIMessage } from 'ai'
import {
	RAW_REASONING_SUMMARY_INDEX_BASE,
	createRawReasoningRewriteTransform,
	reasoningPartKind,
	rewriteRawReasoningFrame
} from '../src/components/playground/responses-sse'
import { sanitizeForModel } from '../src/components/playground/chat-transport'

function frame(data: Record<string, unknown>): string {
	return `event: ${String(data.type)}\ndata: ${JSON.stringify(data)}`
}

function parseFrames(text: string): Array<Record<string, unknown>> {
	const frames: Array<Record<string, unknown>> = []
	for (const chunk of text.split('\n\n')) {
		const dataLine = chunk.split('\n').find((line) => line.startsWith('data:'))
		if (!dataLine) continue
		try {
			frames.push(JSON.parse(dataLine.slice(5)) as Record<string, unknown>)
		} catch {
			/* non-JSON data (e.g. [DONE]) is not a frame under test */
		}
	}
	return frames
}

describe('reasoningPartKind (PG-CHAT8)', () => {
	test('classifies rewritten raw-reasoning ids as content', () => {
		expect(reasoningPartKind(`rs_1:${RAW_REASONING_SUMMARY_INDEX_BASE}`)).toBe('content')
		expect(reasoningPartKind('rs_1:1003')).toBe('content')
	})

	test('classifies native summary ids and malformed ids as summary', () => {
		expect(reasoningPartKind('rs_1:0')).toBe('summary')
		expect(reasoningPartKind('rs_1:2')).toBe('summary')
		expect(reasoningPartKind('rs_1')).toBe('summary')
		expect(reasoningPartKind('rs_1:x')).toBe('summary')
		expect(reasoningPartKind(undefined)).toBe('summary')
	})
})

describe('rewriteRawReasoningFrame (PG-CHAT7)', () => {
	test('passes through non-reasoning frames unchanged', () => {
		const started = new Set<string>()
		expect(
			rewriteRawReasoningFrame(
				frame({ type: 'response.output_text.delta', item_id: 'msg_1', delta: 'hi' }),
				started
			)
		).toBeNull()
		expect(rewriteRawReasoningFrame('data: [DONE]', started)).toBeNull()
		expect(started.size).toBe(0)
	})

	test('rewrites raw deltas and injects one part-added per item/content pair', () => {
		const started = new Set<string>()
		const first = rewriteRawReasoningFrame(
			frame({
				type: 'response.reasoning_text.delta',
				item_id: 'rs_1',
				output_index: 0,
				content_index: 0,
				delta: 'thinking '
			}),
			started
		)
		const firstFrames = parseFrames(first ?? '')
		expect(firstFrames).toEqual([
			{
				type: 'response.reasoning_summary_part.added',
				item_id: 'rs_1',
				output_index: 0,
				summary_index: 1000
			},
			{
				type: 'response.reasoning_summary_text.delta',
				item_id: 'rs_1',
				output_index: 0,
				summary_index: 1000,
				delta: 'thinking '
			}
		])

		const second = rewriteRawReasoningFrame(
			frame({
				type: 'response.reasoning_text.delta',
				item_id: 'rs_1',
				output_index: 0,
				content_index: 0,
				delta: 'more'
			}),
			started
		)
		expect(parseFrames(second ?? '')).toEqual([
			{
				type: 'response.reasoning_summary_text.delta',
				item_id: 'rs_1',
				output_index: 0,
				summary_index: 1000,
				delta: 'more'
			}
		])
	})

	test('rewrites done frames and injects part-added when no delta preceded', () => {
		const started = new Set<string>()
		const done = rewriteRawReasoningFrame(
			frame({
				type: 'response.reasoning_text.done',
				item_id: 'rs_2',
				output_index: 1,
				content_index: 0,
				text: 'full'
			}),
			started
		)
		const frames = parseFrames(done ?? '')
		expect(frames[0]).toEqual({
			type: 'response.reasoning_summary_part.added',
			item_id: 'rs_2',
			output_index: 1,
			summary_index: 1000
		})
		expect(frames[1]).toEqual({
			type: 'response.reasoning_summary_part.done',
			item_id: 'rs_2',
			output_index: 1,
			summary_index: 1000,
			text: 'full'
		})
	})
})

describe('createRawReasoningRewriteTransform (PG-CHAT7)', () => {
	async function pump(chunks: string[]): Promise<string> {
		const transform = createRawReasoningRewriteTransform()
		const writer = transform.writable.getWriter()
		const reader = transform.readable.getReader()
		const output: string[] = []
		const readAll = (async () => {
			for (;;) {
				const { done, value } = await reader.read()
				if (done) break
				output.push(value)
			}
		})()
		for (const chunk of chunks) await writer.write(chunk)
		await writer.close()
		await readAll
		return output.join('')
	}

	test('rewrites frames split across arbitrary chunk boundaries', async () => {
		const input =
			frame({ type: 'response.created', response: { id: 'r1' } }) +
			'\n\n' +
			frame({
				type: 'response.reasoning_text.delta',
				item_id: 'rs_1',
				output_index: 0,
				content_index: 0,
				delta: 'x'
			}) +
			'\n\ndata: [DONE]\n\n'
		const whole = await pump([input])
		const split = await pump(input.split(''))
		expect(split).toBe(whole)
		const types = parseFrames(whole).map((data) => data.type)
		expect(types).toEqual([
			'response.created',
			'response.reasoning_summary_part.added',
			'response.reasoning_summary_text.delta'
		])
		expect(whole.endsWith('data: [DONE]\n\n')).toBe(true)
	})

	test('passes non-reasoning frames through byte-identical', async () => {
		const input =
			`event: response.output_text.delta\ndata: {"type":"response.output_text.delta","item_id":"m1","output_index":0,"delta":"hi"}\n\n` +
			`data: [DONE]\n\n`
		expect(await pump([input])).toBe(input)
	})
})

describe('sanitizeForModel (PG-CHAT3)', () => {
	const textPart = { type: 'text' as const, text: 'answer' }
	const reasoningPart = { type: 'reasoning' as const, text: 'thinking' }
	const filePart = {
		type: 'file' as const,
		mediaType: 'image/png',
		url: 'data:image/png;base64,AAAA'
	}

	test('strips assistant reasoning and file parts, keeps text', () => {
		const messages: UIMessage[] = [
			{ id: 'a1', role: 'assistant', parts: [reasoningPart, textPart, filePart] }
		]
		const sanitized = sanitizeForModel(messages)
		expect(sanitized).toHaveLength(1)
		expect(sanitized[0]?.parts).toEqual([textPart])
	})

	test('substitutes [image] when stripping removes a file part and empties the message', () => {
		const messages: UIMessage[] = [{ id: 'a1', role: 'assistant', parts: [filePart] }]
		expect(sanitizeForModel(messages)[0]?.parts).toEqual([
			{ type: 'text', text: '[image]' }
		])
	})

	test('drops assistant messages emptied by reasoning-only stripping', () => {
		const messages: UIMessage[] = [
			{ id: 'a1', role: 'assistant', parts: [reasoningPart] },
			{ id: 'u1', role: 'user', parts: [{ type: 'text', text: 'hi' }] }
		]
		const sanitized = sanitizeForModel(messages)
		expect(sanitized).toHaveLength(1)
		expect(sanitized[0]?.id).toBe('u1')
	})

	test('preserves user file parts', () => {
		const messages: UIMessage[] = [
			{ id: 'u1', role: 'user', parts: [filePart, { type: 'text', text: 'look' }] }
		]
		expect(sanitizeForModel(messages)[0]?.parts).toHaveLength(2)
	})
})
