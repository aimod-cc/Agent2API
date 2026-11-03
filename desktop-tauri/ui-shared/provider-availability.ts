// 稳定 ID 仍用于存量账号；Rust 执行体恢复前不开放添加和调用。
export const SUSPENDED_PROVIDER_IDS: ReadonlySet<string> = new Set([
  'cursor',
  'kiro',
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
