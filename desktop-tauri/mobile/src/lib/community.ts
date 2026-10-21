/**
 * 社区入口常量。
 *
 * ── 为什么单独一份 ────────────────────────────────────────────
 * 桌面端同一份数据在 `ui-islands/src/islands/update-shared.ts` 的
 * `QQ_GROUPS` / `COMMUNITY_LINKS`。手机端是独立工程（自己的 Vite 构建），
 * 不能 import 那个文件（它依赖桌面壳的桥）。**改群号 / 换链接时两处一起改**
 * —— 这里与那边是同一份事实的两个落点。
 *
 * ── 为什么只给群号，不给二维码 ────────────────────────────────
 * 与桌面端同一理由（见那边 `QQ_GROUPS` 的注释）：二维码是打进包里的静态图，
 * 换群 / 换码就得跟着发一次版；群号 + 复制按钮改一个常量即可。
 */

export type QqGroup = { no: string; nearlyFull: boolean }

export const QQ_GROUPS: readonly QqGroup[] = [
  { no: '1127656296', nearlyFull: true },
  { no: '1130894674', nearlyFull: false },
]

export type CommunityLink = { id: 'telegram' | 'discord'; url: string }

export const COMMUNITY_LINKS: readonly CommunityLink[] = [
  { id: 'telegram', url: 'https://t.me/+9gM8YPvgRL02NGRl' },
  { id: 'discord', url: 'https://discord.gg/z9GzSJMWp' },
]

/**
 * 用系统浏览器打开一个外链。
 *
 * ── 为什么 APK 里是「同窗跳转」而不是 window.open ─────────────
 * Capacitor 的 WebView 没开多窗口支持，`window.open` 会被直接忽略（返回
 * null，什么也不发生）。而主框架导航会走 `BridgeWebViewClient` 的
 * `shouldOverrideUrlLoading` → `bridge.launchIntent`：目标不是应用自己的
 * origin 时，它抛一个 `ACTION_VIEW` Intent 给系统，由默认浏览器接管，
 * WebView 自己停在原地。所以这里用 `location.href`。
 *
 * PWA 里没有那层壳，`window.open` 才是对的（新标签页，不丢当前页面）。
 */
export function openExternal(url: string): void {
  const target = String(url || '').trim()
  if (!/^https?:\/\//i.test(target)) return
  const packaged = Boolean(
    (window as Window & { Capacitor?: { isNativePlatform?: () => boolean } }).Capacitor?.isNativePlatform?.(),
  )
  if (packaged) {
    window.location.href = target
    return
  }
  window.open(target, '_blank', 'noopener')
}
