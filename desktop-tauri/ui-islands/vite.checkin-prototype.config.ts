import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { fileURLToPath } from 'node:url'

/** 签到中心「左侧分类」原型：独立入口，不进生产产物。 */
export default defineConfig({
  root: fileURLToPath(new URL('./prototype-checkin', import.meta.url)),
  base: '/prototype-checkin/',
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: { '@ui': fileURLToPath(new URL('../ui-kit/src', import.meta.url)) },
    dedupe: ['react', 'react-dom', '@base-ui/react'],
  },
  define: { 'process.env.NODE_ENV': JSON.stringify('production') },
  build: {
    outDir: fileURLToPath(new URL('../ui/prototype-checkin', import.meta.url)),
    emptyOutDir: true,
    target: 'es2020',
  },
})
