import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Virtuoso } from 'react-virtuoso'
import { AnimatePresence } from 'framer-motion'
import { Check, ChevronRight, Copy, RefreshCw } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
	Dialog,
	DialogContent,
	DialogDescription,
	DialogHeader,
	DialogTitle
} from '@/components/ui/dialog'
import {
	Select,
	SelectContent,
	SelectItem,
	SelectTrigger,
	SelectValue
} from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { easings, motion } from '@/components/ui/motion'
import { useRequestCapture } from '@/lib/swr'
import { cn } from '@/lib/utils'
import type { CaptureAttempt, CaptureFrameTruncation } from '@/lib/api'

// RCV-F15: non-linear motion tokens, durations inside the 0.16s-0.30s band.
const expandTransition = { duration: 0.22, ease: easings.easeOutExpo }
const collapseTransition = { duration: 0.18, ease: easings.easeInOutQuart }

// RCV-F13/RCV-F14 collapse thresholds.
const COLLAPSE_DEPTH = 3
const ARRAY_COLLAPSE_THRESHOLD = 20
const OBJECT_COLLAPSE_THRESHOLD = 50
const STRING_TRUNCATE_LENGTH = 400

type Translate = (key: string, options?: Record<string, unknown>) => string

interface CaptureViewerDialogProps {
	open: boolean
	onOpenChange: (open: boolean) => void
	requestId: string | null
	userId: string | null
}

export function CaptureViewerDialog({
	open,
	onOpenChange,
	requestId,
	userId
}: CaptureViewerDialogProps) {
	const { t } = useTranslation()
	const { data, error, isLoading, mutate } = useRequestCapture(
		requestId,
		userId,
		open
	)
	const [attemptIndex, setAttemptIndex] = useState(0)
	const [lastRequestId, setLastRequestId] = useState(requestId)
	if (lastRequestId !== requestId) {
		setLastRequestId(requestId)
		setAttemptIndex(0)
	}

	const attempts = data?.dump?.attempts ?? []
	const selectedAttempt: CaptureAttempt | null =
		attempts.length > 0 ? attempts[Math.min(attemptIndex, attempts.length - 1)] : null

	return (
		<Dialog open={open} onOpenChange={onOpenChange}>
			<DialogContent className='w-[92vw] max-w-[92vw] gap-0 p-4 sm:w-full sm:max-w-4xl sm:p-6'>
				<DialogHeader className='pr-8 text-left'>
					<DialogTitle>{t('requestLogs.capture.title')}</DialogTitle>
					<DialogDescription className='break-all font-mono text-xs'>
						{data?.request_id ?? requestId ?? ''}
					</DialogDescription>
				</DialogHeader>

				{isLoading || (!data && !error) ? (
					<div className='mt-4 space-y-3' aria-busy='true'>
						<div className='flex flex-wrap gap-2'>
							<Skeleton className='h-5 w-24' />
							<Skeleton className='h-5 w-16' />
							<Skeleton className='h-5 w-20' />
						</div>
						<Skeleton className='h-9 w-full' />
						<Skeleton className='h-64 w-full' />
					</div>
				) : error ? (
					<div className='mt-4 flex flex-col items-start gap-3 rounded-md border border-destructive/40 bg-destructive/5 p-4'>
						<p className='text-sm text-destructive'>
							{t('requestLogs.capture.loadFailed')}
						</p>
						<Button
							type='button'
							variant='outline'
							size='sm'
							onClick={() => void mutate()}
						>
							<RefreshCw className='mr-1.5 h-3.5 w-3.5' />
							{t('requestLogs.capture.retry')}
						</Button>
					</div>
				) : data ? (
					<div className='mt-3 flex min-h-0 flex-col gap-3'>
						<div className='flex flex-wrap items-center gap-1.5 text-xs'>
							<Badge
								variant='secondary'
								className='h-5 px-1.5 font-mono border-info-border bg-info-soft text-info-foreground'
							>
								{data.dump.downstream_protocol}
							</Badge>
							{data.dump.is_stream ?
								<Badge
									variant='secondary'
									className='h-5 px-1.5 border-info-border bg-info-soft text-info-foreground'
								>
									{t('requestLogs.capture.stream')}
								</Badge>
							:	<Badge
									variant='secondary'
									className='h-5 px-1.5 border-warning-border bg-warning-soft text-warning-foreground'
								>
									{t('requestLogs.capture.nonStream')}
								</Badge>
							}
							<span className='text-muted-foreground'>
								{new Date(data.created_at).toLocaleString()}
							</span>
							{data.owner.username ?
								<span className='text-muted-foreground'>
									{data.owner.username}
								</span>
							:	null}
							<span className='font-mono text-muted-foreground'>
								{formatBytes(data.size_bytes)}
							</span>
						</div>

						{attempts.length > 1 && (
							<Select
								value={String(Math.min(attemptIndex, attempts.length - 1))}
								onValueChange={value => setAttemptIndex(Number(value))}
							>
								<SelectTrigger
									className='h-8 w-full text-xs sm:w-80'
									aria-label={t('requestLogs.capture.attempt')}
								>
									<SelectValue />
								</SelectTrigger>
								<SelectContent>
									{attempts.map((attempt, index) => (
										<SelectItem key={index} value={String(index)}>
											<span className='font-mono'>
												#{attempt.attempt_number} · {attempt.provider_id} ·{' '}
												{attempt.upstream_model}
											</span>
										</SelectItem>
									))}
								</SelectContent>
							</Select>
						)}

						{selectedAttempt ?
							<>
								<TransformChainStrip attempt={selectedAttempt} t={t} />
								<AttemptTabs attempt={selectedAttempt} t={t} />
							</>
						:	<p className='py-8 text-center text-sm text-muted-foreground'>
								{t('requestLogs.capture.empty')}
							</p>
						}
					</div>
				) : null}
			</DialogContent>
		</Dialog>
	)
}

