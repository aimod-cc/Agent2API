import { activeGroupSortRules } from './accounts-group-settings'
import type { AccountSorting } from './accounts-shared'
import type { RoutingDraft } from './routing-editor-state'
import type { RouteProfile, RoutingWorkspace } from './routing-types'

const same = (left: unknown, right: unknown) => JSON.stringify(left) === JSON.stringify(right)
const rulesOf = (sorting: AccountSorting, provider: string) => sorting.rules[provider] || []

export function syncRoutingSorting(draft: RoutingDraft, sorting: AccountSorting): RoutingDraft {
  const before = draft.workspace.sorting
  if (before && sorting.updatedAt < before.updatedAt) return draft
  const profile = structuredClone(draft.profile), baseline = draft.baseline && structuredClone(draft.baseline)
  let inheritedProfile = draft.workspace.inheritedProfile
  if (draft.workspace.model && draft.mode === 'inherit') {
    inheritedProfile = structuredClone(inheritedProfile || draft.profile)
    for (const provider of inheritedProfile.providers) {
      if (sorting.accountOrder[provider]) inheritedProfile.accountOrder[provider] = [...sorting.accountOrder[provider]]
    }
  }
  if (draft.workspace.model && draft.mode === 'custom' && baseline) {
    for (const [provider, order] of Object.entries(sorting.accountOrder)) {
      if (!profile.providers.includes(provider)) continue
      const mode = profile.accountOrderMode?.[provider] ?? (profile.accountOrder?.[provider] ? 'manual' : 'followGroup')
      if (mode === 'followGroup') profile.accountOrder[provider] = [...order]
      const baseMode = baseline.accountOrderMode?.[provider] ?? (baseline.accountOrder?.[provider] ? 'manual' : 'followGroup')
      if (baseline.providers.includes(provider) && baseMode === 'followGroup') {
        baseline.accountOrder[provider] = [...order]
      }
    }
  }
  if (!draft.workspace.model && baseline) {
    for (const [provider, order] of Object.entries(sorting.accountOrder)) {
      if (!profile.providers.includes(provider) || !baseline.providers.includes(provider)) continue
      const rules = rulesOf(sorting, provider)
      const automatic = activeGroupSortRules(provider, rules, sorting).length > 0
      const rulesChanged = before && !same(rulesOf(before, provider), rules)
      if (automatic || rulesChanged || same(profile.accountOrder[provider], baseline.accountOrder[provider])) {
        profile.accountOrder[provider] = [...order]
      }
      baseline.accountOrder[provider] = [...order]
    }
  }
  return { ...draft, profile, baseline, workspace: { ...draft.workspace, sorting, inheritedProfile } }
}

function profileWithoutOrder(profile: RouteProfile) {
  const { accountOrder: _order, ...rest } = profile
  return rest
}

export function canSyncSortingRevision(before: RoutingWorkspace, next: RoutingWorkspace): boolean {
  if ((before.model || '') !== (next.model || '') || !same(profileWithoutOrder(before.profile), profileWithoutOrder(next.profile))) return false
  if (before.customized !== next.customized || before.settings.routingStrategy !== next.settings.routingStrategy
    || before.settings.routingSticky !== next.settings.routingSticky
    || (before.settings.routingPriority || 'sticky') !== (next.settings.routingPriority || 'sticky')) return false
  return Object.entries(next.profile.accountOrder).every(([provider, order]) => {
    if (same(before.profile.accountOrder[provider], order)) return true
    if (before.model && before.customized && before.profile.accountOrderMode?.[provider] === 'manual') return false
    const prior = before.sorting.rules[provider] || [], current = next.sorting.rules[provider] || []
    return activeGroupSortRules(provider, prior, before.sorting).length > 0
      || activeGroupSortRules(provider, current, next.sorting).length > 0 || !same(prior, current)
  }) && Object.keys(before.profile.accountOrder).every(provider => provider in next.profile.accountOrder)
}
