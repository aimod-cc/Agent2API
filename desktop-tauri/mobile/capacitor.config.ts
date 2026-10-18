import type { CapacitorConfig } from '@capacitor/cli'

/**
 * Capacitor 配置（仅用于打 APK）。
 *
 * ── 为什么 androidScheme 是 https，而 cleartext 打开 ──────────
 * 这两个不矛盾，各管一件事：
 *   · `androidScheme: 'https'` 决定**应用自己页面**的 origin（`https://localhost`）。
 *     它必须是安全上下文，否则 `crypto.subtle` 不可用 —— 登录页的人机验证
 *     要用它算 SHA-256。改成 http 会让验证退回纯 JS 实现，慢且没必要。
 *   · `cleartext: true` + manifest 里的 `usesCleartextTraffic` 决定**能不能
 *     访问 http:// 的网关**。局域网网关都是 http（没有证书），不开这个
 *     所有请求会被 Android 9+ 直接拦掉。
 *
 * 页面是 https、请求是 http —— 这是 mixed content，WebView 默认禁止。
 * 所以还要 `allowMixedContent: true`。它被官方标注「不用于生产」，但这个
 * 应用的场景就是「访问局域网里的 http 服务」，没有别的选择；且页面内容是
 * 我们自己打进包里的，不存在被第三方脚本注入的风险面。
 *
 * ── 为什么不用 server.url 指向网关 ────────────────────────────
 * 那种做法把 App 变成一个去掉了地址栏的浏览器：切服务器要重载整个 WebView，
 * 登录也还是服务器那份网页，而且离线时连界面都起不来。这里要的是「界面在
 * 包里 + 地址可配 + 多台切换」，所以走打包 + 跨源请求。
 */
const config: CapacitorConfig = {
  appId: 'com.agent2api.mobile',
  appName: 'Agent2API',
  // 构建产物目录（npm run build:apk 的输出）
  webDir: 'android-www',
  android: {
    // 局域网网关普遍是 http，不开这个请求全被拦
    allowMixedContent: true,
  },
  server: {
    // 页面 origin 用 https://localhost：安全上下文，crypto.subtle 可用
    androidScheme: 'https',
    // 允许访问明文 http 的网关
    cleartext: true,
  },
}

export default config
