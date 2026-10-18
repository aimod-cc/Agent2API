import { defineConfig, type Plugin } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { fileURLToPath } from 'node:url'
import { createHash } from 'node:crypto'
import fs from 'node:fs'
import path from 'node:path'

/**
 * 把 sw.js 从产物目录搬到 `ui/` 根，并把缓存版本号换成**产物内容哈希**。
 *
 * ── 为什么搬到根 ────────────────────────────────────────────
 * SW 的默认 scope 上限是它自己所在的目录：`ui/m/sw.js` 最多只能控制
 * `/m/*`，而入口是 `/m.html` —— 它落在 scope 之外。注册成 `scope: '/'`
 * 会被浏览器拒绝（`The path of the provided scope is not under the max
 * scope`），注册成 `/m/` 又管不到入口页。放根目录没有这个矛盾。
 *
 * ── 为什么是内容哈希，不是时间戳 ────────────────────────────
 * SW 的更新判据是**文件内容**：产物变了但 sw.js 一字未改时，浏览器不会
 * 重新安装 SW，旧缓存里的 mobile.js 会一直生效 —— 用户看到上一个版本的
 * 界面。所以版本号必须跟着产物变。
 *
 * 但不能用 `Date.now()`：CI 有一道「入库产物必须能由源码逐字节复现」的
 * 闸（.github/workflows/frontend-artifacts.yml），时间戳会让每次构建都
 * 产出不同的 sw.js，那道闸必然报错。改用 mobile.js + mobile.css 的
 * 内容哈希：产物变了版本就变，产物没变就完全一致，两边都满足。
 *
 * ── 只在 web 目标下跑 ───────────────────────────────────────
 * APK 目标（`--mode apk`）没有 Service Worker（界面在包里，不经过 HTTP），
 * 这个插件在那儿无事可做。
 */
function swBuildStamp(): Plugin {
  return {
    name: 'sw-build-stamp',
    apply: 'build',
    closeBundle() {
      const built = path.resolve(__dirname, '../ui/m/sw.js')
      const target = path.resolve(__dirname, '../ui/sw.js')
      if (!fs.existsSync(built)) return

      const hash = createHash('sha256')
      // sw.js 自身也算：只改缓存策略、不改 JS/CSS 时，版本号同样要变，
      // 否则浏览器不重装 SW，新策略永远不生效。
      hash.update(fs.readFileSync(path.resolve(__dirname, 'public', 'sw.js')))
      for (const name of ['mobile.js', 'mobile.css']) {
        const file = path.resolve(__dirname, '../ui/m', name)
        if (fs.existsSync(file)) hash.update(fs.readFileSync(file))
      }
      // 12 位十六进制足够区分版本，也不会把缓存名拉得太长
      const stamp = hash.digest('hex').slice(0, 12)

      const text = fs.readFileSync(built, 'utf8').replace(/__BUILD__/g, stamp)
      fs.writeFileSync(target, text)
      // 产物目录里不留副本：它在那儿会被当成「可注册的 SW」，而 scope 不对
      fs.rmSync(built)
    },
  }
}

/**
 * APK 目标：把入口 HTML 与资源铺平到一个目录，交给 Capacitor 当 `webDir`。
 *
 * ── 为什么单独一份，不复用 ui/m/ ─────────────────────────────
 * PWA 产物是「网关托管的一个子目录」，路径都相对 `/m/`；APK 的 WebView
 * 从本地文件加载，`/m/mobile.js` 这种绝对路径会解析到文件系统根。所以
 * APK 目标要用**相对路径**、入口是根目录的 index.html，不能和 PWA 共用。
 *
 * ── 为什么还要把 i18n 与图标搬进来 ───────────────────────────
 * 浏览器形态下词典与提供商图标由网关托管（`ui/i18n/*`、`ui/assets/*`）；
 * APK 里没有网关，界面要能独立起来，这两样必须打进包。
 */
