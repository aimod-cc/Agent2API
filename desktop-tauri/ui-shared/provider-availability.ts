// 稳定 ID 仍用于存量账号；Rust 执行体恢复前不开放添加和调用。
// Kiro 已由「提供商增强」插件实现（2026-11），与后端
// `core/providers/availability.rs` 的 SUSPENDED_IDS 同步移出。
//
// 2026-11 又移出八家（cursor / devin / factory / zed / grok / kimi-work /
// opendesign / nous-portal）：它们由 Magpie 社区插件接手（见
// `script::catalog::recommendations`），后端同一份名单已放开 —— 两边必须一致，
// 只放开后端会让可用账号在添加页消失。Claude、ChatGPT、Codex 已由本体执行；
// Copilot 和 Gemini 的订阅身份仍停用，不与同品牌插件身份混用。
export const SUSPENDED_PROVIDER_IDS: ReadonlySet<string> = new Set([
  // 千问办公国际版：登录链路被上游挡住（只认它自己的回调地址），
  // 添加不了账号，先从可见列表摘掉；国内版 (`qwenwork`) 不受影响。
  'qwenwork-intl',
  'qwenwork-enterprise',
  'copilot-subscription',
  'gemini-subscription',
])

export function isSuspendedProvider(id: string): boolean {
  return SUSPENDED_PROVIDER_IDS.has(id)
}

export function isAddAccountProviderHidden(id: string): boolean {
  return isSuspendedProvider(id)
}
