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
 * `PROVIDER_FEATURES` 摘出来，只保留手机端用到的四列（有效期 / 标识 /
 * 长期有效 / 余额查询能力）。桌面端那张表还有 creditDetail / claim 等
 * 能力位，手机端不消费，不搬。
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
  /**
   * 主凭证**长期有效**（没有固定到期时刻）：有效期显示「长期有效」而不是「—」。
   *
   * 与 `expiry: ''` 的区别是语义 —— 后者只说「读不到到期时间」（可能是数据
   * 缺失，也可能上游根本不给），前者是明确断言「本来就没有到期这回事」。
   * 判据见桌面端 `accounts-domain.ts` 的同名字段：只收「令牌本身不过期」与
   * 「有不过期的主凭证 + 短期令牌自动续期」两类，**不收**「无续期、只能重登」
   * 的家（Loomy / MonkeyCode / CatPaw 的凭证有固定寿命）。
   */
  longLived?: boolean
  /** 这家有没有余额查询能力（决定账号行/详情里渲不渲染余额读数） */
  usage: boolean
}

const FACTS: Record<string, ProviderFacts> = {
  workbuddy: { expiry: 'expiresAt', identifier: 'uid', usage: true },
  'workbuddy-intl': { expiry: 'expiresAt', identifier: 'uid', usage: true },
  raccoon: { expiry: 'tokenExpiresAt', identifier: 'userId', usage: true },
  catpaw: { expiry: 'tokenExpiresAt', identifier: 'uid', usage: true },
  autoclaw: { expiry: 'tokenExpiresAt', identifier: 'userId', usage: true },
  'autoclaw-intl': { expiry: 'tokenExpiresAt', identifier: 'userId', usage: true },
  qoder: { expiry: 'expiresAt', identifier: 'userId', usage: true },
  'qoder-intl': { expiry: 'expiresAt', identifier: 'userId', usage: true },
  'cline-free': { expiry: 'expiresAt', identifier: 'account', usage: true },
  'cline-pass': { expiry: 'expiresAt', identifier: 'account', usage: true },
  accio: { expiry: 'expiresAt', identifier: 'userId', usage: true },
  'accio-cn': { expiry: 'expiresAt', identifier: 'userId', usage: true },
  zcode: { expiry: 'expiresAt', identifier: 'userId', usage: true },
  'zcode-intl': { expiry: 'expiresAt', identifier: 'userId', usage: true },
  codearts: { expiry: 'expiresAt', identifier: 'userId', usage: true },
  trae: { expiry: 'expiresAt', identifier: 'uid', usage: true },
  loomy: { expiry: 'expiresAt', identifier: 'userId', usage: true },
  // BDUSS 读不出到期时刻，但上游随时可能作废、且没有续期接口 —— 只算「没有
  // 可显示的到期时间」，不算长期有效（与桌面端同一处置）。
  kuku: { expiry: '', identifier: 'uid', usage: true },
  officeace: { expiry: 'expiresAt', identifier: 'userId', usage: true },
  // 令牌有效期写在 JWT 的 `exp` 里、由网关自动续期（访客则由网关用同一设备号
  // 重新换发），用户不需要关心到期时刻。
  chatglm: { expiry: '', identifier: 'uid', longLived: true, usage: true },  // refresh token 不过期、access token 由网关提前 15 分钟自动续期。
  antigravity: { expiry: '', identifier: 'userId', longLived: true, usage: true },
  // `ms-` 令牌是长期有效的 API key，没有到期时刻可算。
  modelscope: { expiry: '', identifier: 'userId', longLived: true, usage: true },
  'modelscope-intl': { expiry: '', identifier: 'userId', longLived: true, usage: true },
  // 单个静态 API Key，上游没有续期接口、key 本身也没有到期时刻。
  // 这家**没有余额查询**（`/alpha/billing/*` 只在添加账号时用），故 usage: false。
  commandcode: { expiry: '', identifier: 'id', longLived: true, usage: false },
  orcaterm: { expiry: 'expiresAt', identifier: 'userId', usage: true },
}

const FALLBACK: ProviderFacts = { expiry: 'expiresAt', identifier: 'userId', usage: false }

export function providerFacts(id: string | undefined): ProviderFacts {
  return (id && FACTS[id]) || FALLBACK
}

/** 这家账号能不能查余额：自定义提供商看 balanceQuery.scheme，内置家看能力位 */
export function supportsUsage(account: { provider?: string; balanceQuery?: unknown }): boolean {
  const id = String(account.provider || '')
  if (id.startsWith('custom-')) {
    const scheme = (account.balanceQuery as { scheme?: unknown } | undefined)?.scheme
    return Boolean(scheme)
  }
  return providerFacts(id).usage
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
  const facts = providerFacts(provider)
  return facts.expiry !== '' || facts.longLived === true
}

/**
 * 这家的主凭证是不是**长期有效**（没有固定到期时刻）。
 *
 * 有它的家照样渲染「有效期」那一格，只是值写「长期有效」而不是「—」：
 * 「—」读作「记录里没有到期时间」，长期有效读作「本来就不会到期」。
 */
export function isLongLivedCredential(provider: string | undefined): boolean {
  return providerFacts(provider).longLived === true
}
