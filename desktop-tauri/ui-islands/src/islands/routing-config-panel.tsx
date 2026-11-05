import * as React from 'react'
import { Badge, Button, Collapsible, CollapsibleContent, CollapsibleTrigger, Empty, InputGroup, InputGroupAddon, InputGroupInput, Label, Spinner, confirmDialog } from '@ui'
import { t } from '../i18n'
import { displayNameOf, providerOf } from './accounts-domain'
import { RoutingDragOverlay, reorderRoute, useRoutingDrag, type RoutingDrop } from './routing-drag'
import { RoutingModelFilter, useRoutingModelFilter } from './routing-model-filter'
import { RoutingSelect } from './routing-select'
import { RoutingActions } from './routing-canvas-controls'
import { RoutingCanvasLayout, RoutingDraftFooter } from './routing-canvas-layout'
import { RoutingProviderCard } from './routing-provider-card'
import { routingAccountSections, routingCollapsedSections, useRoutingExpansion } from './routing-account-sections'
import { RouteIcon, stickyLabel, strategyLabel } from './routing-ui'
import { draftDirty, type RoutingEditorState } from './routing-editor-state'
import { host, type RouteProfile, type RoutingWorkspace } from './routing-types'

export type RoutingConfigProps = {
  editor: RoutingEditorState; scope: string; now: number
  onScope(model: string): void; onAccount(id: string): void; onSort(provider: string): void; onProtection(workspace: RoutingWorkspace): void
}

