import { snapshot } from './accounts-store'
import { shared } from './accounts-shared'

export function applyAccountLimited(limited: Record<string, boolean> | undefined): boolean {
  const current = snapshot()
  if (!limited || !current) return false
  let changed = false
  const accounts = (current.accounts || []).map(account => {
    const value = limited[account.id]
    if (typeof value !== 'boolean' || value === account.listLimited) return account
    changed = true
    return { ...account, listLimited: value }
  })
  if (changed) {
    const limited = accounts.filter(account => account.enabled !== false && account.listLimited === true).length
    shared().wbApp?.applyAccounts?.({ ...current, accounts,
      summary: current.summary ? { ...current.summary, limited } : current.summary })
  }
  return changed
}
