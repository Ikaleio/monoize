import { useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
	ArrowDown,
	ArrowUp,
	CheckCircle2,
	ChevronRight,
	CircleDollarSign,
	CloudDownload,
	Plus,
	RefreshCw,
	Search,
	Settings2,
	Trash2
} from 'lucide-react'
import { toast } from 'sonner'
import { ModelBadge } from '@/components/ModelBadge'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
	Dialog,
	DialogContent,
	DialogDescription,
	DialogFooter,
	DialogHeader,
	DialogTitle
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Skeleton } from '@/components/ui/skeleton'
import {
	deleteBillingRateOptimistic,
	syncModelMetadata,
	updatePricingProfilePatternsOptimistic,
	upsertBillingRateOptimistic,
	useBillingRates,
	useModelMetadata,
	usePricingProfilePatterns
} from '@/lib/swr'
import type { BillingRateRecord, PricingProfilePattern } from '@/lib/api'
import { cn } from '@/lib/utils'

type UsageClass = 'input_uncached' | 'cache_read' | 'output'

const visibleUsageClasses: Array<{ id: UsageClass; label: string }> = [
	{ id: 'input_uncached', label: 'Input' },
	{ id: 'cache_read', label: 'Cache read' },
	{ id: 'output', label: 'Output' }
]

function nanoToPerMillion(value?: string | null): string {
	if (value == null) return '—'
	const number = Number(value)
	if (!Number.isFinite(number)) return '—'
	const price = number / 1000
	if (price === 0) return '$0'
	if (price < 0.01) return `$${price.toFixed(4)}`
	return `$${price.toLocaleString(undefined, { maximumFractionDigits: 4 })}`
}

function nanoToInput(value?: string | null): string {
	if (value == null || !Number.isFinite(Number(value))) return ''
	return String(Number(value) / 1000)
}

function perMillionToNano(value: string): string {
	const number = Number(value)
	if (!Number.isFinite(number) || number < 0) throw new Error('Price must be zero or greater')
	return String(Math.round(number * 1000))
}

function effectiveRate(rates: BillingRateRecord[], usageClass: UsageClass) {
	return rates
		.filter(rate => rate.enabled && rate.rate_kind === 'token' && rate.usage_class === usageClass)
		.sort((a, b) => {
			if (a.source === 'manual' && b.source !== 'manual') return -1
			if (b.source === 'manual' && a.source !== 'manual') return 1
			return b.priority - a.priority
		})[0]
}

function safeIdPart(value: string) {
	return value.toLowerCase().replace(/[^a-z0-9._-]+/g, '-')
}

