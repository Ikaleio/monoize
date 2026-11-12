import path from 'node:path'
import { defineConfig, loadEnv } from 'vite'
import react from '@vitejs/plugin-react-swc'

// Packages too large to share a chunk with the code that imports them.
const vendorChunks: Record<string, string> = {
	react: 'react',
	'react-dom': 'react',
	'react-router': 'react',
	'react-router-dom': 'react',
	scheduler: 'react',
	prettier: 'prettier',
	ai: 'ai'
}

export default defineConfig(({ mode }) => {
	const env = loadEnv(mode, process.cwd(), '')
	const apiTarget = env.VITE_API_TARGET || 'http://127.0.0.1:8080'
	return {
		plugins: [react()],
		resolve: {
			alias: {
				'@': path.resolve(__dirname, 'src')
			}
		},
		build: {
			chunkSizeWarningLimit: 1000,
			rollupOptions: {
				output: {
					manualChunks(id) {
						// Rollup otherwise puts the CommonJS interop helper into the `ai`
						// chunk, which makes the entry import the AI SDK on every page.
						if (id === '\0commonjsHelpers.js') return 'react'
						const pkg = id.match(/\/node_modules\/(@[^/]+\/[^/]+|[^/]+)\//)?.[1]
						return pkg && vendorChunks[pkg]
					}
				}
			}
		},
		server: {
			allowedHosts: ['localhost', '127.0.0.1', '::1'],
			proxy: {
				'/api': {
					target: apiTarget,
					changeOrigin: true
				}
			}
		}
	}
})