function apkBundle(): Plugin {
  return {
    name: 'apk-bundle',
    apply: 'build',
    closeBundle() {
      const root = path.resolve(__dirname, 'android-www')
      const source = path.resolve(__dirname, '../ui')

      // 入口页：库模式不生成 HTML，这里把 apk.html 改名成 index.html。
      // Capacitor 的 webDir 只认 index.html。
      fs.copyFileSync(path.resolve(__dirname, 'apk.html'), path.join(root, 'index.html'))

      // 词典：只带运行时用得到的那几个域文件，连同 manifest.js
      const i18nSrc = path.join(source, 'i18n')
      const i18nOut = path.join(root, 'i18n')
      if (fs.existsSync(i18nSrc)) {
        fs.cpSync(i18nSrc, i18nOut, { recursive: true })
      }
      // 提供商图标
      const providersSrc = path.join(source, 'assets', 'providers')
      const providersOut = path.join(root, 'assets', 'providers')
      if (fs.existsSync(providersSrc)) {
        fs.mkdirSync(providersOut, { recursive: true })
        fs.cpSync(providersSrc, providersOut, { recursive: true })
      }
      // 应用图标（登录页 / 空态可能用到）
      for (const name of ['app-icon-192.png', 'app-icon-512.png']) {
        const file = path.join(source, 'assets', name)
        if (fs.existsSync(file)) {
          fs.mkdirSync(path.join(root, 'assets'), { recursive: true })
          fs.copyFileSync(file, path.join(root, 'assets', name))
        }
      }
    },
  }
}

/**
 * 手机端构建配置。两个目标：
 *
 *   `npm run build`         → web（PWA），产物落 `ui/m/`，入口 `ui/m.html`
 *   `npm run build:apk`     → APK，产物落 `android-www/`，入口 index.html
 *
 * ── 为什么 web 产物直接落 ui/ ───────────────────────────────
 * `ui/` 是 Tauri 的 frontendDist、Docker 的 COPY 目标、服务端 static_files
 * 的根 —— 手机端与桌面端共用同一份静态目录，发版流程一行都不用改。
 *
 * 为什么入口是 `ui/m.html` 而不是 `ui/m/index.html`：服务端 static_files
 * 的路径解析是「无扩展名补 .html」（`/login` → `login.html`），不支持子目录
 * 的 index 兜底 —— 放 `ui/m/index.html` 时 `/m` 找不到文件。`ui/m.html` 让
 * `/m` 直接命中，Rust 侧零改动。
 *
 * ── 为什么是 iife ───────────────────────────────────────────
 * 与 ui-islands 同因：页面里还有 `i18n.js` 等经典脚本，module 脚本是 defer
 * 执行的，混进来会多一层时序问题。产物是单个经典脚本，按加载顺序执行。
 * APK 里同样是经典脚本 —— Capacitor 的 WebView 直接从文件加载，不涉及
 * 模块解析。
 *
 * ── 组件库 ──────────────────────────────────────────────────
 * `../ui-kit` 以源码接入（不走 npm 包），dedupe 把 React 收敛到本工程这一份。
 */
export default defineConfig(({ mode }) => {
  const apk = mode === 'apk'
  return {
    plugins: apk
      ? [react(), tailwindcss(), apkBundle()]
      : [react(), tailwindcss(), swBuildStamp()],
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
      outDir: apk ? 'android-www' : '../ui/m',
      emptyOutDir: true,
      // web 目标要复制 public/（manifest 与 sw.js）；APK 目标不要
      // —— Service Worker 在本地文件里没有意义
      copyPublicDir: !apk,
      lib: {
        entry: 'src/main.tsx',
        name: 'A2aMobile',
        formats: ['iife'],
        fileName: () => 'mobile.js',
        cssFileName: 'mobile',
      },
      minify: 'esbuild',
      sourcemap: false,
      target: 'es2020',
    },
  }
})
