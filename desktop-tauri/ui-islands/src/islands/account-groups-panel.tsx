import * as React from 'react'
import {
  Button,
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogSection,
  DialogTitle,
  Input,
  Label,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  SegmentedControl,
} from '@ui'
import { t } from '../i18n'
import {
  accountGroupBridge,
  groupMembers,
  type AccountGroup,
  type AccountGroupMember,
  type AccountGroupMemberType,
} from './account-groups-domain'
import { esc, shared, type AccountRecord } from './accounts-shared'

type AccountGroupsPanelProps = {
  children: React.ReactNode
}

type GroupDraft = {
  group: AccountGroup | null
  name: string
  members: AccountGroupMember[]
}

type ProviderOption = {
  id: string
  label: string
}

function normalizeMembers(members: AccountGroupMember[]): AccountGroupMember[] {
  return members.map((member, position) => ({ ...member, position }))
}

function accountLabel(account: AccountRecord): string {
  return String(account.name || account.email || account.id)
}

function accountDisplayLabel(account: AccountRecord, providers: ProviderOption[]): string {
  const provider = providers.find(candidate => candidate.id === String(account.provider || '').trim())
  return `${accountLabel(account)}${provider?.label ? `（${provider.label}）` : ''}`
}

function memberLabel(
  member: AccountGroupMember,
  accounts: AccountRecord[],
  providers: ProviderOption[],
): string {
  const account = member.type === 'account'
    ? accounts.find(candidate => candidate.id === member.id)
    : undefined
  const provider = member.type === 'provider'
    ? providers.find(candidate => candidate.id === member.id)
    : undefined
  const label = account
    ? accountDisplayLabel(account, providers)
    : provider?.label || member.id
  return `${member.type === 'account' ? t('账号') : t('提供商')} · ${label}`
}

function removeCoveredAccounts(members: AccountGroupMember[], accounts: AccountRecord[]): AccountGroupMember[] {
  const providers = new Set(members.filter(member => member.type === 'provider').map(member => member.id))
  const accountProviders = new Map(
    accounts.map(account => [account.id, String(account.provider || '').trim()]),
  )
  return normalizeMembers(members.filter(member => (
    member.type !== 'account' || !providers.has(accountProviders.get(member.id) || '')
  )))
}

