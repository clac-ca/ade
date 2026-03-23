import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

const apiOrigin = process.env.ADE_API_ORIGIN ?? 'http://127.0.0.1:8001'

export default defineConfig({
  plugins: [react()],
  server: {
    host: '127.0.0.1',
    port: Number(process.env.ADE_WEB_PORT ?? 8000),
    strictPort: true,
    proxy: {
      '/api': {
        target: apiOrigin,
        changeOrigin: true
      }
    }
  }
})
