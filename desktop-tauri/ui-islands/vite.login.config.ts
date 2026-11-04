import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { fileURLToPath } from 'node:url'

/**
 * 登录页独立打包配置。
 *
 * 动机：`login.html` 只需要渲染一张登录卡片，但此前引用的是主面板的
 * `islands/ui.js`（2.3 MB，含全部页面与报表图表）。远程访问时这一份
 * 就把登录页拖到几十秒。这里只以 `src/login-entry.tsx` 为入口，产出
 * `ui/islands/login.js`，与主包共用同一套 React / 组件库版本。
 *
 * 其余配置与 `vite.config.ts` 保持一致（alias、dedupe、production 替换、
 * esbuild 压缩、目标 es2020），避免两份产物在运行时行为上出现差异。
 * 样式不在这里产出：登录页的 Tailwind 类仍由主构建的 `ui.css` 提供，
 * `login.html` 继续引用那一份。
 */
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      '@ui': fileURLToPath(new URL('../ui-kit/src', import.meta.url)),
    },
    dedupe: ['react', 'react-dom', '@base-ui/react'],
  },
  define: {
    'process.env.NODE_ENV': JSON.stringify('production'),
  },
  build: {
    outDir: '../ui/islands',
    // 与主构建共用目录，别把 ui.js / ui.css 清掉
    emptyOutDir: false,
    lib: {
      entry: 'src/login-entry.tsx',
      name: 'WbLogin',
      formats: ['iife'],
      fileName: () => 'login.js',
    },
    minify: 'esbuild',
    sourcemap: false,
    target: 'es2020',
  },
})
