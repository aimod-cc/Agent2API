import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { fileURLToPath } from 'node:url'

export default defineConfig({
  root: fileURLToPath(new URL('./prototype', import.meta.url)),
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: { '@ui': fileURLToPath(new URL('../ui-kit/src', import.meta.url)) },
    dedupe: ['react', 'react-dom', '@base-ui/react'],
  },
  server: {
    host: '127.0.0.1',
    port: 8794,
    strictPort: true,
    fs: { allow: [fileURLToPath(new URL('..', import.meta.url))] },
  },
  define: { 'process.env.NODE_ENV': JSON.stringify('development') },
})
