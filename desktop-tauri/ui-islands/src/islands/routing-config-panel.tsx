import * as React from 'react'
import { Badge, Button, Checkbox, Empty, Input, Label, Spinner, cn, confirmDialog } from '@ui'
import { t } from '../i18n'
import { displayNameOf, providerOf } from './accounts-domain'
import { activeGroupSortRules, groupSortSummary, manualOrderBlockedMessage } from './accounts-group-settings'
import { accountFlowState } from './routing-flow-model'
import { RoutingLivePanel } from './routing-live-panel'
import { RoutingSelect } from './routing-select'
import { IconButton, RouteIcon, formatDuration, stickyLabel, strategyLabel } from './routing-ui'
import { draftDirty, type RoutingDraft, type RoutingEditorState } from './routing-editor-state'
import { bridge, errorMessage, host, type RouteProfile, type RoutingWorkspace } from './routing-types'

type Props = {
  editor: RoutingEditorState
  scope: string
  now: number
  onScope(model: string): void
  onAccount(id: string): void
  onSort(provider: string): void
  onAddModel(): void
  onProtection(workspace: RoutingWorkspace): void
}

/**
 * 路由配置：左边作用范围，中间有序名单，右边「此刻队列」预演。
 *
 * 全局默认时，中间改的是全局名单；右侧可以选一个模型，看它实际会走哪套配置。
 * 如果这个模型有自己的独立配置，右侧会明确提示「你改的全局默认不影响它」。
 * 模型独立配置时，右侧直接预览这个模型自己。
 */
