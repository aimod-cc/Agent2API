export const ANTIGRAVITY_PROVIDER = 'antigravity'
export const ANTIGRAVITY_ACP_PROVIDER = 'agy-cli'

export function isAntigravityAccountProvider(provider: string): boolean {
  return provider === ANTIGRAVITY_PROVIDER || provider === ANTIGRAVITY_ACP_PROVIDER
}

export function antigravityLoginProviders<T extends { id?: string; loginAvailable?: boolean }>(providers: readonly T[]): T[] {
  return [ANTIGRAVITY_PROVIDER, ANTIGRAVITY_ACP_PROVIDER]
    .flatMap(id => providers.find(provider => provider.id === id && provider.loginAvailable !== false) || [])
}

export function groupAntigravityCards<T extends { id?: string; count?: number; label?: string; loginAvailable?: boolean }>(providers: readonly T[]): T[] {
  const members = antigravityLoginProviders(providers)
  const primary = members[0]
  return providers.flatMap(provider => {
    if (!isAntigravityAccountProvider(provider.id || '')) return [provider]
    if (provider.id !== primary?.id) return []
    return [{ ...provider, label: 'Antigravity', count: members.reduce((total, item) => total + (Number(item.count) || 0), 0) }]
  })
}
