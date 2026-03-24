import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Activity, Check, Loader2, Play, X, Zap } from 'lucide-react'
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
	Tooltip,
	TooltipContent,
	TooltipProvider,
	TooltipTrigger
} from '@/components/ui/tooltip'
import { api } from '@/lib/api'
import { SWR_KEYS } from '@/lib/swr'
import { mutate } from 'swr'
import { Virtuoso } from 'react-virtuoso'
import type { ChannelTestResult } from '@/lib/api'

type ChannelTestState = Record<
	string,
	{ status: 'idle' | 'testing' | 'passed' | 'failed'; latency_ms?: number; error?: string }
>

type ChannelTestDialogProps = {
	open: boolean
	onOpenChange: (open: boolean) => void
	providerId: string
	channelId: string
	channelName: string
	providerName: string
	models: string[]
}

export function ChannelTestDialog({
	open,
	onOpenChange,
	providerId,
	channelId,
	channelName,
	providerName,
	models
}: ChannelTestDialogProps) {
	const { t } = useTranslation()
	const [testState, setTestState] = useState<ChannelTestState>({})
	const [testingAll, setTestingAll] = useState(false)
	const abortRef = useRef(false)

	useEffect(() => {
		if (open) {
			setTestState({})
			setTestingAll(false)
			abortRef.current = false
		} else {
			abortRef.current = true
		}
	}, [open])

	const runSingleTest = async (model: string) => {
		setTestState(prev => ({
			...prev,
			[model]: { status: 'testing' }
		}))
		try {
			const result: ChannelTestResult = await api.testChannel(
				providerId,
				channelId,
				model
			)
			setTestState(prev => ({
				...prev,
				[model]: {
					status: result.success ? 'passed' : 'failed',
					latency_ms: result.latency_ms,
					error: result.error ?? undefined
				}
			}))
		} catch (err) {
			setTestState(prev => ({
				...prev,
				[model]: {
					status: 'failed',
					error: err instanceof Error ? err.message : 'Unknown error'
				}
			}))
		}
	}

	const runAllTests = async () => {
		setTestingAll(true)
		abortRef.current = false
		for (const model of models) {
			if (abortRef.current) break
			await runSingleTest(model)
		}
		setTestingAll(false)
		mutate(SWR_KEYS.PROVIDERS)
	}

	const testedCount = Object.values(testState).filter(
		state => state.status === 'passed' || state.status === 'failed'
	).length
	const passedCount = Object.values(testState).filter(
		state => state.status === 'passed'
	).length

	return (
		<Dialog open={open} onOpenChange={onOpenChange}>
			<DialogContent className='max-h-[85vh] flex flex-col overflow-hidden max-w-2xl'>
				<DialogHeader>
					<DialogTitle className='flex items-center gap-2'>
						<Activity className='h-5 w-5 text-muted-foreground' />
						{t('providers.testChannelTitle')}
					</DialogTitle>
					<DialogDescription>
						{t('providers.testChannelDesc', {
							channel: channelName,
							provider: providerName
						})}
					</DialogDescription>
				</DialogHeader>

				<div className='flex items-center justify-between'>
					<div className='text-sm text-muted-foreground'>
						{testedCount > 0 && (
							<span>
								{passedCount}/{testedCount}{' '}
								{t('providers.testPassed').toLowerCase()}
							</span>
						)}
					</div>
					<Button
						size='sm'
						variant='outline'
						disabled={testingAll || models.length === 0}
						onClick={runAllTests}
					>
						{testingAll ?
							<>
								<Loader2 className='h-4 w-4 mr-2 animate-spin' />
								{t('providers.testAllRunning')}
							</>
						:	<>
								<Play className='h-4 w-4 mr-2' />
								{t('providers.testAll')} ({models.length})
							</>
						}
					</Button>
				</div>

				<div className='border rounded-lg overflow-hidden'>
					{models.length === 0 ?
						<div className='text-sm text-muted-foreground py-8 text-center'>
							{t('providers.validationAtLeastOneModel')}
						</div>
					:	<Virtuoso
							style={{ height: Math.min(models.length * 44, 352) }}
							data={models}
							computeItemKey={(_idx, model) => model}
							itemContent={(_idx, model) => {
								const state = testState[model]
								const status = state?.status ?? 'idle'
								return (
									<div className='flex h-11 items-center gap-3 px-3 border-b last:border-b-0 hover:bg-muted/50 transition-colors'>
										<span className='text-sm font-mono truncate min-w-0 flex-1'>
											{model}
										</span>

										<span className='flex items-center gap-2 shrink-0'>
											{status === 'passed' && (
												<Badge className='bg-emerald-600/15 text-emerald-700 hover:bg-emerald-600/15 dark:bg-emerald-500/15 dark:text-emerald-400 border-0 gap-1'>
													<Check className='h-3 w-3' />
													{t('providers.testLatency', {
														ms: state?.latency_ms ?? 0
													})}
												</Badge>
											)}
											{status === 'failed' && (
												<TooltipProvider delayDuration={0}>
													<Tooltip>
														<TooltipTrigger asChild>
															<Badge className='bg-red-500/15 text-red-700 hover:bg-red-500/15 dark:bg-red-500/15 dark:text-red-400 border-0 gap-1'>
																<X className='h-3 w-3' />
																{state?.latency_ms != null ?
																	t('providers.testLatency', { ms: state.latency_ms })
																:	t('providers.testFailed')
																}
															</Badge>
														</TooltipTrigger>
														{state?.error && (
															<TooltipContent side='left' className='max-w-xs'>
																{state.error}
															</TooltipContent>
														)}
													</Tooltip>
												</TooltipProvider>
											)}
											{status === 'testing' && (
												<Badge variant='secondary' className='gap-1 border-0'>
													<Loader2 className='h-3 w-3 animate-spin' />
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
											className='h-7 px-2 shrink-0'
											disabled={status === 'testing' || testingAll}
											onClick={() => runSingleTest(model)}
										>
											{status === 'testing' ?
												<Loader2 className='h-3.5 w-3.5 animate-spin' />
											:	<Zap className='h-3.5 w-3.5' />
											}
										</Button>
									</div>
								)
							}}
						/>
					}
				</div>
			</DialogContent>
		</Dialog>
	)
}
