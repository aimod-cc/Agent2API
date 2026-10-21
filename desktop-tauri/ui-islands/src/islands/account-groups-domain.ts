import type { AccountRecord, AccountsSnapshot } from './accounts-shared'

export type AccountGroupMemberType = 'account' | 'provider'

export type AccountGroupMember = {
  position: number
  type: AccountGroupMemberType
  id: string
}

export type AccountGroupPreview = {
  accounts?: AccountRecord[]
  providers?: Array<{ id?: string; label?: string }>
}

export type AccountGroup = {
  id: string
  name: string
  createdAt?: number
  updatedAt?: number
  members: AccountGroupMember[]
  missingMembers?: AccountGroupMember[]
  preview?: AccountGroupPreview
}

export type AccountGroupsPayload = { groups?: AccountGroup[] }

export type AccountGroupBridge = {
  getAccountGroups(): Promise<AccountGroupsPayload | null | undefined>
  createAccountGroup(name: string): Promise<{ group?: AccountGroup } | null | undefined>
  renameAccountGroup(id: string, name: string): Promise<{ group?: AccountGroup } | null | undefined>
  replaceAccountGroupMembers(
    id: string,
    members: AccountGroupMember[],
  ): Promise<{ group?: AccountGroup } | null | undefined>
  deleteAccountGroup(id: string): Promise<unknown>
  getState?(): Promise<{ accounts?: AccountsSnapshot } | null | undefined>
}

export type AccountGroupWindow = {
  workbuddyDesktop?: AccountGroupBridge
}

export function accountGroupBridge(): AccountGroupBridge | undefined {
  return (window as unknown as AccountGroupWindow).workbuddyDesktop
}

export function groupMembers(group: AccountGroup): AccountGroupMember[] {
  return Array.isArray(group.members) ? group.members.slice().sort((a, b) => a.position - b.position) : []
}
