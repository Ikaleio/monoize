import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
	Activity,
	Check,
	Layers3,
	ListRestart,
	Loader2,
	RotateCcw,
	Square,
	X,
	Zap
} from 'lucide-react'
import { mutate } from 'swr'
import { Virtuoso } from 'react-virtuoso'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { ButtonGroup } from '@/components/ui/button-group'
import { Checkbox } from '@/components/ui/checkbox'
import { CopyButton } from '@/components/ui/copy-button'
import {
	Dialog,
	DialogContent,
	DialogDescription,
	DialogHeader,
	DialogTitle
} from '@/components/ui/dialog'
import {
	Field,
	FieldContent,
	FieldDescription,
	FieldLabel
} from '@/components/ui/field'
import { StatusBadge } from '@/components/ui/status'
import { api } from '@/lib/api'
import { apiErrorMessage } from '@/lib/api-error'
import { SWR_KEYS } from '@/lib/swr'
import type { ProviderType } from '@/lib/api'
import {
	CHANNEL_TEST_CONCURRENCY,
	ChannelTestContext,
	useChannelTests,
	type ChannelTestContextValue,
	type ChannelTestRun,
	type ChannelTestTarget,
	type ModelTestResult
} from './channel-test-context'

const STREAM_TEST_SUPPORTED: Record<ProviderType, boolean> = {
	responses: true,
	chat_completion: true,
	messages: true,
	gemini: true,
	openai_image: false,
	openrouter_image: false,
	replicate: false,
	system_one: false,
	openai_decisions: false
}

/**
 * CP-FE-9a: page-level owner of Channel test runs. Results survive closing the
 * dialog, and a run keeps going in the background until it finishes or is stopped.
 */
export function ChannelTestProvider({ children }: { children: React.ReactNode }) {
	const { t } = useTranslation()
	const [runs, setRuns] = useState<Record<string, ChannelTestRun>>({})
	const [target, setTarget] = useState<ChannelTestTarget | null>(null)
	const [open, setOpen] = useState(false)
	const [openSeq, setOpenSeq] = useState(0)
	const controllers = useRef(new Map<string, AbortController>())

	useEffect(() => {
		const active = controllers.current
		return () => {
			for (const controller of active.values()) controller.abort()
		}
	}, [])

	const setResult = useCallback((channelId: string, model: string, result: ModelTestResult | undefined) => {
		setRuns(previous => {
			const run = previous[channelId]
			if (!run) return previous
			const results = { ...run.results }
			if (result) results[model] = result
			else delete results[model]
			return { ...previous, [channelId]: { ...run, results } }
		})
	}, [])

	const start = useCallback<ChannelTestContextValue['start']>((runTarget, models, mode, stream) => {
		const { channelId } = runTarget
		if (controllers.current.has(channelId) || models.length === 0) return
		const controller = new AbortController()
		controllers.current.set(channelId, controller)
		setRuns(previous => {
			const run = previous[channelId]
			const results = { ...run?.results }
			for (const model of models) delete results[model]
			return {
				...previous,
				[channelId]: {
					results,
					running: mode,
					lastMode: mode === 'single' ? run?.lastMode ?? 'sequential' : mode
				}
			}
		})

		const testOne = async (model: string) => {
			if (controller.signal.aborted) return
			setResult(channelId, model, { status: 'testing' })
			try {
				const result = await api.testChannel(runTarget.providerId, channelId, model, stream, controller.signal)
				setResult(channelId, model, {
					status: result.success ? 'passed' : 'failed',
					latency_ms: result.latency_ms,
					http_status: result.http_status,
					error_code: result.error_code,
					error_type: result.error_type,
					error: result.error ?? undefined
				})
			} catch (error) {
				setResult(
					channelId,
					model,
					controller.signal.aborted ? undefined : { status: 'failed', error: apiErrorMessage(error, t) }
				)
			}
		}

		void (async () => {
			try {
				if (mode === 'concurrent') {
					const queue = [...models]
					await Promise.all(
						Array.from({ length: Math.min(CHANNEL_TEST_CONCURRENCY, queue.length) }, async () => {
							for (let model = queue.shift(); model !== undefined; model = queue.shift()) {
								await testOne(model)
							}
						})
					)
				} else {
					for (const model of models) await testOne(model)
				}
			} finally {
				controllers.current.delete(channelId)
				setRuns(previous => previous[channelId]
					? { ...previous, [channelId]: { ...previous[channelId], running: null } }
					: previous)
				mutate(SWR_KEYS.PROVIDERS)
			}
		})()
	}, [setResult, t])

	const stop = useCallback((channelId: string) => {
		controllers.current.get(channelId)?.abort()
	}, [])

	const openTest = useCallback((next: ChannelTestTarget) => {
		setTarget(next)
		setOpenSeq(previous => previous + 1)
		setOpen(true)
	}, [])

	const value = useMemo(() => ({ runs, openTest, start, stop }), [runs, openTest, start, stop])

	return (
		<ChannelTestContext.Provider value={value}>
			{children}
			{/* Keyed per open so dialog-local state (stream checkbox) starts fresh (CP-FE-8). */}
			{target ? <ChannelTestDialog key={openSeq} open={open} onOpenChange={setOpen} target={target} /> : null}
		</ChannelTestContext.Provider>
	)
}