export function RoutingConfigPanel({ editor, scope, now, onScope, onAccount, onSort, onAddModel, onProtection }: Props) {
  const [opened, setOpened] = React.useState<ReadonlySet<string>>(new Set())
  const [search, setSearch] = React.useState('')
  const [showEmpty, setShowEmpty] = React.useState(false)
  const [previewModel, setPreviewModel] = React.useState('')
  const [preview, setPreview] = React.useState<RoutingWorkspace | null>(null)
  const [previewLoading, setPreviewLoading] = React.useState(false)
  const [previewError, setPreviewError] = React.useState('')
  const [previewToken, setPreviewToken] = React.useState(0)
  const previewSequence = React.useRef(0)
  const autoOpened = React.useRef<ReadonlySet<string>>(new Set())
  const draft = editor.draft
  const disabled = editor.busy || editor.loading
  const mutate = React.useCallback((fn: (profile: RouteProfile, workspace: RoutingWorkspace) => void) => {
    editor.update(current => { const profile = structuredClone(current.profile); fn(profile, current.workspace); return { ...current, profile } })
  }, [editor])
  const drop = React.useCallback(({ kind, provider, id, target, after }: { kind: 'provider' | 'account'; provider: string; id: string; target: string; after: boolean }) => {
    const move = (ids: string[]) => {
      const from = ids.indexOf(id)
      if (from < 0) return
      ids.splice(from, 1)
      const to = ids.indexOf(target)
      ids.splice(to < 0 ? from : after ? to + 1 : to, 0, id)
    }
    mutate(profile => {
      if (kind === 'provider') move(profile.providers)
      else move(profile.accountOrder[provider] || (profile.accountOrder[provider] = []))
    })
  }, [mutate])
  const drag = useDraftDrag(!disabled, drop)

  // 预览模型：全局范围时可选任意模型；模型范围时固定为当前模型。
  const revision = draft?.revision
  React.useEffect(() => {
    if (!draft) { setPreview(null); setPreviewError(''); setPreviewLoading(false); return }
    let target = ''
    if (scope) {
      target = scope
      if (previewModel !== scope) { setPreviewModel(scope); return }
    } else {
      const models = draft.workspace.models
      const last = draft.workspace.queue.lastRequestModel || ''
      const known = models.find(model => model.toLowerCase() === previewModel.toLowerCase())
      const fallback = models.find(model => model.toLowerCase() === last.toLowerCase()) || models[0] || ''
      target = known || fallback
      if (target !== previewModel) { setPreviewModel(target); return }
    }
    if (!target) { setPreview(null); setPreviewLoading(false); return }
    let alive = true
    const id = ++previewSequence.current
    setPreviewLoading(true); setPreviewError('')
    bridge().getRoutingWorkspace(target).then(next => {
      if (alive && id === previewSequence.current) { setPreview(next); setPreviewLoading(false) }
    }).catch(failure => {
      if (alive && id === previewSequence.current) { setPreviewError(errorMessage(failure)); setPreviewLoading(false) }
    })
    return () => { alive = false }
  }, [scope, previewModel, revision, previewToken, draft?.workspace])

  if (!draft) return <div className='routing-config-loading'><Empty>{editor.loading ? <Spinner /> : editor.error || t('选择作用范围后配置路由')}</Empty></div>

  const workspace = draft.workspace
  const inherited = draft.mode === 'inherit'
  const profile = inherited ? workspace.inheritedProfile || draft.profile : draft.profile
  const locked = disabled || inherited
  const dirty = draftDirty(draft)
  const previewCustom = Boolean(preview?.customized)
  const previewDirty = dirty && (scope ? true : !previewCustom)
  const changes = describeDraft(draft, provider => workspace.providers.find(item => item.id === provider)?.label || provider)
  const label = (provider: string) => workspace.providers.find(item => item.id === provider)?.label || provider
  const scopeLabel = scope || t('全局默认')
  const query = search.trim().toLowerCase()
  const configured = new Set(workspace.configuredModels.map(model => model.toLowerCase()))
  const previewOptions: Array<[string, string]> = workspace.models.map(model => [model,
    configured.has(model.toLowerCase()) ? `${model} · ${t('独立配置')}` : `${model} · ${t('继承全局')}`])
  const ownModels = [...new Set([...workspace.configuredModels, ...(scope ? [scope] : [])].filter(Boolean))]
  const scopes = [{ id: '', label: t('全局默认'), note: t('供继承全局的模型使用') },
    ...ownModels
      .filter((model, index, list) => list.findIndex(other => other.toLowerCase() === model.toLowerCase()) === index)
      .map(model => ({
        id: model,
        label: model,
        note: workspace.configuredModels.some(known => known.toLowerCase() === model.toLowerCase())
          ? t('模型独立配置')
          : scope && model.toLowerCase() === scope.toLowerCase() && inherited ? t('继承全局')
            : t('新配置 · 保存后生效'),
      }))]
  const allProviders = [...new Set([...profile.providers, ...workspace.providers.map(provider => provider.id)])]
  const providers = allProviders.filter(provider => !query || label(provider).toLowerCase().includes(query)
    || workspace.accounts.some(account => providerOf(account) === provider && displayNameOf(account).toLowerCase().includes(query)))
  const providerLive = (provider: string) => {
    const on = (profile.providerScope === 'all' || profile.providers.includes(provider)) && !profile.disabledProviders.includes(provider)
    return on && workspace.accounts.some(account => providerOf(account) === provider && account.enabled !== false)
  }
  const hasAccounts = (provider: string) => workspace.accounts.some(account => providerOf(account) === provider)
  // 没有账号的提供商不参与选路，默认收起；搜索时仍然展开匹配项，避免「搜不到」。
  const emptyProviders = providers.filter(provider => !hasAccounts(provider))
  const providerRank = (provider: string) => providerLive(provider) ? 0 : hasAccounts(provider) ? 1 : 2
  const listedProviders = providers
    .filter(provider => showEmpty || Boolean(query) || hasAccounts(provider))
    .map((provider, index) => ({ provider, index }))
    // 能参与选路的排前面；全部账号禁用或没账号的排到最后，组内仍按原顺位。
    .sort((left, right) => providerRank(left.provider) - providerRank(right.provider) || left.index - right.index)
    .map(item => item.provider)
  const liveProviders = allProviders.filter(providerLive)
  const toggleOpen = (provider: string) => setOpened(current => {
    const next = new Set(current)
    if (next.has(provider)) next.delete(provider)
    else next.add(provider)
    return next
  })
  // 搜索时自动展开命中的分组，但保留手动展开/收起的结果：清空搜索后恢复原状态。
  const applySearch = (value: string) => {
    setSearch(value)
    const nextQuery = value.trim().toLowerCase()
    setOpened(current => {
      const next = new Set(current)
      for (const provider of autoOpened.current) next.delete(provider)
      const matches = nextQuery ? allProviders.filter(provider => label(provider).toLowerCase().includes(nextQuery)
        || workspace.accounts.some(account => providerOf(account) === provider && displayNameOf(account).toLowerCase().includes(nextQuery))) : []
      autoOpened.current = new Set(matches)
      for (const provider of matches) next.add(provider)
      return next
    })
  }
  const removeIndependent = async () => {
    if (!scope || disabled) return
    if (!await confirmDialog(t('删除后这个模型会重新跟随全局配置，已保存的独立设置和当前未保存修改都会被移除。'), {
      title: t('删除独立配置'), confirmText: t('删除'), destructive: true,
    })) return
    await editor.remove()
  }
  const participants = workspace.accounts.filter(account => {
    const provider = providerOf(account)
    return account.enabled !== false && (profile.providerScope === 'all' || profile.providers.includes(provider))
      && !profile.disabledProviders.includes(provider) && !profile.excludedAccounts.includes(account.id)
  })

  const ensure = (next: RouteProfile, provider: string) => {
    if (!next.providers.includes(provider)) next.providers.push(provider)
    if (!next.accountOrder[provider]) next.accountOrder[provider] = workspace.accounts.filter(account => providerOf(account) === provider).map(account => account.id)
  }
  const toggleProvider = (provider: string, on: boolean) => mutate(next => {
    ensure(next, provider)
    if (next.providerScope === 'all') next.disabledProviders = on ? next.disabledProviders.filter(id => id !== provider) : [...new Set([...next.disabledProviders, provider])]
    else next.providers = on ? [...new Set([...next.providers, provider])] : next.providers.filter(id => id !== provider)
  })
  const toggleAccount = (id: string, on: boolean) => mutate(next => {
    next.excludedAccounts = on ? next.excludedAccounts.filter(value => value !== id) : [...new Set([...next.excludedAccounts, id])]
  })
  const moveAccounts = (provider: string, id: string, direction: number) => mutate(next => {
    ensure(next, provider)
    const values = next.accountOrder[provider]
    const index = values.indexOf(id), to = index + direction
    if (index >= 0 && to >= 0 && to < values.length) [values[index], values[to]] = [values[to], values[index]]
  })
  const moveProvider = (id: string, direction: number) => mutate(next => {
    ensure(next, id)
    const live = (provider: string) => {
      const on = (next.providerScope === 'all' || next.providers.includes(provider)) && !next.disabledProviders.includes(provider)
      return on && workspace.accounts.some(account => providerOf(account) === provider && account.enabled !== false)
    }
    const peers = next.providers.filter(provider => live(provider) === live(id))
    const index = peers.indexOf(id)
    const target = peers[index + direction]
    if (!target) return
    const from = next.providers.indexOf(id), to = next.providers.indexOf(target)
    if (from < 0 || to < 0) return
    next.providers.splice(from, 1)
    next.providers.splice(to, 0, id)
  })

  return <div className='routing-config-shell'>
    <aside className='routing-config-rail'>
      <div className='routing-config-rail-head'><RouteIcon name='route' size={13} />{t('作用范围')}</div>
      <div className='routing-config-rail-list'>
        {scopes.map(item => {
          const active = item.id.toLowerCase() === scope.toLowerCase()
          return <Button key={item.id || '__global'} variant='ghost' className={cn('routing-config-scope-item', active && 'is-selected')}
            title={item.note} onClick={() => { if (!active && !editor.busy) onScope(item.id) }}>
            <span className='top'>{item.id ? <code>{item.label}</code> : <b>{item.label}</b>}</span>
            <small>{item.note}</small>
          </Button>
        })}
      </div>
      <div className='routing-config-rail-foot'>
        <Button variant='outline' className='w-full' onClick={onAddModel}><RouteIcon name='plus' size={13} />{t('新增模型独立配置')}</Button>
      </div>
      <p className='routing-config-rail-note'>{t('全局默认供继承的模型使用；给某个模型单独配置后，它就不再跟随全局。')}</p>
    </aside>

    <section className='routing-config-main'>
      <header className='routing-config-head'>
        <b title={scopeLabel}>{scopeLabel}</b>
        {scope ? null : <Badge shape='tag' variant='secondary'>{t('全局')}</Badge>}
        {scope ? (workspace.customized
          ? <Button variant='outline' size='sm' className='routing-config-delete' disabled={disabled}
            onClick={() => void removeIndependent()}>{t('删除独立配置')}</Button>
          : <Badge shape='tag' variant='secondary'>{t('继承全局')}</Badge>)
          : <span className='sub'>{t('修改后影响继承全局的模型')}</span>}
        <div className='right'>
          {scope ? <span className='sub'>{t('正在预览 {model}', { model: scope })}</span>
            : <label className='routing-config-preview'>
              <span>{t('预览模型')}</span>
              <RoutingSelect label={t('预览模型')} value={previewModel || workspace.models[0] || ''} disabled={!workspace.models.length}
                options={previewOptions.length ? previewOptions : [['', t('没有可用模型')]]} onChange={setPreviewModel} />
            </label>}
          <span className='sub'>{t('{providers} 个提供商 · {accounts} 个参与账号', { providers: profile.providers.length, accounts: participants.length })}</span>
        </div>
      </header>

      <div className='routing-config-rules'>
        <div className='field'><Label>{t('路由策略')}</Label>
          <RoutingSelect label={t('路由策略')} value={profile.strategy || '__inherit'} disabled={locked}
            options={[...(scope ? [['__inherit', t('继承全局：{value}', { value: strategyLabel(workspace.globalSettings?.routingStrategy || workspace.settings.routingStrategy) })] as [string, string]] : []),
              ['order', t('顺序优先')], ['rotate', t('负载均衡')], ['provider', t('家内轮询')]]}
            onChange={value => mutate(next => { next.strategy = value === '__inherit' ? undefined : value })} />
        </div>
        <div className='field'><Label>{t('会话粘滞')}</Label>
          <RoutingSelect label={t('会话粘滞')} value={profile.sticky || '__inherit'} disabled={locked}
            options={[...(scope ? [['__inherit', t('继承全局：{value}', { value: stickyLabel(workspace.globalSettings?.routingSticky || workspace.settings.routingSticky) })] as [string, string]] : []),
              ['auto', t('自动')], ['session', t('会话内')], ['turn', t('每轮')], ['off', t('关闭')]]}
            onChange={value => mutate(next => { next.sticky = value === '__inherit' ? undefined : value })} />
        </div>
        <div className='field'><Label>{t('参与方式')}</Label>
          <RoutingSelect label={t('参与方式')} value={profile.providerScope || 'all'} disabled={locked}
            options={[['all', t('全部自动参与')], ['selected', t('仅勾选的参与')]]}
            onChange={value => mutate(next => {
              next.providerScope = value === 'all' ? 'all' : 'selected'
              next.providers = allProviders
              if (value === 'selected') next.disabledProviders = next.disabledProviders.filter(id => next.providers.includes(id))
            })} />
        </div>
        <div className='right'><Button variant='ghost' size='sm' disabled={editor.busy} onClick={() => onProtection(workspace)}>{t('全局保护设置')}</Button></div>
      </div>

      <div className='routing-config-body'>
        <div className='routing-config-listcol'>
          <p className='routing-config-note'>{profile.providerScope === 'all'
            ? t('所有可用提供商自动参与，新增账号或插件后无需再加路由；明确停用的保持不变。')
            : t('只有勾选的提供商参与；以后新增的提供商需要在这里勾选。')}{' '}
            {t('拖动改顺位、勾选决定参与；这里的位置就是实际选路顺序。')}
          </p>
          <div className='routing-config-listhead'>
            <b>{t('参与顺序')}</b><span>{t('从上到下就是选路顺序')}</span>
            <div className='right'>
              <Input className='routing-config-search' placeholder={t('搜索提供商或账号')} aria-label={t('搜索提供商或账号')} value={search} onChange={event => applySearch(event.currentTarget.value)} />
              {emptyProviders.length ? <Button variant='ghost' size='sm' onClick={() => setShowEmpty(value => !value)}>
                {showEmpty ? t('隐藏无账号 ({n})', { n: emptyProviders.length }) : t('显示无账号 ({n})', { n: emptyProviders.length })}
              </Button> : null}
              <Button variant='ghost' size='sm' onClick={() => setOpened(opened.size ? new Set() : new Set(listedProviders))}>{opened.size ? t('折叠全部') : t('展开全部')}</Button>
            </div>
          </div>

          {!listedProviders.length ? <Empty>
            <span>{query ? t('没有匹配的提供商或账号')
              : workspace.accounts.length ? t('当前范围的提供商都没有账号，先在账号页添加')
                : t('还没有账号，先在账号页添加；有账号的提供商才会出现在这里')}</span>
            {!query && !workspace.accounts.length ? <Button variant='link' size='sm' onClick={() => host().wbApp?.showPage('accounts')}>{t('去账号页添加')}</Button> : null}
          </Empty> : null}
          {listedProviders.map(provider => {
            const accounts = workspace.accounts.filter(account => providerOf(account) === provider)
            const order = [...new Set([...(profile.accountOrder[provider] || []), ...accounts.map(account => account.id)])].filter(id => accounts.some(account => account.id === id))
            const on = (profile.providerScope === 'all' || profile.providers.includes(provider)) && !profile.disabledProviders.includes(provider)
            if (!accounts.length) return <section key={provider} className={cn('routing-config-provider is-empty', !on && 'is-off')}
              data-cfg-provider={provider} data-disabled={!on}>
              <header>
                <Button variant='ghost' size='icon' className='routing-grip' disabled aria-hidden='true'><RouteIcon name='gripVertical' size={14} /></Button>
                <Checkbox checked={on} disabled={locked} aria-label={label(provider)} onCheckedChange={value => toggleProvider(provider, value === true)} />
                <span className='routing-config-rank'>—</span>
                <span className='routing-provider-mark routing-letter'>{label(provider).slice(0, 1).toUpperCase()}</span>
                <span className='routing-provider-name'><b>{label(provider)}</b><span>{t('还没有账号')}</span></span>
                {!on ? <Badge shape='tag' variant='secondary'>{t('未参与')}</Badge> : null}
              </header>
            </section>
            const rules = !scope ? activeGroupSortRules(provider, workspace.sorting?.rules?.[provider] || []) : []
            const sorted = rules.length > 0
            const live = providerLive(provider)
            const rank = liveProviders.indexOf(provider)
            const peers = allProviders.filter(item => providerLive(item) === live)
            const groupIndex = peers.indexOf(provider)
            const open = opened.has(provider)
            const unavailable = workspace.providers.find(item => item.id === provider)?.available === false
            const target = drag.state?.target === provider
            const enabledCount = accounts.filter(account => account.enabled !== false).length
            return <section key={provider} className={cn('routing-config-provider', !on && 'is-off', !live && 'is-parked',
              drag.state?.id === provider && 'is-dragging', target && (drag.state?.after ? 'drop-after' : 'drop-before'))}
              data-cfg-provider={provider} data-disabled={!on}>
              <header onClick={event => {
                if ((event.target as HTMLElement).closest('button, input, label, a, [role="checkbox"], [role="switch"]')) return
                toggleOpen(provider)
              }}>
                <Button variant='ghost' size='icon' className='routing-grip' disabled={locked || !live} title={t('拖动调整提供商顺序')} aria-label={t('拖动调整提供商顺序')}
                  onPointerDown={event => drag.start(event, 'provider', provider, provider)}><RouteIcon name='gripVertical' size={14} /></Button>
                <Checkbox checked={on} disabled={locked} aria-label={label(provider)} onCheckedChange={value => toggleProvider(provider, value === true)} />
                <span className='routing-config-rank'>{live ? rank + 1 : '—'}</span>
                <span className='routing-provider-mark routing-letter'>{label(provider).slice(0, 1).toUpperCase()}</span>
                <span className='routing-provider-name'><b>{label(provider)}</b>
                  <span>{accounts.length
                    ? `${t('{n} 个账号', { n: accounts.length })} · ${live ? t('第 {n} 顺位', { n: rank + 1 })
                      : on ? t('暂无可用账号') : unavailable ? t('模型绑定不可用') : t('未参与本范围')}`
                    : t('还没有账号')}</span></span>
                {!live && accounts.length ? <Badge shape='tag' variant='secondary'>{enabledCount ? t('未参与') : t('账号全部禁用')}</Badge> : null}
                {sorted ? <Badge shape='tag' variant='secondary' title={manualOrderBlockedMessage(rules)}>{groupSortSummary(rules)}</Badge> : null}
                <IconButton name='arrowUp' title={t('提高优先级')} disabled={locked || !live || groupIndex === 0} onClick={() => moveProvider(provider, -1)} />
                <IconButton name='arrowDown' title={t('降低优先级')} disabled={locked || !live || groupIndex >= peers.length - 1} onClick={() => moveProvider(provider, 1)} />
                <IconButton name={open ? 'chevronDown' : 'caretRight'} title={open ? t('折叠这一组') : t('展开这一组')} aria-expanded={open}
                  onClick={() => toggleOpen(provider)} />
              </header>
              {open ? <div className='routing-config-accounts'>
                {sorted ? <div className='routing-config-lock'><span>{manualOrderBlockedMessage(rules)}</span>
                  <Button variant='link' size='sm' disabled={editor.busy} onClick={() => onSort(provider)}>{t('消耗规则')}</Button></div> : null}
                {order.map((id, index) => {
                  const account = accounts.find(account => account.id === id)
                  if (!account) return null
                  const state = accountFlowState(workspace, account, now)
                  const included = !profile.excludedAccounts.includes(account.id)
                  const rowTarget = drag.state?.target === id
                  return <div key={id} className={cn('routing-config-account', !included && 'is-excluded',
                    drag.state?.id === id && 'is-dragging', rowTarget && (drag.state?.after ? 'drop-after' : 'drop-before'))}
                    data-cfg-account={id} data-provider={provider}>
                    <Button variant='ghost' size='icon' className={cn('routing-grip', sorted && 'is-locked')} disabled={locked || sorted}
                      title={sorted ? manualOrderBlockedMessage(rules) : t('拖动调整账号顺序')} aria-label={t('拖动调整账号顺序')}
                      onPointerDown={event => drag.start(event, 'account', provider, id)}><RouteIcon name='gripVertical' size={14} /></Button>
                    <span className='routing-account-rank'>{index + 1}</span>
                    <Checkbox checked={included} disabled={locked || !on} aria-label={displayNameOf(account)} onCheckedChange={value => toggleAccount(id, value === true)} />
                    <span className='routing-account-avatar'>{displayNameOf(account).slice(0, 1).toUpperCase()}</span>
                    <span className='routing-config-account-name'><b>{displayNameOf(account)}</b>
                      <small>{on ? state.label : t('提供商未参与')}{state.resetAt > now ? ` · ${formatDuration(state.resetAt - now)}` : ''}</small></span>
                    {included && on ? <span className='routing-config-state'>{t('参与')}</span> : <span className='routing-config-state is-off'>{t('不参与')}</span>}
                    <IconButton name='arrowUp' title={t('提高优先级')} disabled={locked || sorted || index === 0} onClick={() => moveAccounts(provider, id, -1)} />
                    <IconButton name='arrowDown' title={t('降低优先级')} disabled={locked || sorted || index >= order.length - 1} onClick={() => moveAccounts(provider, id, 1)} />
                    <IconButton name='settings' title={t('账号设置')} disabled={editor.busy} onClick={() => onAccount(id)} />
                  </div>
                })}
                {!order.length ? <p className='routing-config-note'>{t('该提供商没有账号，请先到账号页添加。')}</p> : null}
              </div> : <p className='routing-config-collapsed' onClick={() => toggleOpen(provider)}>{t('{n} 个账号 · {on} 启用 · {off} 禁用', { n: accounts.length, on: enabledCount, off: accounts.length - enabledCount })}</p>}
            </section>
          })}
        </div>

        <RoutingLivePanel workspace={preview} model={scope || previewModel || workspace.models[0] || ''} loading={previewLoading}
          error={previewError} dirty={previewDirty} globalScope={!scope} modelsAvailable={workspace.models.length > 0}
          onOpenScope={onScope} onRefresh={() => setPreviewToken(value => value + 1)} />
      </div>

      <div className={cn('routing-config-draft', dirty && 'is-dirty')}>
        <b>{scopeLabel}</b>
        <Badge shape='tag' variant={dirty ? 'warning' : 'secondary'}>
          {dirty ? t('{n} 处未保存', { n: changes.length || 1 }) : inherited && scope ? t('继承全局') : t('已保存')}
        </Badge>
        <div className='routing-config-draft-list'>
          {changes.slice(0, 4).map((change, index) => <span key={index}>· {change}</span>)}
          {changes.length > 4 ? <span>· {t('还有 {n} 处', { n: changes.length - 4 })}</span> : null}
        </div>
        <div className='right'>
          {editor.error ? <span className='routing-config-error' role='alert'>{editor.error}</span> : null}
          <span className='routing-config-savehint'>{t('保存后只影响后续请求')}</span>
          <Button variant='outline' size='sm' disabled={!dirty || disabled} onClick={editor.discard}>{t('撤销草稿')}</Button>
          <Button size='sm' disabled={!dirty || disabled} onClick={() => void editor.save()}>{editor.busy ? t('保存中…') : scope ? t('保存模型路由') : t('保存全局路由')}</Button>
        </div>
      </div>
    </section>
  </div>
}

