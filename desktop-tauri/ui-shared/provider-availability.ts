// 稳定 ID 仍用于存量账号；Rust 执行体恢复前不开放添加和调用。
// Kiro 已由「提供商增强」插件实现（2026-11），与后端
// `core/providers/availability.rs` 的 SUSPENDED_IDS 同步移出。
//
// 2026-11 又移出八家（cursor / devin / factory / zed / grok / kimi-work /
// opendesign / nous-portal）：它们由 Magpie 社区插件接手（见
// `script::catalog::recommendations`），后端同一份名单已放开 —— 两边必须一致，
// 只放开后端会变成「插件装好了、账号也加得上，界面上却找不到这家」。订阅族那
// 五家（`claude` / `codex` / `chatgpt` / `copilot` / `gemini` 的 `*-subscription`）
// 仍留着：它们是内置订阅身份，任何插件都申报不了（`script::runtime` 用
// `BUILTIN_SUBSCRIPTION_IDS` 硬挡），Magpie 的 Copilot 插件用的是
// `github-copilot`，与 `copilot-subscription` 不是同一个身份。
export const SUSPENDED_PROVIDER_IDS: ReadonlySet<string> = new Set([
  // 千问办公国际版：登录链路被上游挡住（只认它自己的回调地址），
  // 添加不了账号，先从可见列表摘掉；国内版 (`qwenwork`) 不受影响。
  'qwenwork-intl',
  'qwenwork-enterprise',
  'claude-subscription',
  'codex-subscription',
  'chatgpt-subscription',
  'copilot-subscription',
  'gemini-subscription',
])

export function isSuspendedProvider(id: string): boolean {
  return SUSPENDED_PROVIDER_IDS.has(id)
}

export function isAddAccountProviderHidden(id: string): boolean {
  return isSuspendedProvider(id)
}