const STATUS_ORDER: Record<string, number> = { failed: 0, idle: 1, testing: 1, passed: 2 }
const EMPTY_RESULTS: ChannelTestRun['results'] = {}

function ChannelTestDialog({
	open,
	onOpenChange,
	target
}: {
	open: boolean
	onOpenChange: (open: boolean) => void
	target: ChannelTestTarget
}) {
	const { t } = useTranslation()
	const { runs, start, stop } = useChannelTests()
	const streamCapable = STREAM_TEST_SUPPORTED[target.providerType]
	const [stream, setStream] = useState(streamCapable)
	const run = runs[target.channelId]
	const results = run?.results ?? EMPTY_RESULTS
	const running = run?.running ?? null

	const sortedModels = useMemo(
		() => [...target.models].sort((left, right) =>
			STATUS_ORDER[results[left]?.status ?? 'idle'] - STATUS_ORDER[results[right]?.status ?? 'idle']
			|| left.localeCompare(right)),
		[target.models, results]
	)
	const failedModels = target.models.filter(model => results[model]?.status === 'failed')
	const passedCount = target.models.filter(model => results[model]?.status === 'passed').length
	const untestedCount = target.models.length - passedCount - failedModels.length

	return (
		<Dialog open={open} onOpenChange={onOpenChange}>
			<DialogContent
				className='flex max-h-[85vh] max-w-2xl flex-col overflow-hidden'
				onInteractOutside={event => { if (running) event.preventDefault() }}
			>
				<DialogHeader>
					<DialogTitle className='flex items-center gap-2'>
						<Activity className='size-5 text-muted-foreground' aria-hidden='true' />
						{t('providers.testChannelTitle')}
					</DialogTitle>
					<DialogDescription>
						{t('providers.testChannelDesc', {
							channel: target.channelName,
							provider: target.providerName
						})}
					</DialogDescription>
				</DialogHeader>

				<div className='flex flex-wrap items-center justify-between gap-3'>
					<Field orientation='horizontal' data-disabled={!streamCapable} className='w-auto'>
						<Checkbox
							id='channel-test-stream'
							checked={stream}
							disabled={!streamCapable || running !== null}
							onCheckedChange={checked => setStream(checked === true)}
						/>
						<FieldContent>
							<FieldLabel htmlFor='channel-test-stream'>
								{t('providers.streamTest')}
							</FieldLabel>
							<FieldDescription>
								{streamCapable ?
									t('providers.streamTestDesc')
								: 	t('providers.streamTestUnsupported')}
							</FieldDescription>
						</FieldContent>
					</Field>
					<div className='flex flex-wrap items-center gap-2'>
						{running ?
							<Button size='sm' variant='outline' onClick={() => stop(target.channelId)}>
								<Square data-icon='inline-start' />
								{t('providers.testStop')}
							</Button>
						: 	<>
								<ButtonGroup aria-label={t('providers.testModes')}>
									<Button
										size='sm'
										variant='outline'
										disabled={target.models.length === 0}
										onClick={() => start(target, target.models, 'sequential', stream)}
									>
										<ListRestart data-icon='inline-start' />
										{t('providers.testSequential')}
									</Button>
									<Button
										size='sm'
										variant='outline'
										disabled={target.models.length === 0}
										onClick={() => start(target, target.models, 'concurrent', stream)}
									>
										<Layers3 data-icon='inline-start' />
										{t('providers.testConcurrent')}
									</Button>
								</ButtonGroup>
								<span className='text-xs text-muted-foreground'>
									{t('providers.testConcurrencyLimit', { count: CHANNEL_TEST_CONCURRENCY })}
								</span>
								<Button
									size='sm'
									variant='outline'
									disabled={failedModels.length === 0}
									onClick={() => start(target, failedModels, run?.lastMode ?? 'sequential', stream)}
								>
									<RotateCcw data-icon='inline-start' />
									{t('providers.testRetryFailed')}
								</Button>
							</>
						}
					</div>
				</div>

				<div className='flex min-h-5 items-center gap-2 text-sm text-muted-foreground' aria-live='polite'>
					{running ? <Loader2 className='size-3.5 animate-spin' aria-hidden='true' /> : null}
					{t('providers.testSummary', {
						passed: passedCount,
						failed: failedModels.length,
						untested: untestedCount
					})}
				</div>

				<div className='min-h-0 overflow-hidden rounded-lg border'>
					{target.models.length === 0 ?
						<div className='py-8 text-center text-sm text-muted-foreground'>
							{t('providers.validationAtLeastOneModel')}
						</div>
					: 	<Virtuoso
							style={{ height: Math.min(Math.max(target.models.length * 44, 88), 352) }}
							data={sortedModels}
							computeItemKey={(_index, model) => model}
							itemContent={(_index, model) => {
								const state = results[model]
								const status = state?.status ?? 'idle'
								return (
									<div className='flex flex-col border-b last:border-b-0'>
										<div className='flex min-h-11 items-center gap-3 px-3 py-2 transition-colors hover:bg-muted/50'>
											<span className='min-w-0 flex-1 truncate font-mono text-sm'>
												{model}
											</span>
											<span className='flex shrink-0 items-center gap-2'>
												{status === 'passed' && (
													<StatusBadge variant='success' className='gap-1'>
														<Check className='size-3' aria-hidden='true' />
														{t('providers.testPassedLatency', { ms: state?.latency_ms ?? 0 })}
													</StatusBadge>
												)}
												{status === 'failed' && (
													<StatusBadge variant='destructive' className='gap-1'>
														<X className='size-3' aria-hidden='true' />
														{state?.latency_ms != null ?
															t('providers.testFailedLatency', { ms: state.latency_ms })
														: 	t('providers.testFailed')}
													</StatusBadge>
												)}
												{status === 'testing' && (
													<Badge variant='secondary' className='gap-1 border-0'>
														<Loader2 className='size-3 animate-spin' aria-hidden='true' />
														{t('providers.testing')}
													</Badge>
												)}
												{status === 'idle' && (
													<span className='text-xs text-muted-foreground'>
														{t('providers.testIdle')}
													</span>
												)}
											</span>
											<Button
												variant='ghost'
												size='sm'
												className='h-7 shrink-0 px-2'
												aria-label={t('providers.testModelAria', { model })}
												disabled={running !== null}
												onClick={() => start(target, [model], 'single', stream)}
											>
												{status === 'testing' ?
													<Loader2 className='animate-spin' aria-hidden='true' />
												: 	<Zap aria-hidden='true' />}
											</Button>
										</div>
										{status === 'failed' && state?.error && (
											<div className='flex items-start gap-2 border-t bg-error-soft px-3 py-2'>
												<div className='flex min-w-0 flex-1 flex-col gap-1'>
													<div className='flex flex-wrap items-center gap-1.5'>
														{state.http_status != null && (
															<Badge variant='outline'>HTTP {state.http_status}</Badge>
														)}
														{state.error_code && (
															<Badge variant='outline' className='font-mono'>
																{state.error_code}
															</Badge>
														)}
														{state.error_type && (
															<Badge variant='outline' className='font-mono'>
																{state.error_type}
															</Badge>
														)}
													</div>
													<code className='break-words whitespace-pre-wrap font-mono text-sm leading-relaxed text-error-foreground'>
														{state.error}
													</code>
												</div>
												<CopyButton value={state.error} label={t('providers.copyTestError')} />
											</div>
										)}
									</div>
								)
							}}
						/>}
				</div>
			</DialogContent>
		</Dialog>
	)
}
