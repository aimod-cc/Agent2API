import * as React from 'react'
import { t } from '../i18n'
import { providerOf } from './accounts-domain'
import type { AccountRecord } from './accounts-shared'

export const routingCollapsedSections = [
  { key: 'excluded', label: t('未参与本路由') },
  { key: 'disabled', label: t('账号全部禁用') },
  { key: 'empty', label: t('还没有账号') },
] as const

type CollapsedSection = typeof routingCollapsedSections[number]['key']
type AccountSection = 'active' | CollapsedSection

// 分组只改变展示位置，保留每组内部的原始顺序，不改写路由优先级。
export function routingAccountSections<T>(items: T[], accounts: AccountRecord[], provider: (item: T) => string, included: (item: T) => boolean) {
  const counts = new Map<string, { total: number; enabled: number }>()
  for (const account of accounts) {
    const id = providerOf(account)
    const count = counts.get(id) || { total: 0, enabled: 0 }
    count.total++
    if (account.enabled !== false) count.enabled++
    counts.set(id, count)
  }
  const sections: Record<AccountSection, T[]> = { active: [], excluded: [], disabled: [], empty: [] }
  for (const item of items) {
    const count = counts.get(provider(item))
    const section = !count?.total ? 'empty' : !count.enabled ? 'disabled' : included(item) ? 'active' : 'excluded'
    sections[section].push(item)
  }
  return sections
}

export function useRoutingExpansion() {
  const [opened, setOpened] = React.useState<Record<string, boolean>>({})
  const [sectionOpen, setSectionOpen] = React.useState<Partial<Record<CollapsedSection, boolean>>>({})
  const [search, setSearch] = React.useState('')
  React.useEffect(() => {
    const reset = (event: Event) => {
      if ((event as CustomEvent<unknown>).detail !== 'routing') return
      setOpened({})
      setSectionOpen({})
      setSearch('')
    }
    window.addEventListener('wb:page', reset)
    return () => window.removeEventListener('wb:page', reset)
  }, [])
  const query = search.trim().toLowerCase()
  const isOpen = (id: string) => Boolean(query) || Boolean(opened[id])
  return { opened, setOpened, sectionOpen, setSectionOpen, search, setSearch, query, isOpen }
}