function formatBytes(bytes: number): string {
	if (bytes < 1024) return `${bytes} B`
	if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
	return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

const SCOPE_CHIP_CLASSES: Record<string, string> = {
	provider: 'border-warning-border bg-warning-soft text-warning-foreground',
	global: 'border-info-border bg-info-soft text-info-foreground',
	api_key: 'border-success-border bg-success-soft text-success-foreground'
}

function TransformChainStrip({
	attempt,
	t
}: {
	attempt: CaptureAttempt
	t: Translate
}) {
	const chain = Array.isArray(attempt.transform_chain) ? attempt.transform_chain : []
	const hidden = attempt.hidden_transforms ?? 0

	return (
		<div className='flex flex-wrap items-center gap-1.5'>
			<span className='text-xs font-medium text-muted-foreground'>
				{t('requestLogs.capture.transformChain')}
			</span>
			{chain.length === 0 && hidden === 0 ?
				<span className='text-xs text-muted-foreground/70'>
					{t('requestLogs.capture.transformChainEmpty')}
				</span>
			:	<>
					{chain.map((entry, index) => (
						<Badge
							key={`${entry.scope}-${entry.transform}-${index}`}
							variant='secondary'
							className={cn(
								'h-5 gap-1 px-1.5 font-normal',
								SCOPE_CHIP_CLASSES[entry.scope] ?? 'bg-muted text-muted-foreground'
							)}
						>
							<span className='font-mono'>{entry.transform}</span>
							<span className='opacity-70'>
								{t(`requestLogs.capture.scope.${entry.scope}`)} ·{' '}
								{t(`requestLogs.capture.phase.${entry.phase}`)}
							</span>
						</Badge>
					))}
					{hidden > 0 && (
						<Badge
							variant='secondary'
							className='h-5 px-1.5 font-normal bg-muted text-muted-foreground'
						>
							{t('requestLogs.capture.hiddenTransforms', { count: hidden })}
						</Badge>
					)}
				</>
			}
		</div>
	)
}

function AttemptTabs({ attempt, t }: { attempt: CaptureAttempt; t: Translate }) {
	// RCV-F10b: the Output Stream tab exists iff frames were captured.
	const hasOutputStream = Array.isArray(attempt.downstream_sse_frames)
	// RCV-F10a: Response tab content precedence.
	const responseValue =
		attempt.downstream_response != null ? attempt.downstream_response
		: attempt.reconstructed_urp_response != null ?
			attempt.reconstructed_urp_response
		: attempt.error != null ? attempt.error
		: null
	const [tab, setTab] = useState('downstream')
	// Switching to an attempt without frames while Output Stream is active
	// falls back to the Response tab instead of a blank content area.
	const activeTab = tab === 'outputStream' && !hasOutputStream ? 'response' : tab

	return (
		<Tabs value={activeTab} onValueChange={setTab} className='flex min-h-0 flex-col'>
			<TabsList
				className={cn(
					'grid h-auto w-full p-1',
					hasOutputStream ? 'grid-cols-5' : 'grid-cols-4'
				)}
			>
				<TabsTrigger value='downstream' className='px-1.5 py-1.5 text-xs'>
					{t('requestLogs.capture.tabDownstream')}
				</TabsTrigger>
				<TabsTrigger value='upstream' className='px-1.5 py-1.5 text-xs'>
					{t('requestLogs.capture.tabUpstream')}
				</TabsTrigger>
				<TabsTrigger value='urp' className='px-1.5 py-1.5 text-xs'>
					{t('requestLogs.capture.tabUrp')}
				</TabsTrigger>
				<TabsTrigger value='response' className='px-1.5 py-1.5 text-xs'>
					{t('requestLogs.capture.tabResponse')}
				</TabsTrigger>
				{hasOutputStream && (
					<TabsTrigger value='outputStream' className='px-1.5 py-1.5 text-xs'>
						{t('requestLogs.capture.tabOutputStream')}
					</TabsTrigger>
				)}
			</TabsList>
			<TabsContent value='downstream' className='mt-2'>
				<JsonPane value={attempt.raw_input} t={t} />
			</TabsContent>
			<TabsContent value='upstream' className='mt-2'>
				<JsonPane value={attempt.upstream_request} t={t} />
			</TabsContent>
			<TabsContent value='urp' className='mt-2'>
				<JsonPane value={attempt.transformed_urp_request} t={t} />
			</TabsContent>
			<TabsContent value='response' className='mt-2'>
				<JsonPane
					value={responseValue}
					emptyLabel={
						hasOutputStream ?
							t('requestLogs.capture.responseEmptyStream')
						:	t('requestLogs.capture.empty')
					}
					t={t}
				/>
			</TabsContent>
			{hasOutputStream && (
				<TabsContent value='outputStream' className='mt-2'>
					<SseFramesPane
						frames={attempt.downstream_sse_frames ?? []}
						truncation={attempt.downstream_sse_frames_truncation}
						t={t}
					/>
				</TabsContent>
			)}
		</Tabs>
	)
}

function CopyButton({
	getText,
	disabled,
	t
}: {
	getText: () => string
	disabled?: boolean
	t: Translate
}) {
	const [copied, setCopied] = useState(false)

	const handleCopy = async () => {
		try {
			await navigator.clipboard.writeText(getText())
			setCopied(true)
			window.setTimeout(() => setCopied(false), 1600)
		} catch {
			// Clipboard API unavailable (insecure context); leave state unchanged.
		}
	}

	return (
		<Button
			type='button'
			variant='outline'
			size='sm'
			disabled={disabled}
			onClick={() => void handleCopy()}
			aria-label={t('requestLogs.capture.copy')}
			className='h-7 gap-1 bg-background/90 px-2 text-xs backdrop-blur'
		>
			<AnimatePresence mode='wait' initial={false}>
				{copied ?
					<motion.span
						key='copied'
						initial={{ opacity: 0, scale: 0.7 }}
						animate={{ opacity: 1, scale: 1, transition: expandTransition }}
						exit={{ opacity: 0, scale: 0.7, transition: collapseTransition }}
						className='inline-flex items-center gap-1 text-success'
					>
						<Check className='h-3.5 w-3.5' />
						{t('requestLogs.capture.copied')}
					</motion.span>
				:	<motion.span
						key='copy'
						initial={{ opacity: 0, scale: 0.7 }}
						animate={{ opacity: 1, scale: 1, transition: expandTransition }}
						exit={{ opacity: 0, scale: 0.7, transition: collapseTransition }}
						className='inline-flex items-center gap-1'
					>
						<Copy className='h-3.5 w-3.5' />
						{t('requestLogs.capture.copy')}
					</motion.span>
				}
			</AnimatePresence>
		</Button>
	)
}

function JsonPane({
	value,
	emptyLabel,
	t
}: {
	value: unknown
	emptyLabel?: string
	t: Translate
}) {
	const isEmpty = value == null
	return (
		<div className='relative'>
			<div className='absolute right-2 top-2 z-10'>
				<CopyButton
					disabled={isEmpty}
					getText={() => JSON.stringify(value, null, 2)}
					t={t}
				/>
			</div>
			<div className='max-h-[45vh] min-h-[8rem] overflow-auto rounded-md border bg-muted/30 p-3 pr-24 font-mono text-xs leading-relaxed sm:max-h-[50vh]'>
				{isEmpty ?
					<span className='text-muted-foreground'>
						{emptyLabel ?? t('requestLogs.capture.empty')}
					</span>
				:	<JsonNode value={value} depth={0} t={t} />}
			</div>
		</div>
	)
}

function JsonNode({
	name,
	nameIsIndex,
	value,
	depth,
	t
}: {
	name?: string
	nameIsIndex?: boolean
	value: unknown
	depth: number
	t: Translate
}) {
	if (value !== null && typeof value === 'object') {
		const isArray = Array.isArray(value)
		const entries =
			isArray ?
				(value as unknown[]).map((item, index) => [String(index), item] as const)
			:	Object.entries(value as Record<string, unknown>)
		return (
			<JsonCompositeNode
				name={name}
				nameIsIndex={nameIsIndex}
				entries={entries}
				isArray={isArray}
				depth={depth}
				t={t}
			/>
		)
	}
	return (
		<div className='break-all'>
			<NodeLabel name={name} nameIsIndex={nameIsIndex} />
			<JsonLeaf value={value} t={t} />
		</div>
	)
}

function NodeLabel({
	name,
	nameIsIndex
}: {
	name?: string
	nameIsIndex?: boolean
}) {
	if (name == null) return null
	return (
		<>
			<span className={nameIsIndex ? 'text-muted-foreground/70' : 'text-info'}>
				{nameIsIndex ? name : `"${name}"`}
			</span>
			<span className='text-muted-foreground'>: </span>
		</>
	)
}

function JsonLeaf({ value, t }: { value: unknown; t: Translate }) {
	if (typeof value === 'string') return <StringLeaf value={value} t={t} />
	if (typeof value === 'number') {
		return <span className='text-warning'>{String(value)}</span>
	}
	if (typeof value === 'boolean') {
		return <span className='text-destructive'>{String(value)}</span>
	}
	return <span className='italic text-muted-foreground'>null</span>
}

// RCV-F14a case 2: base64 image signatures with their preview media types.
const IMAGE_BASE64_SIGNATURES: ReadonlyArray<readonly [string, string]> = [
	['iVBORw0KGgo', 'image/png'],
	['/9j/', 'image/jpeg'],
	['R0lGOD', 'image/gif'],
	['UklGR', 'image/webp']
]
const MIN_BASE64_IMAGE_LENGTH = 64
const IMAGE_DATA_URL_PREFIX = /^data:image\/[a-zA-Z0-9.+-]+;base64,/

// RCV-F14a: image-bearing detection inspects only the string prefix; no
// full-string decode happens before render.
function imagePreviewSource(value: string): string | null {
	if (IMAGE_DATA_URL_PREFIX.test(value)) return value
	if (value.length >= MIN_BASE64_IMAGE_LENGTH) {
		for (const [signature, mediaType] of IMAGE_BASE64_SIGNATURES) {
			if (value.startsWith(signature)) {
				return `data:${mediaType};base64,${value}`
			}
		}
	}
	return null
}

// RCV-F14a: an undecodable payload collapses the preview without breaking
// the surrounding tree.
function ImagePreview({ src, t }: { src: string; t: Translate }) {
	const [failed, setFailed] = useState(false)
	if (failed) return null
	return (
		<img
			src={src}
			alt={t('requestLogs.capture.imagePreview')}
			loading='lazy'
			onError={() => setFailed(true)}
			className='mt-1 block max-h-40 max-w-full rounded-md border border-border/60 bg-background/60 object-contain'
		/>
	)
}

// RCV-F14: long strings render truncated; expanding is a pure view state
// change and never mutates the underlying dump data.
function StringLeaf({ value, t }: { value: string; t: Translate }) {
	const [expanded, setExpanded] = useState(false)
	const needsTruncation = value.length > STRING_TRUNCATE_LENGTH
	const shown =
		!needsTruncation || expanded ? value : value.slice(0, STRING_TRUNCATE_LENGTH)
	const previewSrc = imagePreviewSource(value)

	return (
		<span className='whitespace-pre-wrap break-all text-success'>
			&quot;{shown}
			{needsTruncation && !expanded ? '…' : ''}&quot;
			{needsTruncation && (
				<button
					type='button'
					onClick={() => setExpanded(prev => !prev)}
					aria-expanded={expanded}
					className='ml-1 rounded-sm px-1 font-sans text-[10px] text-info underline-offset-2 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring'
				>
					{expanded ?
						t('requestLogs.capture.collapse')
					:	t('requestLogs.capture.expandChars', { count: value.length })}
				</button>
			)}
			{previewSrc && <ImagePreview src={previewSrc} t={t} />}
		</span>
	)
}

function JsonCompositeNode({
	name,
	nameIsIndex,
	entries,
	isArray,
	depth,
	t
}: {
	name?: string
	nameIsIndex?: boolean
	entries: ReadonlyArray<readonly [string, unknown]>
	isArray: boolean
	depth: number
	t: Translate
}) {
	// RCV-F13: deep nodes and large collections start collapsed.
	const [expanded, setExpanded] = useState(
		() =>
			!(
				depth >= COLLAPSE_DEPTH ||
				(isArray && entries.length > ARRAY_COLLAPSE_THRESHOLD) ||
				(!isArray && entries.length > OBJECT_COLLAPSE_THRESHOLD)
			)
	)

	if (entries.length === 0) {
		return (
			<div className='break-all'>
				<NodeLabel name={name} nameIsIndex={nameIsIndex} />
				<span className='text-muted-foreground'>{isArray ? '[]' : '{}'}</span>
			</div>
		)
	}

	const summary =
		isArray ?
			`[…] ${t('requestLogs.capture.itemsSummary', { count: entries.length })}`
		:	`{…} ${t('requestLogs.capture.keysSummary', { count: entries.length })}`

	return (
		<div>
			<button
				type='button'
				onClick={() => setExpanded(prev => !prev)}
				aria-expanded={expanded}
				className='inline-flex max-w-full items-start gap-0.5 rounded-sm text-left hover:bg-accent/60 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring'
			>
				<motion.span
					animate={{ rotate: expanded ? 90 : 0 }}
					transition={expandTransition}
					className='mt-1 inline-flex shrink-0'
				>
					<ChevronRight className='h-3 w-3 text-muted-foreground' />
				</motion.span>
				<span className='break-all'>
					<NodeLabel name={name} nameIsIndex={nameIsIndex} />
					<span className='text-muted-foreground'>
						{expanded ? (isArray ? '[' : '{') : summary}
					</span>
				</span>
			</button>
			<AnimatePresence initial={false}>
				{expanded && (
					<motion.div
						initial={{ height: 0, opacity: 0 }}
						animate={{
							height: 'auto',
							opacity: 1,
							transition: expandTransition
						}}
						exit={{ height: 0, opacity: 0, transition: collapseTransition }}
						className='overflow-hidden'
					>
						<div className='ml-[5px] border-l border-border/40 pl-3'>
							{entries.map(([key, child]) => (
								<JsonNode
									key={key}
									name={key}
									nameIsIndex={isArray}
									value={child}
									depth={depth + 1}
									t={t}
								/>
							))}
						</div>
						<span className='text-muted-foreground'>{isArray ? ']' : '}'}</span>
					</motion.div>
				)}
			</AnimatePresence>
		</div>
	)
}

// RCV-F16a: virtualized frame list inside a fixed-height scroll container.
function SseFramesPane({
	frames,
	truncation,
	t
}: {
	frames: string[]
	truncation?: CaptureFrameTruncation
	t: Translate
}) {
	return (
		<div className='relative'>
			<div className='absolute right-2 top-2 z-10'>
				<CopyButton
					disabled={frames.length === 0}
					getText={() => frames.join('\n')}
					t={t}
				/>
			</div>
			<div className='rounded-md border bg-muted/30 font-mono text-xs leading-relaxed'>
				{truncation?.truncated === true && (
					<p className='mx-3 mt-3 rounded-sm border border-warning-border bg-warning-soft px-2 py-1 font-sans text-warning-foreground'>
						{t('requestLogs.capture.framesTruncated', {
							frames: truncation.omitted_frames ?? 0,
							bytes: truncation.omitted_bytes ?? 0
						})}
					</p>
				)}
				{frames.length === 0 ?
					<p className='p-3 text-muted-foreground'>
						{t('requestLogs.capture.empty')}
					</p>
				:	<Virtuoso
						style={{ height: '45vh' }}
						totalCount={frames.length}
						overscan={200}
						itemContent={index => (
							<FrameRow index={index} frame={frames[index]} t={t} />
						)}
					/>
				}
			</div>
		</div>
	)
}

const FRAME_PREVIEW_LENGTH = 160
// RCV-F16b rule 1: frames longer than this render as plain text.
const HIGHLIGHT_MAX_FRAME_CHARS = 4096

interface FrameToken {
	text: string
	className?: string
}

const SSE_FIELD_NAMES = new Set(['event', 'data', 'id', 'retry'])
const TOKEN_FIELD = 'text-info'
const TOKEN_KEY = 'text-info'
const TOKEN_STRING = 'text-success'
const TOKEN_NUMBER = 'text-warning'
const TOKEN_BOOLEAN = 'text-destructive'
const TOKEN_NULL = 'italic text-muted-foreground'

function isJsonNumberChar(ch: string): boolean {
	return (
		(ch >= '0' && ch <= '9') ||
		ch === '.' ||
		ch === 'e' ||
		ch === 'E' ||
		ch === '+' ||
		ch === '-'
	)
}

function isWordChar(ch: string): boolean {
	return /[A-Za-z0-9_]/.test(ch)
}

// RCV-F16b: single-pass, non-backtracking tokenizer for `data:` payloads.
// RCV-F16c invariant: concatenating token texts reproduces the input exactly,
// so unrecognized spans are emitted verbatim as unclassified tokens.
function tokenizeJsonPayload(payload: string, tokens: FrameToken[]) {
	const n = payload.length
	let i = 0
	let plainStart = 0
	const flushPlain = (end: number) => {
		if (end > plainStart) tokens.push({ text: payload.slice(plainStart, end) })
	}
	while (i < n) {
		const ch = payload[i]
		if (ch === '"') {
			let j = i + 1
			while (j < n) {
				if (payload[j] === '\\') {
					j += 2
					continue
				}
				if (payload[j] === '"') break
				j++
			}
			const closed = j < n
			const end = closed ? j + 1 : n
			let k = end
			while (k < n && payload[k] === ' ') k++
			const isKey = closed && payload[k] === ':'
			flushPlain(i)
			tokens.push({
				text: payload.slice(i, end),
				className: isKey ? TOKEN_KEY : TOKEN_STRING
			})
			plainStart = end
			i = end
			continue
		}
		const prev = i > 0 ? payload[i - 1] : ''
		const boundary = i === 0 || !isWordChar(prev)
		if (
			boundary &&
			((ch >= '0' && ch <= '9') ||
				(ch === '-' && i + 1 < n && payload[i + 1] >= '0' && payload[i + 1] <= '9'))
		) {
			let j = i + 1
			while (j < n && isJsonNumberChar(payload[j])) j++
			flushPlain(i)
			tokens.push({ text: payload.slice(i, j), className: TOKEN_NUMBER })
			plainStart = j
			i = j
			continue
		}
		if (boundary) {
			let matchedLiteral = false
			for (const [literal, className] of [
				['true', TOKEN_BOOLEAN],
				['false', TOKEN_BOOLEAN],
				['null', TOKEN_NULL]
			] as const) {
				if (
					payload.startsWith(literal, i) &&
					(i + literal.length === n || !isWordChar(payload[i + literal.length]))
				) {
					flushPlain(i)
					tokens.push({ text: literal, className })
					plainStart = i + literal.length
					i = plainStart
					matchedLiteral = true
					break
				}
			}
			if (matchedLiteral) {
				continue
			}
		}
		i++
	}
	flushPlain(n)
}

function tokenizeSseFrame(frame: string): FrameToken[] {
	const tokens: FrameToken[] = []
	const lines = frame.split('\n')
	for (let lineIndex = 0; lineIndex < lines.length; lineIndex++) {
		const line = lines[lineIndex]
		const colon = line.indexOf(':')
		if (colon > 0 && SSE_FIELD_NAMES.has(line.slice(0, colon))) {
			tokens.push({ text: line.slice(0, colon), className: TOKEN_FIELD })
			tokens.push({ text: ':' })
			tokenizeJsonPayload(line.slice(colon + 1), tokens)
		} else if (line.length > 0) {
			tokens.push({ text: line })
		}
		if (lineIndex < lines.length - 1) {
			tokens.push({ text: '\n' })
		}
	}
	return tokens
}

// RCV-F16b rule 3: tokenization runs only while the row is mounted and is
// memoized per frame string.
function HighlightedFrame({ frame }: { frame: string }) {
	const tokens = useMemo(() => tokenizeSseFrame(frame), [frame])
	return (
		<span className='whitespace-pre-wrap break-all'>
			{tokens.map((token, index) =>
				token.className ?
					<span key={index} className={token.className}>
						{token.text}
					</span>
				:	<span key={index}>{token.text}</span>
			)}
		</span>
	)
}

function FrameContent({ frame }: { frame: string }) {
	if (frame.length > HIGHLIGHT_MAX_FRAME_CHARS) {
		return <span className='whitespace-pre-wrap break-all'>{frame}</span>
	}
	return <HighlightedFrame frame={frame} />
}

// RCV-F16: one row per frame with a visible index; rows expand independently.
function FrameRow({
	index,
	frame,
	t
}: {
	index: number
	frame: string
	t: Translate
}) {
	const [expanded, setExpanded] = useState(false)
	const isLong = frame.length > FRAME_PREVIEW_LENGTH || frame.includes('\n')

	return (
		<div className='flex items-start gap-2 border-b border-border/30 px-3 py-0.5'>
			<span className='select-none pt-px text-[10px] tabular-nums text-muted-foreground/60'>
				{index}
			</span>
			<div className='min-w-0 flex-1'>
				{!isLong ?
					<FrameContent frame={frame} />
				:	<>
						<button
							type='button'
							onClick={() => setExpanded(prev => !prev)}
							aria-expanded={expanded}
							className='block w-full rounded-sm text-left hover:bg-accent/60 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring'
						>
							{expanded ?
								<span className='font-sans text-[10px] text-info'>
									{t('requestLogs.capture.collapse')}
								</span>
							:	<span className='block truncate'>
									{/* RCV-F16b rule 2: collapsed previews stay plain text. */}
									{frame.slice(0, FRAME_PREVIEW_LENGTH)}
									<span className='ml-1 font-sans text-[10px] text-info'>
										{t('requestLogs.capture.expandChars', {
											count: frame.length
										})}
									</span>
								</span>
							}
						</button>
						<AnimatePresence initial={false}>
							{expanded && (
								<motion.div
									initial={{ height: 0, opacity: 0 }}
									animate={{
										height: 'auto',
										opacity: 1,
										transition: expandTransition
									}}
									exit={{
										height: 0,
										opacity: 0,
										transition: collapseTransition
									}}
									className='overflow-hidden'
								>
									<FrameContent frame={frame} />
								</motion.div>
							)}
						</AnimatePresence>
					</>
				}
			</div>
		</div>
	)
}
