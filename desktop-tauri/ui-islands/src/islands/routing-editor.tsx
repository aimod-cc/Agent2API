import * as React from 'react'
import { Badge, Button, Checkbox, Empty, Label, SegmentedControl, Spinner, cn, confirmDialog } from '@ui'
import { t } from '../i18n'
import { displayNameOf, providerOf } from './accounts-domain'
import { activeGroupSortRules, manualOrderBlockedMessage } from './accounts-group-settings'
import { accountFlowState, accountParticipation } from './routing-flow-model'
import { RoutingSelect } from './routing-select'
import { IconButton, RouteIcon, formatDuration, stickyLabel, strategyLabel } from './routing-ui'
import { draftDirty, type RoutingEditorState } from './routing-editor-state'
import type { ConfigTarget } from './routing-flow-types'
import type { ReplayRow, RouteProfile, RoutingWorkspace } from './routing-types'

type Props = { editor: RoutingEditorState; target: ConfigTarget; models: string[]; now: number; request: ReplayRow | null;
  onExecution(): void; onAccount(id: string): void; onSort(provider: string): void; onProtection(workspace: RoutingWorkspace): void }
export function RoutingEditor({ editor, target, models, now, request, onExecution, onAccount, onSort, onProtection }: Props) {
  const { draft, scope, busy, loading } = editor
  const scopeKey = scope.toLowerCase()
  const options = [...new Map([scope, ...models].filter(Boolean).map(model => [model.toLowerCase(), model])).values()]
  const selection = options.find(model => model.toLowerCase() === scopeKey) || scope
  const controlsDisabled = busy || loading
  if (!draft) return <div className='routing-workbench-empty'><Empty>{loading ? <Spinner /> : editor.error || t('选择作用范围后配置路由')}</Empty></div>
  const inherited = draft.mode === 'inherit'
  const workspace = draft.workspace
  const globalSettings = workspace.globalSettings || workspace.settings
  const profile = inherited ? workspace.inheritedProfile || draft.profile : draft.profile
  const dirty = draftDirty(draft)
  const providers = [...new Set([...profile.providers, ...workspace.providers.map(provider => provider.id)])]
  const label = (provider: string) => workspace.providers.find(item => item.id === provider)?.label || provider
  const mutate = (fn: (profile: RouteProfile) => void) => editor.update(current => { const profile = structuredClone(current.profile); fn(profile); return { ...current, profile } })
  const ensure = (next: RouteProfile, provider: string) => { if (!next.providers.includes(provider)) next.providers.push(provider); if (!next.accountOrder[provider]) next.accountOrder[provider] = workspace.accounts.filter(account => providerOf(account) === provider).map(account => account.id) }
  const toggleProvider = (provider: string, included: boolean) => mutate(next => { ensure(next, provider); next.disabledProviders = included ? next.disabledProviders.filter(id => id !== provider) : [...new Set([...next.disabledProviders, provider])] })
  const toggleAccount = (id: string, included: boolean) => mutate(next => { next.excludedAccounts = included ? next.excludedAccounts.filter(value => value !== id) : [...new Set([...next.excludedAccounts, id])] })
  const move = (kind: 'provider' | 'account', provider: string, id: string, direction: number) => mutate(next => {
    ensure(next, provider)
    const values = kind === 'provider' ? next.providers : next.accountOrder[provider]
    const index = values.indexOf(id), to = index + direction
    if (index >= 0 && to >= 0 && to < values.length) [values[index], values[to]] = [values[to], values[index]]
  })
  const enabledProfile = { ...workspace, profile }
  const participants = workspace.accounts.filter(account => accountParticipation(enabledProfile, account))
  const refresh = async () => {
    if (dirty && !await confirmDialog(t('重新读取会丢弃当前范围的未保存草稿，其他范围的草稿保留。'), { title: t('重新读取配置'), confirmText: t('重新读取') })) return
    void editor.load(scope, true)
  }
  return <>
    <div className='routing-inspector-head'><b><RouteIcon name='settings' />{t('当前配置')}</b><Badge variant={dirty ? 'warning' : 'secondary'} shape='tag'>{dirty ? t('未保存草稿') : t('已保存')}</Badge></div>
    <div className='routing-inspector-body routing-editor-body'>
      <Label>{t('作用范围')}</Label><RoutingSelect label={t('配置作用范围')} value={selection || '__global'} disabled={busy} options={[[ '__global', t('全局默认') ], ...options.map(model => [model, model] as [string, string])]} onChange={value => void editor.load(value === '__global' ? '' : value)} />
      {request ? <div className='routing-editor-context'><RouteIcon name='lock' size={13} /><span>{t('回放保持不变')}<code>{request.id}</code></span><Button variant='link' size='sm' onClick={onExecution}>{t('本次执行')}</Button></div> : null}
      {scope ? <><SegmentedControl aria-label={t('配置来源')} value={draft.mode} disabled={controlsDisabled} options={[{ value: 'inherit', label: t('继承全局') }, { value: 'custom', label: t('模型独立配置') }]} onValueChange={mode => editor.update(current => ({ ...current, mode }))} /><p className='routing-editor-hint'>{inherited ? t('当前继承全局，切换到独立配置后只修改本模型。') : t('当前模型配置独立保存，不影响全局和其他模型。')}</p></> : <p className='routing-editor-hint'>{t('全局修改影响继承它的模型；独立模型配置不变。')}</p>}
      <div className='routing-editor-fields'><div><Label>{t('路由策略')}</Label><RoutingSelect label={t('路由策略')} value={profile.strategy || '__inherit'} disabled={controlsDisabled || inherited} options={[...(scope ? [[ '__inherit', t('继承全局：{value}', { value: strategyLabel(globalSettings.routingStrategy) }) ] as [string, string]] : []), ['order', t('顺序优先')], ['rotate', t('负载均衡')], ['provider', t('家内轮询')]]} onChange={value => mutate(next => { next.strategy = value === '__inherit' ? undefined : value })} /></div>
        <div><Label>{t('会话粘滞')}</Label><RoutingSelect label={t('会话粘滞')} value={profile.sticky || '__inherit'} disabled={controlsDisabled || inherited} options={[...(scope ? [[ '__inherit', t('继承全局：{value}', { value: stickyLabel(globalSettings.routingSticky) }) ] as [string, string]] : []), ['auto', t('自动')], ['session', t('会话内')], ['turn', t('每轮')], ['off', t('关闭')]]} onChange={value => mutate(next => { next.sticky = value === '__inherit' ? undefined : value })} /></div></div>
      <p className='routing-editor-hint'>{t('自动：空闲五分钟释放；会话内：空闲六小时释放；每轮或关闭：不保留跨请求绑定。')}</p>
      <Label>{t('提供商参与范围')}</Label><RoutingSelect label={t('提供商参与范围')} value={profile.providerScope || 'selected'} disabled={controlsDisabled || inherited} options={[[ 'all', t('自动参与') ], ['selected', t('指定提供商')]]} onChange={value => mutate(next => { next.providerScope = value === 'all' ? 'all' : 'selected'; next.providers = providers; if (value === 'selected') next.disabledProviders = next.disabledProviders.filter(id => next.providers.includes(id)) })} />
      <div className='routing-editor-section-head'><b>{t('提供商与账号')}</b><span>{t('{n} 个参与账号', { n: participants.length })}</span></div>
      <p className='routing-editor-hint'>{t('顺序调整不会解除限流；有效粘滞优先于普通账号顺序。')}</p>
      <div className='routing-editor-providers'>{providers.map(provider => {
        const enabled = (profile.providerScope === 'all' || profile.providers.includes(provider)) && !profile.disabledProviders.includes(provider)
        const accounts = workspace.accounts.filter(account => providerOf(account) === provider)
        const order = [...new Set([...(profile.accountOrder[provider] || []), ...accounts.map(account => account.id)])].filter(id => accounts.some(account => account.id === id))
        const rules = !scope ? activeGroupSortRules(provider, workspace.sorting?.rules?.[provider] || []) : []
        const locked = rules.length > 0
        const providerIndex = profile.providers.indexOf(provider)
        return <section key={provider} className={cn('routing-editor-provider', target.id === provider && 'is-target')}>
          <header><Label><Checkbox checked={enabled} disabled={controlsDisabled || inherited} onCheckedChange={on => toggleProvider(provider, on === true)} />{label(provider)}</Label><span><IconButton name='arrowUp' title={t('提高优先级')} disabled={controlsDisabled || inherited || providerIndex <= 0} onClick={() => move('provider', provider, provider, -1)} /><IconButton name='arrowDown' title={t('降低优先级')} disabled={controlsDisabled || inherited || providerIndex < 0 || providerIndex >= profile.providers.length - 1} onClick={() => move('provider', provider, provider, 1)} /></span></header>
          {locked ? <div className='routing-editor-sort-note'><span>{manualOrderBlockedMessage(rules)}</span><Button variant='link' size='sm' disabled={busy} onClick={() => onSort(provider)}>{t('消耗规则')}</Button></div> : null}
          {order.map((id, index) => {
            const account = accounts.find(account => account.id === id)!, status = accountFlowState(workspace, account, now)
            return <div key={id} className={cn('routing-editor-account', target.id === id && 'is-target')}><span>{index + 1}</span><Label><Checkbox checked={!profile.excludedAccounts.includes(id)} disabled={controlsDisabled || inherited || !enabled} onCheckedChange={on => toggleAccount(id, on === true)} /><span><b>{displayNameOf(account)}</b><small>{status.label}{status.resetAt > now ? ` · ${formatDuration(status.resetAt - now)}` : ''}</small></span></Label><span><IconButton name='arrowUp' title={t('提高优先级')} disabled={controlsDisabled || inherited || !enabled || locked || index === 0} onClick={() => move('account', provider, id, -1)} /><IconButton name='arrowDown' title={t('降低优先级')} disabled={controlsDisabled || inherited || !enabled || locked || index === order.length - 1} onClick={() => move('account', provider, id, 1)} /><IconButton name='settings' title={t('账号设置')} disabled={busy} onClick={() => onAccount(id)} /></span></div>
          })}{!order.length ? <p className='routing-editor-hint'>{t('该提供商没有账号，请先到账号页添加。')}</p> : null}
        </section>
      })}</div>
      <Button variant='outline' size='sm' disabled={busy} onClick={() => onProtection(workspace)}>{t('全局保护设置')}</Button>
      {editor.error ? <div className='routing-editor-error' role='alert'>{editor.error}<Button variant='link' size='sm' disabled={busy} onClick={() => void refresh()}>{t('重新读取配置')}</Button></div> : null}
      {!participants.length && !inherited ? <p className='routing-editor-hint'>{t('当前没有参与账号，保存后这个范围可能无法承载新请求。')}</p> : null}
    </div>
    <div className='routing-inspector-foot routing-editor-footer'><span>{t('保存仅影响后续选路，历史快照与在途请求不改写。')}</span><div><Button variant='outline' size='sm' disabled={!dirty || controlsDisabled} onClick={editor.discard}>{t('撤销草稿')}</Button><Button size='sm' disabled={!dirty || busy || loading} onClick={() => void editor.save()}>{busy ? t('保存中…') : t('保存当前配置')}</Button></div></div>
  </>
}
