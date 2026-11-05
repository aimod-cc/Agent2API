// 稳定 ID 仍用于存量账号；Rust 执行体恢复前不开放添加和调用。
// Kiro 已由「提供商增强」插件实现（2026-11），与后端
// `core/providers/availability.rs` 的 SUSPENDED_IDS 同步移出。
export const SUSPENDED_PROVIDER_IDS: ReadonlySet<string> = new Set([
  'cursor',
  'devin',
  'factory',
  'zed',
  'grok',
  'minimax-code',
  'minimax-code-global',
  // 千问办公国际版：登录链路被上游挡住（只认它自己的回调地址），
  // 添加不了账号，先从可见列表摘掉；国内版 (`qwenwork`) 不受影响。
  'qwenwork-intl',
  'qwenwork-enterprise',
  'kimi-work',
  'opendesign',
  'nous-portal',
  'claude-subscription',
  'codex-subscription',
  'chatgpt-subscription',
  'copilot-subscription',
  'gemini-subscription',
])

export function isSuspendedProvider(id: string): boolean {
  return SUSPENDED_PROVIDER_IDS.has(id)
}

// LobsterAI 封号严重；只隐藏新增入口，不影响存量账号的显示与调用。
const HIDDEN_ADD_ACCOUNT_PROVIDER_IDS: ReadonlySet<string> = new Set(['lobsterai'])

export function isAddAccountProviderHidden(id: string): boolean {
  return isSuspendedProvider(id) || HIDDEN_ADD_ACCOUNT_PROVIDER_IDS.has(id)
}
