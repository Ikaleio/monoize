import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
	Activity,
	Check,
	Clipboard,
	Layers3,
	ListRestart,
	Loader2,
	X,
	Zap
} from 'lucide-react'
import { toast } from 'sonner'
import { mutate } from 'swr'
import { Virtuoso } from 'react-virtuoso'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { ButtonGroup } from '@/components/ui/button-group'
import { Checkbox } from '@/components/ui/checkbox'
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
import { SWR_KEYS } from '@/lib/swr'
import type { ChannelTestResult, ProviderType } from '@/lib/api'

type ChannelTestState = Record<
	string,
	{
		status: 'idle' | 'testing' | 'passed' | 'failed'
		latency_ms?: number
		http_status?: number | null
		error_code?: string | null
		error_type?: string | null
		error?: string
	}
>

type TestMode = 'sequential' | 'concurrent' | null

type ChannelTestDialogProps = {
	open: boolean
	onOpenChange: (open: boolean) => void
	providerId: string
	channelId: string
	channelName: string
	providerName: string
	providerType: ProviderType
	models: string[]
}

export function ChannelTestDialog({
	open,
	onOpenChange,
	providerId,
	channelId,
	channelName,
	providerName,
	providerType,
	models
}: ChannelTestDialogProps) {
	const { t } = useTranslation()
	const streamCapable = providerType !== 'openai_image' && providerType !== 'replicate'
	const [stream, setStream] = useState(streamCapable)
	const [testState, setTestState] = useState<ChannelTestState>({})
	const [testMode, setTestMode] = useState<TestMode>(null)
	const testingAll = testMode !== null

	useEffect(() => {
		if (!open) return
		setTestState({})
		setTestMode(null)
		setStream(streamCapable)
	}, [open, streamCapable])

	const runSingleTest = async (
		model: string,
		streamMode = stream,
		revalidateProvider = true
	) => {
		setTestState(previous => ({
			...previous,
			[model]: { status: 'testing' }
		}))
		try {
			const result: ChannelTestResult = await api.testChannel(
				providerId,
				channelId,
				model,
				streamMode
			)
			setTestState(previous => ({
				...previous,
				[model]: {
					status: result.success ? 'passed' : 'failed',
					latency_ms: result.latency_ms,
					http_status: result.http_status,
					error_code: result.error_code,
					error_type: result.error_type,
					error: result.error ?? undefined
				}
			}))
		} catch (error) {
			setTestState(previous => ({
				...previous,
				[model]: {
					status: 'failed',
					error: error instanceof Error ? error.message : t('common.error')
				}
			}))
		} finally {
			if (revalidateProvider) mutate(SWR_KEYS.PROVIDERS)
		}
	}

	const runSequentialTests = async () => {
		setTestMode('sequential')
		const streamMode = stream
		try {
			for (const model of models) {
				await runSingleTest(model, streamMode, false)
			}
		} finally {
			setTestMode(null)
			mutate(SWR_KEYS.PROVIDERS)
		}
	}

	const runConcurrentTests = async () => {
		setTestMode('concurrent')
		const streamMode = stream
		try {
			await Promise.all(
				models.map(model => runSingleTest(model, streamMode, false))
			)
		} finally {
			setTestMode(null)
			mutate(SWR_KEYS.PROVIDERS)
		}
	}

	const testedCount = Object.values(testState).filter(
		state => state.status === 'passed' || state.status === 'failed'
	).length
	const passedCount = Object.values(testState).filter(
		state => state.status === 'passed'
	).length

	const copyError = async (error: string) => {
		await navigator.clipboard.writeText(error)
		toast.success(t('providers.testErrorCopied'))
	}

	return (
		<Dialog open={open} onOpenChange={onOpenChange}>
			<DialogContent className='flex max-h-[85vh] max-w-2xl flex-col overflow-hidden'>
				<DialogHeader>
					<DialogTitle className='flex items-center gap-2'>
						<Activity className='size-5 text-muted-foreground' />
						{t('providers.testChannelTitle')}
					</DialogTitle>
					<DialogDescription>
						{t('providers.testChannelDesc', {
							channel: channelName,
							provider: providerName
						})}
					</DialogDescription>
				</DialogHeader>

				<div className='flex flex-wrap items-center justify-between gap-3'>
					<Field orientation='horizontal' data-disabled={!streamCapable} className='w-auto'>
						<Checkbox
							id='channel-test-stream'
							checked={stream}
							disabled={!streamCapable || testingAll}
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
					<ButtonGroup aria-label={t('providers.testModes')}>
						<Button
							size='sm'
							variant='outline'
							disabled={testingAll || models.length === 0}
							onClick={runSequentialTests}
						>
							{testMode === 'sequential' ?
								<Loader2 data-icon='inline-start' className='animate-spin' />
							: 	<ListRestart data-icon='inline-start' />}
							{t('providers.testSequential')}
						</Button>
						<Button
							size='sm'
							variant='outline'
							disabled={testingAll || models.length === 0}
							onClick={runConcurrentTests}
						>
							{testMode === 'concurrent' ?
								<Loader2 data-icon='inline-start' className='animate-spin' />
							: 	<Layers3 data-icon='inline-start' />}
							{t('providers.testConcurrent')}
						</Button>
					</ButtonGroup>
				</div>

				<div className='min-h-5 text-sm text-muted-foreground'>
					{testedCount > 0 && (
						<span>
							{t('providers.testProgress', {
								passed: passedCount,
								tested: testedCount
							})}
						</span>
					)}
				</div>

				<div className='overflow-hidden rounded-lg border'>
					{models.length === 0 ?
						<div className='py-8 text-center text-sm text-muted-foreground'>
							{t('providers.validationAtLeastOneModel')}
						</div>
					: 	<Virtuoso
							style={{ height: Math.min(Math.max(models.length * 44, 88), 352) }}
							data={models}
							computeItemKey={(_index, model) => model}
							itemContent={(_index, model) => {
								const state = testState[model]
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
														<Check className='size-3' />
														{t('providers.testLatency', {
															ms: state?.latency_ms ?? 0
														})}
													</StatusBadge>
												)}
												{status === 'failed' && (
													<StatusBadge variant='destructive' className='gap-1'>
														<X className='size-3' />
														{state?.latency_ms != null ?
															t('providers.testLatency', { ms: state.latency_ms })
														: 	t('providers.testFailed')}
													</StatusBadge>
												)}
												{status === 'testing' && (
													<Badge variant='secondary' className='gap-1 border-0'>
														<Loader2 className='size-3 animate-spin' />
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
												disabled={status === 'testing' || testingAll}
												onClick={() => runSingleTest(model)}
											>
												{status === 'testing' ?
													<Loader2 className='animate-spin' />
												: 	<Zap />}
											</Button>
										</div>
										{status === 'failed' && state?.error && (
											<div className='flex items-start gap-2 border-t bg-destructive/5 px-3 py-2'>
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
													<code className='break-words whitespace-pre-wrap font-mono text-sm leading-relaxed text-destructive'>
														{state.error}
													</code>
												</div>
												<Button
													variant='ghost'
													size='icon'
													className='size-8 shrink-0'
													aria-label={t('providers.copyTestError')}
													onClick={() => copyError(state.error!)}
												>
													<Clipboard />
												</Button>
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
