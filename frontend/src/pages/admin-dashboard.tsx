import { useTranslation } from 'react-i18next'
import {
	Activity,
	AlertTriangle,
	CopyPlus,
	HardDrive,
	HeartPulse,
	Network,
	RefreshCw,
	Server,
	Users
} from 'lucide-react'
import { useAuth } from '@/hooks/use-auth'
import { useAdminOverview } from '@/lib/swr'
import type { AdminOverviewChannelHealth } from '@/lib/api'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { EmptyState } from '@/components/ui/empty-state'
import { PageHeader } from '@/components/ui/page-header'
import { PageWrapper, motion, transitions } from '@/components/ui/motion'
import { CardsPageSkeleton } from '@/components/ui/page-skeleton'
import { formatNanoUsd } from '@/lib/exact-decimal'
import { cn } from '@/lib/utils'

function formatNumber(value: number): string {
	return value.toLocaleString('en-US')
}

function humanizeUptime(seconds: number): string {
	if (!Number.isFinite(seconds) || seconds <= 0) return '-'
	const days = Math.floor(seconds / 86400)
	const hours = Math.floor((seconds % 86400) / 3600)
	const minutes = Math.floor((seconds % 3600) / 60)
	if (days > 0) return `${days}d ${hours}h ${minutes}m`
	if (hours > 0) return `${hours}h ${minutes}m`
	return `${minutes}m`
}

function formatBytes(bytes: number): string {
	if (!Number.isFinite(bytes) || bytes <= 0) return '0 B'
	const units = ['B', 'KB', 'MB', 'GB']
	let value = bytes
	let unit = 0
	while (value >= 1024 && unit < units.length - 1) {
		value /= 1024
		unit += 1
	}
	return `${value.toFixed(1)} ${units[unit]}`
}

