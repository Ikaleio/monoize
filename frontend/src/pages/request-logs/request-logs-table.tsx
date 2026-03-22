import type { ComponentProps } from 'react'
import { TableVirtuoso } from 'react-virtuoso'
import { Skeleton } from '@/components/ui/skeleton'
import type { RequestLog } from '@/lib/api'
import { LogRowCells } from './log-row-cells'

interface RequestLogsTableProps {
	isAdmin: boolean
	isInitialLoading: boolean
	logs: RequestLog[]
	onLoadMore: () => void
	onTooltipOpenChange: (tooltipId: string, open: boolean) => void
	showIp: boolean
	t: (key: string) => string
}

const tableComponents = {
	Table: (props: ComponentProps<'table'>) => (
		<table
			{...props}
			className='w-full table-auto text-xs'
			style={{ minWidth: '60rem' }}
		/>
	),
	TableHead: (props: ComponentProps<'thead'>) => (
		<thead {...props} className='[&_tr]:border-b' />
	),
	TableBody: (props: ComponentProps<'tbody'>) => (
		<tbody {...props} className='[&_tr:last-child]:border-0' />
	),
	TableRow: (props: ComponentProps<'tr'>) => (
		<tr
			{...props}
			className='border-b transition-colors hover:bg-muted/30 align-middle'
		/>
	)
}

function RequestLogsTableHeader({ isAdmin, t }: Pick<RequestLogsTableProps, 'isAdmin' | 't'>) {
	return (
		<tr className='border-b bg-muted/30'>
			<th className='w-[10rem] text-left font-medium text-muted-foreground pl-2 pr-2 py-1.5 whitespace-nowrap'>
				{t('requestLogs.time')}
			</th>
			<th className='w-[5rem] text-left font-medium text-muted-foreground px-2 py-1.5 whitespace-nowrap'>
				{t('requestLogs.requestId')}
			</th>
			<th className='min-w-[13.5rem] text-left font-medium text-muted-foreground px-2 py-1.5 whitespace-nowrap'>
				{t('requestLogs.model')}
			</th>
			<th className='w-[5rem] text-left font-medium text-muted-foreground px-2 py-1.5 whitespace-nowrap'>
				{t('requestLogs.tokenName')}
			</th>
			{isAdmin && (
				<th className='w-[4rem] text-left font-medium text-muted-foreground px-2 py-1.5 whitespace-nowrap'>
					{t('requestLogs.username')}
				</th>
			)}
			{isAdmin && (
				<th className='w-[5.5rem] text-left font-medium text-muted-foreground px-2 py-1.5 whitespace-nowrap'>
					{t('requestLogs.channel')}
				</th>
			)}
			<th className='w-[8rem] text-left font-medium text-muted-foreground px-2 py-1.5 whitespace-nowrap'>
				{t('requestLogs.duration')} / {t('requestLogs.ttfb')}
			</th>
			<th className='w-[3.25rem] text-right font-medium text-muted-foreground px-2 py-1.5 whitespace-nowrap'>
				{t('requestLogs.input')}
			</th>
			<th className='w-[3.25rem] text-right font-medium text-muted-foreground px-2 py-1.5 whitespace-nowrap'>
				{t('requestLogs.output')}
			</th>
			<th className='min-w-[8.5rem] text-right font-medium text-muted-foreground px-2 py-1.5 whitespace-nowrap'>
				{t('requestLogs.cost')}
			</th>
			<th className='text-left font-medium text-muted-foreground pl-2 pr-2 py-1.5 whitespace-nowrap'>
				{t('requestLogs.requestIp')}
			</th>
		</tr>
	)
}

export function RequestLogsTable({
	isAdmin,
	isInitialLoading,
	logs,
	onLoadMore,
	onTooltipOpenChange,
	showIp,
	t
}: RequestLogsTableProps) {
	if (isInitialLoading) {
		return (
			<div className='p-4 space-y-1.5'>
				{Array.from({ length: 24 }).map((_, i) => (
					<Skeleton key={i} className='h-9 w-full' />
				))}
			</div>
		)
	}

	if (logs.length === 0) {
		return (
			<div className='h-full flex items-center justify-center text-muted-foreground text-sm px-4'>
				{t('requestLogs.noLogs')}
			</div>
		)
	}

	return (
		<TableVirtuoso
			style={{ height: '100%', overflowX: 'auto' }}
			data={logs}
			computeItemKey={(_index, log) => log.id}
			overscan={480}
			endReached={onLoadMore}
			components={tableComponents}
			fixedHeaderContent={() => <RequestLogsTableHeader isAdmin={isAdmin} t={t} />}
			itemContent={(_index, log) => (
				<LogRowCells
					log={log}
					isAdmin={isAdmin}
					showIp={showIp}
					t={t}
					onTooltipOpenChange={onTooltipOpenChange}
				/>
			)}
		/>
	)
}