function GroupEditor({
  draft,
  accounts,
  providers,
  saving,
  onChange,
  onSave,
  onClose,
}: {
  draft: GroupDraft
  accounts: AccountRecord[]
  providers: ProviderOption[]
  saving: boolean
  onChange: (next: GroupDraft) => void
  onSave: () => void
  onClose: () => void
}) {
  const [memberType, setMemberType] = React.useState<AccountGroupMemberType>('account')
  const [memberId, setMemberId] = React.useState('')
  const accountProviders = React.useMemo(() => {
    const byId = new Map<string, string>()
    for (const account of accounts) {
      const provider = String(account.provider || '').trim()
      if (provider) byId.set(account.id, provider)
    }
    return byId
  }, [accounts])
  const selectedProviderIds = new Set(
    draft.members.filter(member => member.type === 'provider').map(member => member.id),
  )
  const candidates = memberType === 'account'
    ? accounts
      .filter(account => !selectedProviderIds.has(accountProviders.get(account.id) || ''))
      .map(account => ({ id: account.id, label: accountDisplayLabel(account, providers) }))
    : providers
  const selectedIds = new Set(draft.members.filter(member => member.type === memberType).map(member => member.id))
  const available = candidates.filter(candidate => !selectedIds.has(candidate.id))
  const memberKey = draft.members.map(member => `${member.type}:${member.id}`).join('|')

  React.useEffect(() => {
    setMemberId(available[0]?.id || '')
  }, [memberType, memberKey, accounts.length, providers.map(provider => `${provider.id}:${provider.label}`).join('|')])

  function addMember(): void {
    if (!memberId) return
    if (draft.members.some(member => member.type === memberType && member.id === memberId)) return
    const members = memberType === 'provider'
      ? removeCoveredAccounts([
        ...draft.members,
        { position: draft.members.length, type: memberType, id: memberId },
      ], accounts)
      : draft.members
    if (memberType === 'provider') {
      onChange({ ...draft, members })
      return
    }
    onChange({
      ...draft,
      members: normalizeMembers([...members, { position: members.length, type: memberType, id: memberId }]),
    })
  }

  function removeMember(position: number): void {
    onChange({ ...draft, members: normalizeMembers(draft.members.filter(member => member.position !== position)) })
  }

  function moveMember(position: number, offset: -1 | 1): void {
    const target = position + offset
    if (target < 0 || target >= draft.members.length) return
    const next = draft.members.slice()
    const [item] = next.splice(position, 1)
    next.splice(target, 0, item)
    onChange({ ...draft, members: normalizeMembers(next) })
  }

  return (
    <Dialog open onOpenChange={open => { if (!open && !saving) onClose() }}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{draft.group ? t('编辑账号组') : t('新建账号组')}</DialogTitle>
        </DialogHeader>
        <DialogBody>
          <DialogSection>
            <div className='flex items-center gap-2.5'>
              <Label htmlFor='account-group-name' className='text-[12.5px] whitespace-nowrap text-subtle'>{t('组名')}</Label>
              <Input id='account-group-name' value={draft.name} maxLength={80}
                placeholder={t('例如：生产账号')} onChange={event => onChange({ ...draft, name: event.currentTarget.value })} />
            </div>
          </DialogSection>
          <DialogSection>
            <div className='flex items-center gap-1.5'>
              <h3>{t('成员顺序')}</h3>
              <span className='tip-q' data-tip={t('1、可以添加整个提供商和某个单独的账号，添加了提供商以后，提供商内部的账号按照默认列表中的顺序进行路由。已经添加了的提供商内部账号不能再重复添加进当前账号组。\n2、一个 API key（网关 Key）暂时只能选择一个账号组进行路由。\n3、账号组内部顺序改动后，正在使用该账号组的 API key 路由顺序可能因为会话粘性暂时还按照旧顺序，会话粘性过期后，就会变成新顺序。')}></span>
            </div>
            {draft.members.length ? (
              <div className='flex flex-col gap-1.5'>
                {draft.members.map((member, index) => (
                  <div key={`${member.type}:${member.id}`} className='flex items-center gap-2 border-b border-hairline py-1.5 last:border-b-0'>
                    <span className='min-w-0 flex-1 truncate text-xs'>{memberLabel(member, accounts, providers)}</span>
                    <Button size='icon-sm' variant='ghost' disabled={index === 0} title={t('上移')}
                      aria-label={t('上移')} onClick={() => moveMember(index, -1)}>↑</Button>
                    <Button size='icon-sm' variant='ghost' disabled={index === draft.members.length - 1} title={t('下移')}
                      aria-label={t('下移')} onClick={() => moveMember(index, 1)}>↓</Button>
                    <Button size='icon-sm' variant='ghost' title={t('移除成员')} aria-label={t('移除成员')}
                      onClick={() => removeMember(member.position)}>×</Button>
                  </div>
                ))}
              </div>
            ) : <p>{t('还没有成员，保存后仍可继续编辑。')}</p>}
            <div className='flex flex-wrap items-center gap-2.5'>
              <Select value={memberType} onValueChange={value => setMemberType(String(value) as AccountGroupMemberType)}>
                <SelectTrigger className='min-w-[110px]' aria-label={t('成员类型')}>
                  <SelectValue>{memberType === 'account' ? t('账号') : t('提供商')}</SelectValue>
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value='account'>{t('账号')}</SelectItem>
                  <SelectItem value='provider'>{t('提供商')}</SelectItem>
                </SelectContent>
              </Select>
              <Select value={memberId} onValueChange={value => setMemberId(String(value))}>
                <SelectTrigger className='min-w-[220px] flex-1' aria-label={t('成员')}>
                  <SelectValue placeholder={available.length ? t('选择成员') : t('没有可添加的成员')}>
                    {candidates.find(candidate => candidate.id === memberId)?.label}
                  </SelectValue>
                </SelectTrigger>
                <SelectContent>
                  {available.map(candidate => <SelectItem key={candidate.id} value={candidate.id}>{candidate.label}</SelectItem>)}
                </SelectContent>
              </Select>
              <Button variant='outline' disabled={!memberId} onClick={addMember}>{t('添加')}</Button>
            </div>
          </DialogSection>
        </DialogBody>
        <DialogFooter>
          <div className='mr-auto' />
          <Button variant='outline' disabled={saving} onClick={onClose}>{t('取消')}</Button>
          <Button variant='default' disabled={saving || !draft.name.trim()} onClick={onSave}>{saving ? t('保存中…') : t('保存')}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function GroupList() {
  const [groups, setGroups] = React.useState<AccountGroup[]>([])
  const [accounts, setAccounts] = React.useState<AccountRecord[]>([])
  const [providers, setProviders] = React.useState<ProviderOption[]>([])
  const [loading, setLoading] = React.useState(true)
  const [status, setStatus] = React.useState('')
  const [draft, setDraft] = React.useState<GroupDraft | null>(null)
  const [saving, setSaving] = React.useState(false)

  const reload = React.useCallback(async () => {
    const api = accountGroupBridge()
    if (!api) {
      setStatus(t('后端桥不可用'))
      setLoading(false)
      return
    }
    try {
      const [groupPayload, session] = await Promise.all([api.getAccountGroups(), api.getState?.()])
      setGroups(Array.isArray(groupPayload?.groups) ? groupPayload.groups : [])
      const snapshot = session?.accounts
      const nextAccounts = Array.isArray(snapshot?.accounts) ? snapshot.accounts : []
      const providerLabels = new Map<string, string>()
      for (const item of Array.isArray(snapshot?.providers) ? snapshot.providers : []) {
        const id = typeof item?.id === 'string' ? item.id.trim() : ''
        if (id) providerLabels.set(id, String(item.label || id))
      }
      const providerIds: string[] = []
      for (const account of nextAccounts) {
        const id = String(account.provider || '').trim()
        if (id && !providerIds.includes(id)) providerIds.push(id)
      }
      setAccounts(nextAccounts)
      setProviders(providerIds.map(id => ({
        id,
        label: providerLabels.get(id) || shared().wbProviders?.labelOf?.(id) || id,
      })))
      setStatus('')
    } catch (error) {
      setStatus(error instanceof Error ? error.message : String(error))
    } finally {
      setLoading(false)
    }
  }, [])

  React.useEffect(() => { void reload() }, [reload])

  function openCreate(): void {
    setDraft({ group: null, name: '', members: [] })
  }

  function openEdit(group: AccountGroup): void {
    setDraft({ group, name: group.name, members: removeCoveredAccounts(groupMembers(group), accounts) })
  }

  async function saveDraft(): Promise<void> {
    if (!draft) return
    const api = accountGroupBridge()
    if (!api) return
    setSaving(true)
    try {
      const group = draft.group
        ? (await api.renameAccountGroup(draft.group.id, draft.name))?.group
        : (await api.createAccountGroup(draft.name))?.group
      if (!group) throw new Error(t('账号组保存失败'))
      const nextDraft = draft.group ? draft : { ...draft, group }
      if (!draft.group) setDraft(nextDraft)
      const saved = await api.replaceAccountGroupMembers(group.id, normalizeMembers(nextDraft.members))
      if (!saved?.group) throw new Error(t('账号组成员保存失败'))
      setDraft(null)
      await reload()
    } catch (error) {
      setStatus(error instanceof Error ? error.message : String(error))
    } finally {
      setSaving(false)
    }
  }

  async function deleteGroup(group: AccountGroup): Promise<void> {
    const api = accountGroupBridge()
    const ask = shared().wbConfirm?.ask
    if (!api || !ask) return
    const confirmed = await ask({
      title: t('删除账号组'),
      html: t('确定删除账号组「{name}」？', { name: `<strong>${esc(group.name)}</strong>` }),
      okText: t('删除'),
      okClass: 'danger',
    })
    if (!confirmed) return
    try {
      await api.deleteAccountGroup(group.id)
      await reload()
    } catch (error) {
      setStatus(error instanceof Error ? error.message : String(error))
    }
  }

  return (
    <section className='panel account-groups-panel'>
      <div className='panel-head'>
        <div>
          <h2>{t('账号组')}</h2>
          <p className='text-xs text-subtle'>{t('把多个账号或提供商组合成一条独立路由池。')}</p>
        </div>
        <div className='head-actions'>
          <Button variant='default' onClick={openCreate}>{t('＋ 新建账号组')}</Button>
          <Button variant='outline' onClick={() => void reload()}>{t('刷新')}</Button>
        </div>
      </div>
      {status ? <div className='px-5 py-3 text-xs text-danger'>{status}</div> : null}
      <div className='flex flex-col gap-2 p-5'>
        {loading ? <div className='empty'>{t('加载中…')}</div> : null}
        {!loading && !groups.length ? <div className='empty'>{t('还没有账号组')}</div> : null}
        {groups.map(group => (
          <div key={group.id} className='flex flex-wrap items-center gap-3 border-b border-hairline py-3 last:border-b-0'>
            <div className='min-w-[180px] flex-1'>
              <div className='text-sm font-medium'>{group.name}</div>
              <div className='text-xs text-subtle'>{t('{providers} 个提供商 · {accounts} 个账号', {
                providers: group.members.filter(member => member.type === 'provider').length,
                accounts: group.members.filter(member => member.type === 'account').length,
              })}</div>
              {group.missingMembers?.length ? <div className='text-xs text-danger'>{t('{n} 个成员引用已失效', { n: group.missingMembers.length })}</div> : null}
            </div>
            <div className='flex items-center gap-1.5'>
              <Button variant='outline' size='sm' onClick={() => openEdit(group)}>{t('编辑')}</Button>
              <Button variant='destructive' size='sm' onClick={() => void deleteGroup(group)}>{t('删除')}</Button>
            </div>
          </div>
        ))}
      </div>
      {draft ? (
        <GroupEditor draft={draft} accounts={accounts} providers={providers} saving={saving}
          onChange={setDraft} onSave={() => void saveDraft()} onClose={() => { if (!saving) setDraft(null) }} />
      ) : null}
    </section>
  )
}

export function AccountGroupsPanel({ children }: AccountGroupsPanelProps) {
  const [view, setView] = React.useState<'accounts' | 'groups'>('accounts')
  return (
    <>
      <div className='flex items-center gap-3 px-1 py-2'>
        <span className='text-xs font-medium text-subtle'>{t('账号')}</span>
        <SegmentedControl options={[
          { value: 'accounts', label: t('账号列表') },
          { value: 'groups', label: t('账号组') },
        ]} value={view} onValueChange={value => setView(value as 'accounts' | 'groups')} aria-label={t('账号页面视图')} />
      </div>
      {view === 'accounts' ? children : <GroupList />}
    </>
  )
}