/** 「改了几处」的文字摘要 —— 草稿栏要说清保存会发生什么，而不是只给一个数字。 */
function describeDraft(draft: RoutingDraft, label: (provider: string) => string) {
  if (draft.mode === 'inherit') return []
  if (!draft.baseline) return [t('把全局配置复制成这个模型的独立配置')]
  const next = draft.profile, base = draft.baseline
  const out: string[] = []
  if (next.providerScope !== base.providerScope) out.push(t('参与方式改为{value}', { value: (next.providerScope || 'all') === 'all' ? t('全部自动参与') : t('仅勾选的参与') }))
  if (next.strategy !== base.strategy) out.push(t('路由策略改为{value}', { value: next.strategy ? strategyLabel(next.strategy) : t('继承全局') }))
  if (next.sticky !== base.sticky) out.push(t('会话粘滞改为{value}', { value: next.sticky ? stickyLabel(next.sticky) : t('继承全局') }))
  for (const provider of new Set([...next.disabledProviders, ...base.disabledProviders])) {
    if (next.disabledProviders.includes(provider) !== base.disabledProviders.includes(provider)) {
      out.push(t('{name} 已{state}', { name: label(provider), state: next.disabledProviders.includes(provider) ? t('停用') : t('启用') }))
    }
  }
  for (const provider of new Set([...next.providers, ...base.providers])) {
    if (next.providers.includes(provider) !== base.providers.includes(provider)
      && !next.disabledProviders.includes(provider) && !base.disabledProviders.includes(provider)) {
      out.push(t('{name} 已{state}', { name: label(provider), state: next.providers.includes(provider) ? t('加入') : t('移出') }))
    }
  }
  for (const id of new Set([...next.excludedAccounts, ...base.excludedAccounts])) {
    if (next.excludedAccounts.includes(id) !== base.excludedAccounts.includes(id)) {
      out.push(t('账号 {name} 已{state}', { name: id.slice(0, 8), state: next.excludedAccounts.includes(id) ? t('排除') : t('恢复') }))
    }
  }
  for (const provider of new Set([...Object.keys(next.accountOrder), ...Object.keys(base.accountOrder)])) {
    if (JSON.stringify(next.accountOrder[provider] || []) !== JSON.stringify(base.accountOrder[provider] || [])) out.push(t('{name} 的账号顺序已调整', { name: label(provider) }))
  }
  return out
}

