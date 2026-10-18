/**
 * 按提供商区分的事实（有效期字段 / 展示名 / 标识字段）。
 *
 * ── 为什么要这张表 ────────────────────────────────────────────
 * 「有效期」落在账号记录的哪个键上**按家不同**：WorkBuddy 写 `expiresAt`，
 * 小浣熊 / CatPaw / AutoClaw 写 `tokenExpiresAt`，KukuAI 压根没有这个字段。
 * 统一读一个键会出现「状态正常但有效期显示已过期」这种自相矛盾 —— 那是读了
 * 另一个家留下的空键。
 *
 * 这张表从桌面端 `ui-islands/src/islands/accounts-domain.ts` 的
 * `PROVIDER_FEATURES` 摘出来，只保留手机端用到的三列。桌面端那张表还有
 * usage / creditDetail / claim 等能力位，手机端不消费，不搬。
 *
 * ── 加一家怎么办 ──────────────────────────────────────────────
 * 认不出的 provider 走兜底：两个键都看，取先有值的那个。宁可兜底，
 * 也不要因为漏登记而在界面上少一行读数。
 */

export type ProviderFacts = {
  /** 有效期字段名；空串 = 这家没有有效期概念（不渲染那一行） */
  expiry: 'expiresAt' | 'tokenExpiresAt' | ''
  /** 账号标识字段名（`uid` / `userId` / `account`），手机端只在详情里显示 */
  identifier: string
}

const FACTS: Record<string, ProviderFacts> = {
  workbuddy: { expiry: 'expiresAt', identifier: 'uid' },
  'workbuddy-intl': { expiry: 'expiresAt', identifier: 'uid' },
  raccoon: { expiry: 'tokenExpiresAt', identifier: 'userId' },
  catpaw: { expiry: 'tokenExpiresAt', identifier: 'uid' },
  autoclaw: { expiry: 'tokenExpiresAt', identifier: 'userId' },
  'autoclaw-intl': { expiry: 'tokenExpiresAt', identifier: 'userId' },
  qoder: { expiry: 'expiresAt', identifier: 'userId' },
  'qoder-intl': { expiry: 'expiresAt', identifier: 'userId' },
  'cline-free': { expiry: 'expiresAt', identifier: 'account' },
  'cline-pass': { expiry: 'expiresAt', identifier: 'account' },
  accio: { expiry: 'expiresAt', identifier: 'userId' },
  'accio-cn': { expiry: 'expiresAt', identifier: 'userId' },
  zcode: { expiry: 'expiresAt', identifier: 'userId' },
  'zcode-intl': { expiry: 'expiresAt', identifier: 'userId' },
  codearts: { expiry: 'expiresAt', identifier: 'userId' },
  trae: { expiry: 'expiresAt', identifier: 'uid' },
  loomy: { expiry: 'expiresAt', identifier: 'userId' },
  kuku: { expiry: '', identifier: 'uid' },
  officeace: { expiry: 'expiresAt', identifier: 'userId' },
  chatglm: { expiry: 'expiresAt', identifier: 'userId' },
  antigravity: { expiry: 'expiresAt', identifier: 'userId' },
  orcaterm: { expiry: 'expiresAt', identifier: 'userId' },
}

const FALLBACK: ProviderFacts = { expiry: 'expiresAt', identifier: 'userId' }

export function providerFacts(id: string | undefined): ProviderFacts {
  return (id && FACTS[id]) || FALLBACK
}

/**
 * 取账号的有效期时间戳。
 *
 * 顺序：本家登记的键 → 另一个键 → `tokenExpiresAt`（旧版通用字段）。
 * 全部为空时返回 0（界面显示「—」，不显示「已过期」—— 那两件事不一样）。
 */
export function expiryOf(account: {
  provider?: string
  expiresAt?: number
  tokenExpiresAt?: number
}): number {
  const facts = providerFacts(account.provider)
  if (facts.expiry === 'expiresAt') {
    return Number(account.expiresAt) || Number(account.tokenExpiresAt) || 0
  }
  if (facts.expiry === 'tokenExpiresAt') {
    return Number(account.tokenExpiresAt) || Number(account.expiresAt) || 0
  }
  // 这家没有有效期概念（KukuAI 之类）：两个键都不看
  return 0
}

/** 这家有没有「有效期」这个概念（决定详情里渲不渲染那一格） */
export function hasExpiry(provider: string | undefined): boolean {
  return providerFacts(provider).expiry !== ''
}
