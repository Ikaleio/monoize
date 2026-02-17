import path from 'node:path'
import { defineConfig, loadEnv } from 'vite'
import react from '@vitejs/plugin-react'

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
			chunkSizeWarningLimit: 1000
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