/** 草稿内的拖动排序：松手写进草稿，不直接保存。 */
function useDraftDrag(enabled: boolean, onDrop: (payload: { kind: 'provider' | 'account'; provider: string; id: string; target: string; after: boolean }) => void) {
  const ref = React.useRef<{ kind: 'provider' | 'account'; provider: string; id: string; x: number; y: number; active: boolean; target?: string; after?: boolean } | null>(null)
  const [state, setState] = React.useState<{ id: string; target?: string; after?: boolean } | null>(null)
  const live = React.useRef({ enabled, onDrop })
  live.current = { enabled, onDrop }
  React.useEffect(() => {
    const move = (event: PointerEvent) => {
      const drag = ref.current
      if (!drag || !live.current.enabled) return
      if (!drag.active && Math.hypot(event.clientX - drag.x, event.clientY - drag.y) < 5) return
      drag.active = true
      event.preventDefault()
      const hit = document.elementFromPoint(event.clientX, event.clientY)
      const row = hit?.closest<HTMLElement>(drag.kind === 'account' ? '[data-cfg-account]' : '[data-cfg-provider]')
      const id = drag.kind === 'account' ? row?.dataset.cfgAccount : row?.dataset.cfgProvider
      if (!row || !id || id === drag.id || (drag.kind === 'account' && row.dataset.provider !== drag.provider)) {
        drag.target = undefined
        setState({ id: drag.id })
        return
      }
      const bounds = row.getBoundingClientRect()
      drag.target = id
      drag.after = event.clientY > bounds.top + bounds.height / 2
      setState({ id: drag.id, target: id, after: drag.after })
    }
    const end = () => {
      const drag = ref.current
      if (!drag) return
      ref.current = null
      setState(null)
      if (!drag.active || !drag.target) return
      live.current.onDrop({ kind: drag.kind, provider: drag.provider, id: drag.id, target: drag.target, after: Boolean(drag.after) })
    }
    window.addEventListener('pointermove', move, { passive: false })
    window.addEventListener('pointerup', end)
    window.addEventListener('pointercancel', end)
    return () => { window.removeEventListener('pointermove', move); window.removeEventListener('pointerup', end); window.removeEventListener('pointercancel', end) }
  }, [])
  const start = (event: React.PointerEvent, kind: 'provider' | 'account', provider: string, id: string) => {
    if (!live.current.enabled || event.button !== 0) return
    event.preventDefault()
    ref.current = { kind, provider, id, x: event.clientX, y: event.clientY, active: false }
  }
  return { start, state }
}