export function BillingProfilesTab() {
	const { i18n } = useTranslation()
	const zh = i18n.language.startsWith('zh')
	const c = (zhText: string, enText: string) => zh ? zhText : enText
	const { data: metadata = [], isLoading: metadataLoading } = useModelMetadata()
	const { data: rates = [], isLoading: ratesLoading } = useBillingRates()
	const { data: patterns = [], isLoading: patternsLoading } = usePricingProfilePatterns()
	const [selectedProfile, setSelectedProfile] = useState('')
	const [search, setSearch] = useState('')
	const [syncing, setSyncing] = useState(false)
	const [autoSyncError, setAutoSyncError] = useState<string | null>(null)
	const autoSyncAttempted = useRef(false)
	const [overrideTarget, setOverrideTarget] = useState<{ profile: string; model: string } | null>(null)
	const [overrideForm, setOverrideForm] = useState({ input: '', cache: '', output: '' })
	const [savingOverride, setSavingOverride] = useState(false)
	const [patternDraft, setPatternDraft] = useState<PricingProfilePattern[]>([])
	const [patternsDirty, setPatternsDirty] = useState(false)
	const [savingPatterns, setSavingPatterns] = useState(false)

	const modelsDevRates = useMemo(() => rates.filter(rate => rate.source === 'models_dev'), [rates])
	const profiles = useMemo(() => {
		const names = new Set<string>()
		for (const rate of rates) if (rate.pricing_profile) names.add(rate.pricing_profile)
		for (const item of metadata) if (item.models_dev_provider) names.add(item.models_dev_provider)
		return [...names].sort((a, b) => a.localeCompare(b))
	}, [metadata, rates])

	useEffect(() => {
		if (!profiles.length) return
		if (!selectedProfile || !profiles.includes(selectedProfile)) {
			setSelectedProfile(profiles.includes('openai') ? 'openai' : profiles[0])
		}
	}, [profiles, selectedProfile])

	useEffect(() => {
		if (!patternsDirty) setPatternDraft(patterns.map(pattern => ({ ...pattern })))
	}, [patterns, patternsDirty])

	const runSync = async (automatic = false) => {
		setSyncing(true)
		setAutoSyncError(null)
		try {
			const result = await syncModelMetadata()
			if (!automatic) toast.success(c(`已同步 ${result.upserted} 个模型`, `Synced ${result.upserted} models`))
		} catch (error) {
			const message = error instanceof Error ? error.message : c('models.dev 同步失败', 'models.dev sync failed')
			setAutoSyncError(message)
			if (!automatic) toast.error(message)
		} finally {
			setSyncing(false)
		}
	}

	useEffect(() => {
		if (metadataLoading || ratesLoading || autoSyncAttempted.current) return
		if (metadata.some(item => item.source === 'models_dev') || modelsDevRates.length > 0) return
		autoSyncAttempted.current = true
		setSyncing(true)
		setAutoSyncError(null)
		void syncModelMetadata()
			.catch(error => setAutoSyncError(error instanceof Error ? error.message : 'models.dev sync failed'))
			.finally(() => setSyncing(false))
	}, [metadata, metadataLoading, modelsDevRates.length, ratesLoading])

	const selectedModelRates = useMemo(() => {
		const grouped = new Map<string, BillingRateRecord[]>()
		for (const rate of rates) {
			if (rate.pricing_profile !== selectedProfile || !rate.model_pattern) continue
			const list = grouped.get(rate.model_pattern) ?? []
			list.push(rate)
			grouped.set(rate.model_pattern, list)
		}
		for (const item of metadata) {
			if (item.models_dev_provider === selectedProfile && !grouped.has(item.model_id)) {
				grouped.set(item.model_id, [])
			}
		}
		return [...grouped.entries()]
			.filter(([model]) => model.toLowerCase().includes(search.trim().toLowerCase()))
			.sort(([a], [b]) => a.localeCompare(b))
	}, [metadata, rates, search, selectedProfile])

	const profileCounts = useMemo(() => {
		const counts = new Map<string, Set<string>>()
		for (const rate of rates) {
			if (!rate.pricing_profile || !rate.model_pattern) continue
			const models = counts.get(rate.pricing_profile) ?? new Set<string>()
			models.add(rate.model_pattern)
			counts.set(rate.pricing_profile, models)
		}
		return counts
	}, [rates])

	const latestSync = useMemo(() => {
		const timestamps = [...metadata, ...modelsDevRates].map(item => new Date(item.updated_at).getTime()).filter(Number.isFinite)
		return timestamps.length ? new Date(Math.max(...timestamps)) : null
	}, [metadata, modelsDevRates])

	const openOverride = (profile: string, model: string, modelRates: BillingRateRecord[]) => {
		setOverrideTarget({ profile, model })
		setOverrideForm({
			input: nanoToInput(effectiveRate(modelRates, 'input_uncached')?.unit_price_nano_usd),
			cache: nanoToInput(effectiveRate(modelRates, 'cache_read')?.unit_price_nano_usd),
			output: nanoToInput(effectiveRate(modelRates, 'output')?.unit_price_nano_usd)
		})
	}

	const saveOverride = async () => {
		if (!overrideTarget) return
		setSavingOverride(true)
		try {
			const values: Record<UsageClass, string> = {
				input_uncached: overrideForm.input,
				cache_read: overrideForm.cache,
				output: overrideForm.output
			}
			for (const { id: usageClass } of visibleUsageClasses) {
				const value = values[usageClass]
				if (!value.trim() && usageClass === 'cache_read') {
					const existingManualCacheRate = rates.find(rate =>
						rate.source === 'manual' &&
						rate.pricing_profile === overrideTarget.profile &&
						rate.model_pattern === overrideTarget.model &&
						rate.usage_class === 'cache_read'
					)
					if (existingManualCacheRate) {
						await deleteBillingRateOptimistic(existingManualCacheRate.id, rates)
					}
					continue
				}
				if (!value.trim()) throw new Error(c('输入和输出价格不能为空', 'Input and output prices are required'))
				const id = `manual:${safeIdPart(overrideTarget.profile)}:${safeIdPart(overrideTarget.model)}:${usageClass}`
				await upsertBillingRateOptimistic(id, {
					source: 'manual',
					pricing_profile: overrideTarget.profile,
					model_pattern: overrideTarget.model,
					provider_type: null,
					rate_kind: 'token',
					usage_class: usageClass,
					unit: 'token',
					unit_price_nano_usd: perMillionToNano(value),
					priority: 1000,
					enabled: true,
					match_json: {},
					raw_json: { editor: 'billing_profiles' }
				}, rates)
			}
			toast.success(c('手动价格已保存', 'Manual pricing saved'))
			setOverrideTarget(null)
		} catch (error) {
			toast.error(error instanceof Error ? error.message : c('保存失败', 'Save failed'))
		} finally {
			setSavingOverride(false)
		}
	}

	const deleteManualOverrides = async (profile: string, model: string, modelRates: BillingRateRecord[]) => {
		const manual = modelRates.filter(rate => rate.source === 'manual')
		for (const rate of manual) await deleteBillingRateOptimistic(rate.id, rates)
		toast.success(c('已恢复 models.dev 价格', 'Restored models.dev pricing'))
	}

	const savePatterns = async () => {
		if (patternDraft.some(pattern => !pattern.pattern.trim() || !pattern.pricing_profile.trim())) {
			toast.error(c('匹配规则不能为空', 'Match rules cannot be blank'))
			return
		}
		setSavingPatterns(true)
		try {
			await updatePricingProfilePatternsOptimistic(patternDraft, patterns)
			setPatternsDirty(false)
			toast.success(c('匹配规则已保存', 'Match rules saved'))
		} catch (error) {
			toast.error(error instanceof Error ? error.message : c('保存失败', 'Save failed'))
		} finally {
			setSavingPatterns(false)
		}
	}

	if ((metadataLoading || ratesLoading || patternsLoading) && !profiles.length) {
		return <div className='grid gap-4 lg:grid-cols-[280px_1fr]'><Skeleton className='h-[520px]' /><div className='flex flex-col gap-3'><Skeleton className='h-24' /><Skeleton className='h-[420px]' /></div></div>
	}

	return <>
		<div className='overflow-hidden rounded-xl border bg-card'>
			<div className='flex flex-col gap-3 border-b bg-muted/15 p-4 sm:flex-row sm:items-center sm:justify-between'>
				<div className='flex items-start gap-3'><div className='grid size-10 place-items-center rounded-lg bg-primary/10 text-primary'><CloudDownload className='size-5' /></div><div><div className='flex flex-wrap items-center gap-2'><h3 className='font-semibold'>models.dev</h3><Badge variant='outline' className='border-status-success/40 text-status-success'><CheckCircle2 className='mr-1 size-3' />{c('自动数据源', 'Automatic source')}</Badge></div><p className='mt-1 text-xs text-muted-foreground'>{latestSync ? c(`最近同步：${latestSync.toLocaleString()}`, `Last synced: ${latestSync.toLocaleString()}`) : c('尚未同步', 'Not synced yet')}</p></div></div>
				<Button variant='outline' onClick={() => void runSync()} disabled={syncing}><RefreshCw data-icon className={syncing ? 'animate-spin' : undefined} />{syncing ? c('同步中…', 'Syncing…') : c('立即同步', 'Sync now')}</Button>
			</div>
			{autoSyncError ? <div className='p-4 pb-0'><Alert variant='destructive'><AlertTitle>{c('自动同步失败', 'Automatic sync failed')}</AlertTitle><AlertDescription className='flex flex-wrap items-center justify-between gap-2'><span>{autoSyncError}</span><Button size='sm' variant='outline' onClick={() => void runSync()}>{c('重试', 'Retry')}</Button></AlertDescription></Alert></div> : null}

			<div className='lg:grid lg:min-h-[560px] lg:grid-cols-[280px_minmax(0,1fr)]'>
				<aside className='border-b bg-muted/10 lg:border-b-0 lg:border-r'>
					<div className='border-b px-4 py-3'><h4 className='text-sm font-medium'>{c('计费 Profile', 'Billing profiles')}</h4><p className='mt-1 text-xs text-muted-foreground'>{profiles.length} {c('个数据源', 'sources')}</p></div>
					<div className='flex gap-2 overflow-x-auto p-2 lg:flex-col lg:overflow-visible'>
						{profiles.map(profile => <button type='button' key={profile} onClick={() => setSelectedProfile(profile)} className={cn('flex min-w-40 shrink-0 items-center gap-3 rounded-lg border-l-2 px-3 py-2.5 text-left transition-colors lg:min-w-0', selectedProfile === profile ? 'border-l-primary bg-primary/10' : 'border-l-transparent hover:bg-muted')}><CircleDollarSign className='size-4 shrink-0 text-muted-foreground' /><span className='min-w-0 flex-1'><span className='block truncate text-sm font-medium'>{profile}</span><span className='block text-xs text-muted-foreground'>{profileCounts.get(profile)?.size ?? 0} models</span></span><ChevronRight className='hidden size-4 text-muted-foreground lg:block' /></button>)}
					</div>
				</aside>

				<section className='min-w-0 p-4 sm:p-5'>
					<div className='flex flex-col gap-3 sm:flex-row sm:items-end sm:justify-between'><div><div className='flex flex-wrap items-center gap-2'><h3 className='text-lg font-semibold'>{selectedProfile || c('选择 Profile', 'Select a profile')}</h3><Badge variant='secondary'>{selectedModelRates.length} models</Badge></div><p className='mt-1 text-sm text-muted-foreground'>{c('默认显示 USD / 100 万 tokens；手动覆盖优先于同步价格。', 'Prices are shown as USD per 1M tokens. Manual overrides take precedence.')}</p></div><div className='relative w-full sm:w-72'><Search className='absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground' /><Input value={search} onChange={event => setSearch(event.target.value)} placeholder={c('搜索模型', 'Search models')} className='pl-9' /></div></div>

					<div className='mt-5 hidden grid-cols-[minmax(220px,1fr)_110px_110px_110px_90px] gap-2 border-b px-3 pb-2 text-xs font-medium text-muted-foreground md:grid'><span>Model</span><span>Input / 1M</span><span>Cache / 1M</span><span>Output / 1M</span><span /></div>
					<div className='mt-2 flex flex-col gap-2'>
						{selectedModelRates.map(([model, modelRates]) => {
							const manual = modelRates.some(rate => rate.source === 'manual')
							const metadataItem = metadata.find(item => item.model_id === model)
							return <div key={model} className='grid gap-3 rounded-lg border p-3 transition-colors hover:bg-muted/30 md:grid-cols-[minmax(220px,1fr)_110px_110px_110px_90px] md:items-center'>
								<div className='flex min-w-0 items-center gap-2'><ModelBadge model={model} provider={metadataItem?.models_dev_provider || selectedProfile} showDetails={false} /><div className='min-w-0'>{manual ? <Badge variant='default' className='mt-1 text-[10px]'>{c('手动覆盖', 'Manual')}</Badge> : null}</div></div>
								{visibleUsageClasses.map(item => <div key={item.id} className='flex items-center justify-between gap-3 md:block'><span className='text-xs text-muted-foreground md:hidden'>{item.label}</span><span className='font-mono text-sm'>{nanoToPerMillion(effectiveRate(modelRates, item.id)?.unit_price_nano_usd)}</span></div>)}
								<div className='flex justify-end gap-1'><Button size='sm' variant='ghost' onClick={() => openOverride(selectedProfile, model, modelRates)}>{c('编辑', 'Edit')}</Button>{manual ? <Button size='icon' variant='ghost' onClick={() => void deleteManualOverrides(selectedProfile, model, modelRates)} aria-label={c('删除手动覆盖', 'Delete manual override')}><Trash2 data-icon /></Button> : null}</div>
							</div>
						})}
						{selectedModelRates.length === 0 ? <div className='rounded-lg border border-dashed p-10 text-center text-sm text-muted-foreground'>{c('这个 Profile 没有匹配的模型。', 'No models match this profile.')}</div> : null}
					</div>

					<details className='group mt-6 rounded-xl border'>
						<summary className='flex cursor-pointer list-none items-center justify-between gap-3 p-4'><div className='flex items-center gap-3'><Settings2 className='size-4 text-muted-foreground' /><div><h4 className='text-sm font-medium'>{c('模型匹配规则', 'Model match rules')}</h4><p className='mt-0.5 text-xs text-muted-foreground'>{c('按顺序把请求模型映射到计费 Profile', 'Ordered rules map request models to billing profiles')}</p></div></div><ChevronRight className='size-4 transition-transform group-open:rotate-90' /></summary>
						<div className='flex flex-col gap-3 border-t p-4'>
							{patternDraft.map((pattern, index) => <div key={index} className='grid gap-2 sm:grid-cols-[44px_1fr_1fr_108px] sm:items-center'><span className='text-center font-mono text-xs text-muted-foreground'>{index + 1}</span><Input value={pattern.pattern} onChange={event => { setPatternDraft(previous => previous.map((item, itemIndex) => itemIndex === index ? { ...item, pattern: event.target.value } : item)); setPatternsDirty(true) }} placeholder='gpt-*' className='font-mono' /><Input value={pattern.pricing_profile} onChange={event => { setPatternDraft(previous => previous.map((item, itemIndex) => itemIndex === index ? { ...item, pricing_profile: event.target.value } : item)); setPatternsDirty(true) }} placeholder='openai' /><div className='flex items-center justify-end'><Button size='icon' variant='ghost' disabled={index === 0} onClick={() => { const next = [...patternDraft]; [next[index - 1], next[index]] = [next[index], next[index - 1]]; setPatternDraft(next); setPatternsDirty(true) }}><ArrowUp data-icon /></Button><Button size='icon' variant='ghost' disabled={index === patternDraft.length - 1} onClick={() => { const next = [...patternDraft]; [next[index + 1], next[index]] = [next[index], next[index + 1]]; setPatternDraft(next); setPatternsDirty(true) }}><ArrowDown data-icon /></Button><Button size='icon' variant='ghost' onClick={() => { setPatternDraft(previous => previous.filter((_, itemIndex) => itemIndex !== index)); setPatternsDirty(true) }}><Trash2 data-icon /></Button></div></div>)}
							<div className='flex flex-wrap items-center justify-between gap-2'><Button variant='outline' size='sm' onClick={() => { setPatternDraft(previous => [...previous, { pattern: '', pricing_profile: selectedProfile }]); setPatternsDirty(true) }}><Plus data-icon />{c('添加规则', 'Add rule')}</Button><Button size='sm' disabled={!patternsDirty || savingPatterns} onClick={() => void savePatterns()}>{savingPatterns ? c('保存中…', 'Saving…') : c('保存规则', 'Save rules')}</Button></div>
						</div>
					</details>
				</section>
			</div>
		</div>

		<Dialog open={!!overrideTarget} onOpenChange={open => { if (!open) setOverrideTarget(null) }}>
			<DialogContent className='max-w-lg'><DialogHeader><DialogTitle>{c('手动价格覆盖', 'Manual price override')}</DialogTitle><DialogDescription>{overrideTarget ? `${overrideTarget.profile} / ${overrideTarget.model}` : ''}</DialogDescription></DialogHeader><div className='flex flex-col gap-4 py-2'><p className='text-sm text-muted-foreground'>{c('输入 USD / 100 万 tokens。留空 Cache 表示不覆盖缓存价格。', 'Enter USD per 1M tokens. Leave cache blank to keep it unspecified.')}</p><div className='grid gap-4 sm:grid-cols-3'>{[{ key: 'input', label: 'Input' }, { key: 'cache', label: 'Cache read' }, { key: 'output', label: 'Output' }].map(item => <div key={item.key} className='flex flex-col gap-2'><Label>{item.label}</Label><div className='relative'><span className='absolute left-3 top-1/2 -translate-y-1/2 text-sm text-muted-foreground'>$</span><Input type='number' min='0' step='0.0001' className='pl-7' value={overrideForm[item.key as keyof typeof overrideForm]} onChange={event => setOverrideForm(previous => ({ ...previous, [item.key]: event.target.value }))} /></div></div>)}</div></div><DialogFooter><Button variant='outline' onClick={() => setOverrideTarget(null)}>{c('取消', 'Cancel')}</Button><Button disabled={savingOverride} onClick={() => void saveOverride()}>{savingOverride ? c('保存中…', 'Saving…') : c('保存覆盖', 'Save override')}</Button></DialogFooter></DialogContent>
		</Dialog>
	</>
}
