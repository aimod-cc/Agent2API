import type { CapacitorConfig } from '@capacitor/cli'

/**
 * Capacitor 配置（Android APK + iOS IPA 共用一份 Web 产物）。
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
 * ── iOS 这边为什么不能照抄 androidScheme ──────────────────────
 * `iosScheme` 不允许是 `https` / `http` —— 那是 WKWebView 自己处理的协议，
 * Capacitor 在 `InstanceDescriptor.normalize()` 里会把它判为非法并**静默退回**
 * 默认值 `capacitor`（见 @capacitor/ios 的 CAPInstanceDescriptor.swift:169）。
 * 所以 iOS 的页面 origin 固定是 `capacitor://localhost`。
 *
 * 代价是它不算「安全上下文」，`crypto.subtle` 在 iOS 上不可用 —— 登录页的
 * 人机验证会自动走 `src/api/auth.ts` 里的纯 JS SHA-256 兜底（那条路径本来
 * 就是为 http 局域网准备的，只是慢一点，结果一致）。
 *
 * ── iOS 的明文 http 网关怎么放行 ──────────────────────────────
 * 靠 `ios/App/App/Info.plist` 的 `NSAppTransportSecurity`（ATS）例外，不在
 * 这里配。网关是局域网 IP + http，iOS 17 起 ATS 默认连 IP 地址都拦，所以
 * 那边同时开了 `NSAllowsArbitraryLoads` 与 `NSAllowsLocalNetworking`。
 *
 * ── 为什么不用 server.url 指向网关 ────────────────────────────
 * 那种做法把 App 变成一个去掉了地址栏的浏览器：切服务器要重载整个 WebView，
 * 登录也还是服务器那份网页，而且离线时连界面都起不来。这里要的是「界面在
 * 包里 + 地址可配 + 多台切换」，所以走打包 + 跨源请求。
 */
const config: CapacitorConfig = {
  appId: 'com.agent2api.mobile',
  appName: 'Agent2API',
  // 构建产物目录（npm run build:native 的输出，Android / iOS 共用）
  webDir: 'android-www',
  android: {
    // 局域网网关普遍是 http，不开这个请求全被拦
    allowMixedContent: true,
  },
  ios: {
    // 安全区由页面自己的 env(safe-area-inset-*) 接管：WKWebView 不再额外
    // 加一圈内边距，否则刘海 / 底部横条会被算两次，顶部多出一条空白。
    contentInset: 'never',
    // 与 Android 一致：底色交给页面主题，避免深色模式下边缘闪白
    backgroundColor: '#f3f3f6',
  },
  server: {
    // 页面 origin 用 https://localhost：安全上下文，crypto.subtle 可用
    androidScheme: 'https',
    // 允许访问明文 http 的网关
    cleartext: true,
  },
  plugins: {
    Keyboard: {
      // iOS 默认的 native 模式会把整个 WebView 缩短，页面里的 fixed / flex
      // 布局会跟着抖一下；body 模式只改 body 高度，配合 visualViewport 的
      // 键盘判断（Shell.tsx 的 useKeyboardOpen）更稳。
      resize: 'body',
      // Android 全屏时键盘不会触发 resize，插件需要额外处理
      resizeOnFullScreen: true,
    },
  },
}

export default config
