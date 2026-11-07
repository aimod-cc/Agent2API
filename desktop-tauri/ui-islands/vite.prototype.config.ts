import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { fileURLToPath } from 'node:url'

/**
 * 插件页原型的独立开发配置（**不参与任何发版构建**）。
 *
 * 为什么单独一份而不是并进 vite.config.ts：主构建的入口是 `src/index.tsx`，
 * 产物要提交进仓库；原型只是给人看的一版界面稿，用 dev server 现编译现看即可，
 * 不需要产物、也不该混进 `ui/islands/`。
 *
 * 组件库以源码方式接入（同主配置）：`@ui` 指向 `../ui-kit/src`，
 * 并把 React 收敛到本工程这一份 —— 两份 React 会让两套调度器互不认识。
 */
export default defineConfig({
  root: 'prototype',
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      '@ui': fileURLToPath(new URL('../ui-kit/src', import.meta.url)),
    },
    dedupe: ['react', 'react-dom', '@base-ui/react'],
  },
  server: {
    host: '127.0.0.1',
    port: 8790,
    strictPort: true,
    /**
     * 放行 `ui-islands` 之外的源码与资源：`@ui` 指向 `../ui-kit/src`，原型里还会
     * 直接引 `ui/assets/providers/*.png`。两个目录都在 `desktop-tauri` 下，
     * 而 Vite 默认只放行最近的 package 根（这里是 `ui-islands`）—— 不加这一条，
     * 这些请求会 403，原型白屏。
     */
    fs: { allow: [fileURLToPath(new URL('..', import.meta.url))] },
  },
  define: {
    'process.env.NODE_ENV': JSON.stringify('development'),
  },
})