/** 参与关系与账号状态分开；这里只修改草稿，保存后才影响后续选路。 */
export function RoutingConfigPanel({ editor, scope, now, onScope, onAccount, onSort, onProtection }: RoutingConfigProps) {
  const { setOpened, sectionOpen, setSectionOpen, search, setSearch, query, isOpen } = useRoutingExpansion()
  const draft = editor.draft
  const modelFilter = useRoutingModelFilter(draft?.workspace, scope)
  const disabled = editor.busy || editor.loading
  const mutate = React.useCallback((fn: (profile: RouteProfile, workspace: RoutingWorkspace) => void) => {
    editor.update(current => { const profile = structuredClone(current.profile); fn(profile, current.workspace); return { ...current, profile } })
  }, [editor])
  const drop = React.useCallback(({ kind, provider, id, target, after }: RoutingDrop) => mutate((profile, workspace) => {
    if (kind === 'provider' && profile.providerScope === 'all') {
      for (const item of workspace.providers) if (!profile.providers.includes(item.id)) profile.providers.push(item.id)
    }
    if (kind === 'account') {
      profile.accountOrder[provider] = [...new Set([...(profile.accountOrder[provider] || []), ...workspace.accounts.filter(account => providerOf(account) === provider).map(account => account.id)])]
    }
    reorderRoute(kind === 'provider' ? profile.providers : profile.accountOrder[provider] || [], id, target, after)
  }), [mutate])
  const drag = useRoutingDrag(!disabled && !modelFilter.loading && draft?.mode !== 'inherit' && !search.trim(), `${scope}:${modelFilter.model}:${search}`, drop)
  if (!draft) return <div className='routing-config-loading'>
    <Empty>{editor.loading ? <Spinner /> : editor.error || t('选择作用范围后配置路由')}</Empty>
  </div>
  const workspace = draft.workspace
  const inherited = draft.mode === 'inherit'
  const profile = inherited ? workspace.inheritedProfile || draft.profile : draft.profile
  const locked = disabled || inherited
  const dirty = draftDirty(draft)
  const label = (id: string) => workspace.providers.find(item => item.id === id)?.label || id
  const allProviders = [...new Set([...profile.providers, ...workspace.providers.map(provider => provider.id)])]
  const included = (id: string) => (profile.providerScope === 'all' || profile.providers.includes(id)) && !profile.disabledProviders.includes(id)
  const participating = allProviders.filter(included)
  const excluded = allProviders.filter(id => !included(id))
  const participants = workspace.accounts.filter(account => included(providerOf(account)) && !profile.excludedAccounts.includes(account.id))
  const accountProviders = new Set(workspace.accounts.map(providerOf))
  const displayedProviders = allProviders.filter(id => accountProviders.has(id) && (!modelFilter.providers || modelFilter.providers.has(id)))
  const displayedParticipating = displayedProviders.filter(included)
  const sections = routingAccountSections(displayedProviders, workspace.accounts, id => id, included)
  const orderingPeers = (id: string) => (Object.values(sections).find(ids => ids.includes(id)) || []).filter(included)
  const matches = (id: string) => !query || label(id).toLowerCase().includes(query)
    || workspace.accounts.some(account => providerOf(account) === id && displayNameOf(account).toLowerCase().includes(query))
  const visible = sections.active.filter(matches)
  const expandable = [...sections.active, ...routingCollapsedSections.flatMap(({ key }) => sectionOpen[key] ? sections[key] : [])].filter(matches)
  const allOpen = expandable.length > 0 && expandable.every(isOpen)
  const ensure = (next: RouteProfile, id: string) => {
    if (!next.providers.includes(id)) next.providers.push(id)
    next.accountOrder[id] = [...new Set([...(next.accountOrder[id] || []), ...workspace.accounts.filter(account => providerOf(account) === id).map(account => account.id)])]
  }
  const join = (id: string, on: boolean) => mutate(next => {
    ensure(next, id)
    next.disabledProviders = on ? next.disabledProviders.filter(value => value !== id) : [...new Set([...next.disabledProviders, id])]
  })
  const moveProvider = (id: string, direction: number) => mutate(next => {
    participating.forEach(provider => ensure(next, provider))
    const peers = orderingPeers(id), target = peers[peers.indexOf(id) + direction]
    if (target) reorderRoute(next.providers, id, target, direction > 0)
  })
  const moveAccount = (provider: string, id: string, direction: number) => mutate(next => {
    ensure(next, provider)
    const order = next.accountOrder[provider].filter(key => profile.excludedAccounts.includes(key) === profile.excludedAccounts.includes(id))
    const target = order[order.indexOf(id) + direction]
    if (target) reorderRoute(next.accountOrder[provider], id, target, direction > 0)
  })
  const removeAll = async () => {
    if (!await confirmDialog(t('只移出本路由，账号状态和其他路由不变；保存后此入口将没有可用路由。'), {
      title: t('全部移出本路由'), confirmText: t('全部移出'),
    })) return
    mutate(next => { next.providerScope = 'selected'; next.providers = allProviders; next.disabledProviders = [...allProviders] })
  }
  const save = async () => {
    if (!participants.length && !await confirmDialog(t('此入口没有参与路由的账号。保存后请求将明确失败，不会回退全局或自动恢复参与名单。'), {
      title: t('保存空路由'), confirmText: t('仍然保存'), destructive: true,
    })) return
    await editor.save()
  }
  const removeIndependent = async () => {
    if (!await confirmDialog(t('删除后这个模型会重新跟随全局配置，已保存的独立设置和当前未保存修改都会被移除。'), {
      title: t('删除独立配置'), confirmText: t('删除'), destructive: true,
    })) return
    if (await editor.remove()) onScope('')
  }
  const renderProvider = (id: string) => <RoutingProviderCard key={id} id={id} profile={profile} workspace={workspace} scope={scope} now={now}
    index={participating.indexOf(id)} first={orderingPeers(id)[0] === id} last={orderingPeers(id).at(-1) === id}
    included={included(id)} open={isOpen(id)} locked={locked} busy={editor.busy} query={query} drag={drag}
    onOpen={open => setOpened(current => ({ ...current, [id]: open }))} onJoin={on => join(id, on)} onMove={direction => moveProvider(id, direction)}
    onMoveAccount={(key, direction) => moveAccount(id, key, direction)} onToggleAccount={key => mutate(next => {
      next.excludedAccounts = next.excludedAccounts.includes(key) ? next.excludedAccounts.filter(value => value !== key) : [...next.excludedAccounts, key]
    })} onAccount={onAccount} onSort={() => onSort(id)} />
  const groups = <section className='routing-groups-column'>
    <div className='routing-column-head'><RouteIcon name='accounts' size={14} /><b>{t('账号组')}</b><span>{t('{n} 项参与', { n: displayedParticipating.length })}</span>
      <div className='right'><Button variant='ghost' size='sm' disabled={!expandable.length || Boolean(query)} onClick={() => setOpened(current => ({ ...current, ...Object.fromEntries(expandable.map(id => [id, !allOpen])) }))}>
        {allOpen ? t('折叠全部') : t('展开全部')}</Button>
        <RoutingActions label={t('路由参与操作')} actions={[{ label: t('全部移出'), disabled: locked || !participating.length, onClick: () => void removeAll() }]} />
      </div>
    </div>
    <InputGroup className='routing-config-search'><InputGroupInput value={search} placeholder={t('搜索提供商或账号')} aria-label={t('搜索提供商或账号')}
      onChange={event => setSearch(event.currentTarget.value)} /><InputGroupAddon><RouteIcon name='search' size={14} /></InputGroupAddon></InputGroup>
    <p className='routing-groups-hint'>{t('拖动把手或按 Alt + 方向键调整顺序，更多操作在右侧菜单。')}</p>
    <div className='routing-config-listcol' data-route-scroll aria-busy={modelFilter.loading}>
      {modelFilter.loading ? <Empty><Spinner />{t('加载中…')}</Empty> : modelFilter.error ? <Empty>
        <p className='routing-config-error' role='alert'>{modelFilter.error}</p><Button variant='outline' size='sm' onClick={modelFilter.retry}>{t('重试')}</Button>
      </Empty> : <>
        <div className='routing-config-participants' data-route-list>{visible.map(renderProvider)}</div>
        {!participating.length && displayedProviders.length ? <Empty>{t('此入口没有参与路由的提供商，保存后请求将明确失败。')}</Empty> : null}
        {routingCollapsedSections.map(({ key, label }) => {
          const ids = sections[key].filter(matches)
          return ids.length ? <Collapsible key={key} open={Boolean(sectionOpen[key]) || Boolean(query)}
            onOpenChange={open => setSectionOpen(current => ({ ...current, [key]: open }))} className='routing-config-excluded'>
            <CollapsibleTrigger className='routing-excluded-trigger'><RouteIcon name='chevronDown' size={13} />{label} · {ids.length}</CollapsibleTrigger>
            <CollapsibleContent data-route-list>{ids.map(renderProvider)}</CollapsibleContent>
          </Collapsible> : null
        })}
        {query && displayedProviders.length && !displayedProviders.some(matches) ? <Empty>{t('没有匹配的提供商或账号')}</Empty> : null}
        {!displayedProviders.length ? <Empty><span>{modelFilter.model ? t('当前模型没有对应的账号，请先添加账号或检查模型绑定。') : t('还没有账号，先在账号页添加；有账号的提供商才会出现在这里')}</span>
          <Button variant='link' onClick={() => host().wbApp?.showPage('accounts')}>{t('去账号页添加')}</Button></Empty> : null}
      </>}
    </div>
  </section>
  return <>
    <RoutingCanvasLayout title={scope ? t('模型路由') : t('全局默认')} groups={groups}
      note={scope ? t('仅影响此路由的后续请求') : t('影响继承全局的模型')}>
      <div className='routing-config-card' data-route-source>
        <div className='routing-config-card-scroll'>
          <header className='routing-config-head'><div><span className='routing-config-eyebrow'>{inherited ? t('继承全局') : scope ? t('独立配置') : t('默认规则')}</span><h2>{scope || t('全局默认')}</h2></div>
            <RoutingActions label={t('路由配置操作')} actions={[
              { label: t('全局保护设置'), icon: 'shield', disabled: editor.busy, onClick: () => onProtection(workspace) },
              ...(scope && workspace.customized ? [{ label: t('删除独立配置'), destructive: true, disabled, onClick: () => void removeIndependent() }] : []),
            ]} />
          </header>
          {inherited ? <div className='routing-inherited-note'><p>{t('没有独立配置的模型沿用全局路由。')}</p><Button variant='outline' disabled={disabled} onClick={() => void editor.create(scope)}>{t('创建独立配置')}</Button></div> : null}
          <div className='routing-config-rules'>
            <div className='routing-rule-field'><Label>{t('路由策略')}</Label><RoutingSelect label={t('路由策略')} value={profile.strategy || '__inherit'} disabled={locked}
              options={[...(scope ? [['__inherit', t('继承全局：{value}', { value: strategyLabel(workspace.globalSettings?.routingStrategy) })] as [string, string]] : []),
                ['order', t('顺序优先')], ['rotate', t('负载均衡')], ['provider', t('家内轮询')]]}
              onChange={value => mutate(next => { next.strategy = value === '__inherit' ? undefined : value })} /></div>
            <div className='routing-rule-field'><Label>{t('会话粘滞')}</Label><RoutingSelect label={t('会话粘滞')} value={profile.sticky || '__inherit'} disabled={locked}
              options={[...(scope ? [['__inherit', t('继承全局：{value}', { value: stickyLabel(workspace.globalSettings?.routingSticky) })] as [string, string]] : []),
                ['auto', t('自动')], ['session', t('会话内')], ['turn', t('每轮')], ['off', t('关闭')]]}
              onChange={value => mutate(next => { next.sticky = value === '__inherit' ? undefined : value })} /></div>
            <div className='routing-rule-field routing-rule-wide'><Label>{t('参与方式')}</Label><RoutingSelect label={t('参与方式')} value={profile.providerScope || 'all'} disabled={locked}
              options={[['all', t('全部自动参与')], ['selected', t('仅加入的参与')]]} onChange={value => mutate(next => {
                next.providerScope = value === 'all' ? 'all' : 'selected'; next.providers = allProviders; next.disabledProviders = excluded
              })} /></div>
          </div>
          <p className='routing-config-note'>{profile.providerScope === 'all' ? t('新增提供商自动参与；移出本路由的不会自动加回。') : t('只有加入的提供商参与，新增提供商需要手动加入。')}</p>
          {!scope ? <RoutingModelFilter value={modelFilter.model} models={workspace.models} disabled={disabled} onChange={modelFilter.setModel} /> : null}
          <div className='routing-protection-summary'><RouteIcon name='shield' size={14} /><span>{t('全局保护设置')}</span>
            <Badge variant='ghost' shape='tag'>{[workspace.settings.routingFailoverBackoff && t('失败退避'), workspace.settings.routingSink && t('限流垫底'), workspace.settings.routingBreakerEnabled && t('熔断保护')].filter(Boolean).join(' · ') || t('关闭')}</Badge>
            <Button variant='link' size='sm' disabled={editor.busy} onClick={() => onProtection(workspace)}>{t('设置')}</Button>
          </div>
        </div>
        <RoutingDraftFooter dirty={dirty} inherited={inherited} busy={editor.busy} disabled={disabled} error={editor.error} onDiscard={editor.discard} onSave={() => void save()} />
      </div>
    </RoutingCanvasLayout>
    <RoutingDragOverlay state={drag.state} />
  </>
}
