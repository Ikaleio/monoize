import { createContext, useContext } from 'react'
import type { ProviderType } from '@/lib/api'

// CP-FE-9: client-side concurrency cap for "concurrent" runs, shown in the UI.
export const CHANNEL_TEST_CONCURRENCY = 5

export type ModelTestResult = {
	status: 'testing' | 'passed' | 'failed'
	latency_ms?: number
	http_status?: number | null
	error_code?: string | null
	error_type?: string | null
	error?: string
}

export type RunMode = 'sequential' | 'concurrent'

export type ChannelTestRun = {
	results: Partial<Record<string, ModelTestResult>>
	running: RunMode | 'single' | null
	lastMode: RunMode
}

export type ChannelTestTarget = {
	providerId: string
	providerName: string
	channelId: string
	channelName: string
	providerType: ProviderType
	models: string[]
}

export type ChannelTestContextValue = {
	runs: Record<string, ChannelTestRun>
	openTest: (target: ChannelTestTarget) => void
	start: (target: ChannelTestTarget, models: string[], mode: RunMode | 'single', stream: boolean) => void
	stop: (channelId: string) => void
}

export const ChannelTestContext = createContext<ChannelTestContextValue | null>(null)

export function useChannelTests() {
	const value = useContext(ChannelTestContext)
	if (!value) throw new Error('useChannelTests must be used inside ChannelTestProvider')
	return value
}