function formatTimestamp(unixMs: number | null | undefined): string {
	if (unixMs == null) return '-'
	const date = new Date(unixMs)
	if (Number.isNaN(date.getTime())) return '-'
	return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(
		date.getDate()
	).padStart(2, '0')} ${String(date.getHours()).padStart(2, '0')}:${String(
		date.getMinutes()
	).padStart(2, '0')}:${String(date.getSeconds()).padStart(2, '0')}`
}

function MetricRow({ label, value }: { label: string; value: React.ReactNode }) {
	return (
		<div className='flex items-center justify-between gap-3'>
			<span className='text-xs text-muted-foreground'>{label}</span>
			<span className='font-mono text-xs'>{value}</span>
		</div>
	)
}

function healthBadge(channel: AdminOverviewChannelHealth, t: (key: string, fallback?: string) => string) {
	if (channel.cooldown_active) {
		return (
			<Badge variant='secondary' className='border-warning-border bg-warning-soft text-warning-foreground'>
				{t('adminDashboard.coolingDown')}
			</Badge>
		)
	}
	if (!channel.enabled) {
		return <Badge variant='secondary'>{t('adminDashboard.disabled')}</Badge>
	}
	return channel.healthy ? (
		<Badge variant='secondary' className='border-success bg-success/10 text-success'>
			{t('adminDashboard.healthy')}
		</Badge>
	) : (
		<Badge variant='secondary' className='border-destructive bg-destructive/10 text-destructive'>
			{t('adminDashboard.unhealthy')}
		</Badge>
	)
}

export function AdminDashboardPage() {
	const { t } = useTranslation()
	const { user } = useAuth()
	const isAdmin = user?.role === 'super_admin' || user?.role === 'admin'
	const { data, error, isLoading, mutate } = useAdminOverview({ isPaused: () => !isAdmin })

	const tt = (key: string, fallback?: string): string => {
		const translated = t(key, { defaultValue: fallback ?? key } as never)
		return typeof translated === 'string' ? translated : (fallback ?? key)
	}

	if (!isAdmin) {
		return (
			<PageWrapper className='h-full min-h-0 overflow-hidden'>
				<EmptyState
					title={tt('adminDashboard.unauthorized', 'Administrator access required')}
					description={tt('adminDashboard.unauthorizedDescription', 'This page is only available to administrators.')}
					className='h-full py-0'
				/>
			</PageWrapper>
		)
	}

	if (isLoading && !data) {
		return (
			<PageWrapper className='h-full min-h-0 overflow-hidden space-y-4'>
				<CardsPageSkeleton />
			</PageWrapper>
		)
	}

	if (error && !data) {
		return (
			<PageWrapper className='h-full min-h-0 overflow-hidden'>
				<EmptyState
					variant='card'
					icon={<AlertTriangle className='h-8 w-8 text-destructive' />}
					title={tt('adminDashboard.loadFailed', 'Failed to load system overview')}
					description={
						<span className='font-mono text-xs break-all'>
							{error instanceof Error ? error.message : tt('common.error', 'Error')}
						</span>
					}
					className='h-full py-0'
				/>
				<div className='mt-3 flex justify-center'>
					<Button variant='outline' onClick={() => void mutate()}>
						<RefreshCw data-icon />
						{tt('adminDashboard.retry', 'Retry')}
					</Button>
				</div>
			</PageWrapper>
		)
	}

	if (!data) return null

	return (
		<PageWrapper className='flex h-full min-h-0 flex-col gap-4 overflow-hidden'>
			<motion.header
				initial={{ opacity: 0, y: -12 }}
				animate={{ opacity: 1, y: 0 }}
				transition={transitions.normal}
				className='shrink-0'
			>
				<PageHeader
					title={tt('adminDashboard.title', 'System Dashboard')}
					description={tt('adminDashboard.subtitle', 'System status, user usage ranking, model/channel health and replica status')}
				/>
			</motion.header>

			<div className='grid min-h-0 flex-1 grid-cols-1 gap-4 overflow-y-auto lg:grid-cols-2'>
				<Card className='h-fit'>
					<CardHeader className='p-4 pb-2'>
						<CardTitle className='flex items-center gap-2 text-base'>
							<Server className='h-4 w-4 text-primary' />
							{tt('adminDashboard.systemStatus', 'System Status')}
						</CardTitle>
					</CardHeader>
					<CardContent className='space-y-1.5 p-4 pt-0'>
						<MetricRow label={tt('adminDashboard.nodeRole', 'Node role')} value={<Badge variant='secondary' className='font-mono'>{data.node.role}</Badge>} />
						<MetricRow label={tt('adminDashboard.version', 'Version')} value={data.node.version} />
						<MetricRow label={tt('adminDashboard.uptime', 'Uptime')} value={humanizeUptime(data.node.uptime_seconds)} />
						<MetricRow label={tt('adminDashboard.startedAt', 'Started at')} value={formatTimestamp(Date.parse(data.node.started_at))} />
						<MetricRow label={tt('adminDashboard.listen', 'Listen')} value={data.node.listen} />
						<MetricRow label={tt('adminDashboard.metricsPath', 'Metrics path')} value={data.node.metrics_path} />
						<MetricRow label={tt('adminDashboard.database', 'Database')} value={`${data.node.database_backend} · ${data.node.database_dsn_redacted}`} />
						<MetricRow label={tt('adminDashboard.upstreamProxy', 'Egress proxy')} value={data.node.upstream_proxy_url || '-'} />
						<MetricRow label={tt('adminDashboard.pendingRequestLogs', 'Pending request logs')} value={formatNumber(data.system.pending_request_logs)} />
						<MetricRow label={tt('adminDashboard.sseConnections', 'SSE connections')} value={formatNumber(data.system.sse_connections)} />
						<MetricRow label={tt('adminDashboard.routingRevision', 'Routing revision')} value={data.system.routing_config_revision} />
					</CardContent>
				</Card>

				<Card className='h-fit'>
					<CardHeader className='p-4 pb-2'>
						<CardTitle className='flex items-center gap-2 text-base'>
							<HardDrive className='h-4 w-4 text-primary' />
							{tt('adminDashboard.replicaStatus', 'Replica Status')}
						</CardTitle>
					</CardHeader>
					<CardContent className='space-y-1.5 p-4 pt-0'>
						{data.node.role === 'replica' ? (
							<>
								<MetricRow label={tt('adminDashboard.spoolPendingCount', 'Spool pending files')} value={formatNumber(data.replica.spool_pending_count)} />
								<MetricRow label={tt('adminDashboard.spoolPendingBytes', 'Spool pending bytes')} value={formatBytes(data.replica.spool_pending_bytes)} />
							</>
						) : (
							<>
								<MetricRow
									label={tt('adminDashboard.ingestEnabled', 'Replica ingest')}
									value={
										data.replica.ingest_enabled ? (
											<Badge variant='secondary' className='border-success bg-success/10 text-success'>
												{tt('adminDashboard.enabled', 'Enabled')}
											</Badge>
										) : (
											<Badge variant='secondary'>{tt('adminDashboard.disabled', 'Disabled')}</Badge>
										)
									}
								/>
								{!data.replica.ingest_enabled && (
									<p className='pt-1 text-xs text-muted-foreground'>
										{tt('adminDashboard.noReplicaToken', 'No replica token configured; there is nothing to monitor.')}
									</p>
								)}
								{data.replica.ingest_enabled && (data.replica.replicas ?? []).length === 0 && (
									<p className='pt-1 text-xs text-muted-foreground'>
										{tt('adminDashboard.noReplicasYet', 'No replica has heartbeated yet.')}
									</p>
								)}
								{(data.replica.replicas ?? []).map((replica) => (
									<div key={replica.id} className='rounded-md border px-3 py-2'>
										<div className='flex items-center justify-between gap-2'>
											<span className='truncate font-mono text-xs'>
												{replica.hostname || replica.id} · {replica.listen}
											</span>
											<Badge
												variant='secondary'
												className={cn(
													replica.stale
														? 'border-warning-border bg-warning-soft text-warning-foreground'
														: 'border-success bg-success/10 text-success'
												)}
											>
												{replica.stale
													? tt('adminDashboard.stale', 'Stale')
													: tt('adminDashboard.live', 'Live')}
											</Badge>
										</div>
										<div className='mt-1 space-y-0.5'>
											<MetricRow label={tt('adminDashboard.version', 'Version')} value={replica.version} />
											<MetricRow label={tt('adminDashboard.uptime', 'Uptime')} value={humanizeUptime(replica.uptime_seconds)} />
											<MetricRow label={tt('adminDashboard.lastSeen', 'Last seen')} value={formatTimestamp(Date.parse(replica.last_seen_at))} />
											<MetricRow label={tt('adminDashboard.spoolPendingCount', 'Spool pending files')} value={formatNumber(replica.spool_pending_count)} />
											<MetricRow label={tt('adminDashboard.spoolPendingBytes', 'Spool pending bytes')} value={formatBytes(replica.spool_pending_bytes)} />
										</div>
									</div>
								))}
							</>
						)}
					</CardContent>
				</Card>

				<Card className='h-fit'>
					<CardHeader className='p-4 pb-2'>
						<CardTitle className='flex items-center gap-2 text-base'>
							<Users className='h-4 w-4 text-primary' />
							{tt('adminDashboard.usageRanking', 'User Usage Ranking (24h)')}
						</CardTitle>
					</CardHeader>
					<CardContent className='p-4 pt-0'>
						{data.users_ranking.length === 0 ? (
							<EmptyState
								title={tt('adminDashboard.noUsage', 'No usage in the last 24 hours')}
								className='py-4'
							/>
						) : (
							<div className='overflow-x-auto'>
								<table className='w-full text-xs'>
									<thead>
										<tr className='border-b text-left text-muted-foreground'>
											<th className='py-1 pr-2 font-medium'>#</th>
											<th className='py-1 pr-2 font-medium'>{tt('adminDashboard.username', 'User')}</th>
											<th className='py-1 pr-2 text-right font-medium'>{tt('adminDashboard.calls', 'Calls')}</th>
											<th className='py-1 text-right font-medium'>{tt('adminDashboard.cost', 'Cost')}</th>
										</tr>
									</thead>
									<tbody>
										{data.users_ranking.map((row, index) => (
											<tr key={row.user_id} className='border-b border-muted/40'>
												<td className='py-1 pr-2 font-mono text-muted-foreground'>{index + 1}</td>
												<td className='py-1 pr-2'>
													<span className={cn(!row.username && 'font-mono')}>
														{row.username || row.user_id}
													</span>
												</td>
												<td className='py-1 pr-2 text-right font-mono'>{formatNumber(row.call_count)}</td>
												<td className='py-1 text-right font-mono'>{formatNanoUsd(row.cost_nano_usd, 6)}</td>
											</tr>
										))}
									</tbody>
								</table>
							</div>
						)}
					</CardContent>
				</Card>

				<Card className='h-fit'>
					<CardHeader className='p-4 pb-2'>
						<CardTitle className='flex items-center justify-between gap-2 text-base'>
							<span className='flex items-center gap-2'>
								<HeartPulse className='h-4 w-4 text-primary' />
								{tt('adminDashboard.channelHealth', 'Model / Channel Health')}
							</span>
							<span className='font-mono text-xs font-normal text-muted-foreground'>
								{tt('adminDashboard.todaySpend', "Today's spend")}: {formatNanoUsd(data.today?.cost_nano_usd ?? '0', 2)}
								<span className='mx-1'>·</span>
								{tt('adminDashboard.todayCalls', "Today's calls")}: {formatNumber(data.today?.calls ?? 0)}
							</span>
						</CardTitle>
					</CardHeader>
					<CardContent className='p-4 pt-0'>
						{data.channel_health.length === 0 ? (
							<EmptyState title={tt('adminDashboard.noChannels', 'No channels configured')} className='py-4' />
						) : (
							<div className='overflow-x-auto'>
								<table className='w-full text-xs'>
									<thead>
										<tr className='border-b text-left text-muted-foreground'>
											<th className='py-1 pr-2 font-medium'>{tt('adminDashboard.channel', 'Channel')}</th>
											<th className='py-1 pr-2 font-medium'>{tt('adminDashboard.weight', 'Weight')}</th>
											<th className='py-1 pr-2 font-medium'>{tt('adminDashboard.affinity', 'Affinity')}</th>
											<th className='py-1 pr-2 font-medium'>{tt('adminDashboard.status', 'Status')}</th>
											<th className='py-1 pr-2 text-right font-medium'>{tt('adminDashboard.todaySpend', "Today")}</th>
											<th className='py-1 text-right font-medium'>{tt('adminDashboard.lastProbe', 'Last probe')}</th>
										</tr>
									</thead>
									<tbody>
										{data.channel_health.map((channel) => (
											<tr key={channel.channel_id} className='border-b border-muted/40'>
												<td className='max-w-[10rem] truncate py-1 pr-2'>
													<span className='font-medium'>{channel.channel_name}</span>
													<span className='text-muted-foreground'> · {channel.provider_name}</span>
												</td>
												<td className='py-1 pr-2 font-mono'>{channel.weight}</td>
												<td className='py-1 pr-2'>
													{channel.session_affinity_auto ? (
														<Badge variant='secondary' className='border-info-border bg-info-soft text-info-foreground'>
															auto
														</Badge>
													) : (
														<span className='text-muted-foreground'>-</span>
													)}
												</td>
												<td className='py-1 pr-2'>
													<div className='flex flex-col gap-0.5'>
														{healthBadge(channel, tt)}
														{(channel.unhealthy_models ?? []).length > 0 && (
															<span className='truncate font-mono text-xs text-destructive'>
																{(channel.unhealthy_models ?? []).join(', ')}
															</span>
														)}
													</div>
												</td>
												<td className='py-1 pr-2 text-right font-mono'>
													<div>{formatNanoUsd(channel.today_cost_nano_usd ?? '0', 2)}</div>
													<div className='text-xs text-muted-foreground'>
														{formatNumber(channel.today_calls ?? 0)} {tt('adminDashboard.calls', 'Calls')}
													</div>
												</td>
												<td className='py-1 text-right font-mono text-muted-foreground'>
													{formatTimestamp(channel.last_probe_at)}
												</td>
											</tr>
										))}
									</tbody>
								</table>
							</div>
						)}
					</CardContent>
				</Card>
			</div>

			<div className='flex shrink-0 items-center gap-2 text-xs text-muted-foreground'>
				<Activity className='h-3.5 w-3.5' />
				<span>
					{tt('adminDashboard.healthEntries', 'Tracked health entries')}: {formatNumber(data.system.channel_health_entries)}
				</span>
				<span>·</span>
				<span>
					{tt('adminDashboard.affinityEntries', 'Affinity bindings')}: {formatNumber(data.system.channel_affinity_entries)}
				</span>
				<span>·</span>
				<Button
					variant='ghost'
					size='sm'
					className='h-6 gap-1 px-2 text-xs'
					onClick={() => void mutate()}
				>
					<CopyPlus className='h-3.5 w-3.5' />
					{tt('adminDashboard.refresh', 'Refresh')}
				</Button>
				<span className={cn('ml-auto hidden items-center gap-1 sm:flex')}>
					<Network className='h-3.5 w-3.5' />
					{tt('adminDashboard.autoRefresh', 'Auto refresh 10s')}
				</span>
			</div>
		</PageWrapper>
	)
}