import { useState } from 'react'
import { useTranslation } from 'react-i18next'
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
	const responseSource =
		attempt.downstream_response != null ? ('response' as const)
		: Array.isArray(attempt.downstream_sse_frames) ? ('frames' as const)
		: attempt.error != null ? ('error' as const)
		: ('empty' as const)

	return (
		<Tabs defaultValue='downstream' className='flex min-h-0 flex-col'>
			<TabsList className='grid h-auto w-full grid-cols-4 p-1'>
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
				{responseSource === 'frames' ?
					<SseFramesPane
						frames={attempt.downstream_sse_frames ?? []}
						truncation={attempt.downstream_sse_frames_truncation}
						t={t}
					/>
				:	<JsonPane
						value={
							responseSource === 'response' ? attempt.downstream_response
							: responseSource === 'error' ?
								attempt.error
							:	null
						}
						t={t}
					/>
				}
			</TabsContent>
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

function JsonPane({ value, t }: { value: unknown; t: Translate }) {
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
						{t('requestLogs.capture.empty')}
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

// RCV-F14: long strings render truncated; expanding is a pure view state
// change and never mutates the underlying dump data.
function StringLeaf({ value, t }: { value: string; t: Translate }) {
	const [expanded, setExpanded] = useState(false)
	const needsTruncation = value.length > STRING_TRUNCATE_LENGTH
	const shown =
		!needsTruncation || expanded ? value : value.slice(0, STRING_TRUNCATE_LENGTH)

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
				className='inline-flex max-w-full items-baseline gap-0.5 rounded-sm text-left hover:bg-accent/60 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring'
			>
				<motion.span
					animate={{ rotate: expanded ? 90 : 0 }}
					transition={expandTransition}
					className='inline-flex self-center'
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
			<div className='max-h-[45vh] min-h-[8rem] overflow-auto rounded-md border bg-muted/30 p-3 pr-24 font-mono text-xs leading-relaxed sm:max-h-[50vh]'>
				{truncation?.truncated === true && (
					<p className='mb-2 rounded-sm border border-warning-border bg-warning-soft px-2 py-1 font-sans text-warning-foreground'>
						{t('requestLogs.capture.framesTruncated', {
							frames: truncation.omitted_frames ?? 0,
							bytes: truncation.omitted_bytes ?? 0
						})}
					</p>
				)}
				{frames.length === 0 ?
					<span className='text-muted-foreground'>
						{t('requestLogs.capture.empty')}
					</span>
				:	frames.map((frame, index) => (
						<FrameRow key={index} index={index} frame={frame} t={t} />
					))
				}
			</div>
		</div>
	)
}

const FRAME_PREVIEW_LENGTH = 160

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
		<div className='flex items-start gap-2 border-b border-border/30 py-0.5 last:border-b-0'>
			<span className='select-none pt-px text-[10px] tabular-nums text-muted-foreground/60'>
				{index}
			</span>
			<div className='min-w-0 flex-1'>
				{!isLong ?
					<span className='break-all'>{frame}</span>
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
									<span className='whitespace-pre-wrap break-all'>{frame}</span>
								</motion.div>
							)}
						</AnimatePresence>
					</>
				}
			</div>
		</div>
	)
}
