import * as React from 'react'
import { Badge, Button, Collapsible, CollapsibleContent, CollapsibleTrigger, Empty, Input, cn } from '@ui'
import { t } from '../i18n'
import { displayNameOf, providerOf } from './accounts-domain'
import { inactiveProvider, orderedProviders } from './routing-display'
import { PROVIDER_ICONS } from './add-provider-pick'
import { groupSortSummary, manualOrderBlockedMessage } from './accounts-group-settings'
import {
  accountOrder, accountOrderLocked, accountOrderRules, cloneProfile, moveAccount, moveProvider, patch, saveProfile,
  setAllProvidersExpanded, setProviderExpanded, snapshot,
} from './routing-state'
import { formatDuration, IconButton, ProviderRouteBadge, RouteIcon, RouteStateBadge, stickyLabel, strategyLabel } from './routing-ui'
import { host, notify, type RouteEntry, type RoutingWorkspace } from './routing-types'

function ProviderMark({ id, label }: { id: string; label: string }) {
  return PROVIDER_ICONS[id]
    ? <img className='routing-provider-mark' src={PROVIDER_ICONS[id]} alt='' />
    : <span className='routing-provider-mark routing-letter'>{label.slice(0, 1).toUpperCase()}</span>
}

function Connections({ canvas, gateway, runningProviders }: {
  canvas: React.RefObject<HTMLDivElement | null>
  gateway: React.RefObject<HTMLDivElement | null>
  runningProviders: ReadonlySet<string>
}) {
  const [paths, setPaths] = React.useState<Array<{ id: string; d: string; off: boolean }>>([])
  const [size, setSize] = React.useState({ width: 0, height: 0 })
  React.useLayoutEffect(() => {
    const container = canvas.current
    const node = gateway.current
    const list = container?.querySelector<HTMLElement>('.routing-provider-list')
    if (!container || !node || !list) return
    let frame = 0
    const draw = () => {
      const bounds = container.getBoundingClientRect()
      const listBounds = list.getBoundingClientRect()
      const zoom = snapshot().zoom
      const rect = node.getBoundingClientRect()
      const x1 = (rect.right - bounds.left) / zoom
      const y1 = (rect.top + rect.height / 2 - bounds.top) / zoom
      const next = Array.from(list.querySelectorAll<HTMLElement>('[data-routing-provider][data-routing-connect="true"]')).flatMap(card => {
        const header = card.querySelector('header')?.getBoundingClientRect() || card.getBoundingClientRect()
        const center = header.top + header.height / 2
        if (center < listBounds.top || center > listBounds.bottom) return []
        const x2 = (header.left - bounds.left) / zoom
        const y2 = (center - bounds.top) / zoom
        const mid = x1 + (x2 - x1) * 0.52
        return [{ id: card.dataset.routingProvider || '', off: card.dataset.disabled === 'true', d: `M ${x1} ${y1} C ${mid} ${y1}, ${mid} ${y2}, ${x2} ${y2}` }]
      })
      setPaths(next)
      setSize({ width: container.offsetWidth, height: container.offsetHeight })
    }
    const schedule = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(draw) }
    const observer = new ResizeObserver(schedule)
    observer.observe(container)
    observer.observe(node)
    observer.observe(list)
    list.querySelectorAll('[data-routing-provider]').forEach(card => observer.observe(card))
    const mutation = new MutationObserver(schedule)
    mutation.observe(list, { childList: true, subtree: true })
    list.addEventListener('scroll', schedule, { passive: true })
    schedule()
    window.addEventListener('resize', schedule)
    return () => { observer.disconnect(); mutation.disconnect(); cancelAnimationFrame(frame); list.removeEventListener('scroll', schedule); window.removeEventListener('resize', schedule) }
  }, [canvas, gateway, snapshot().workspace, snapshot().search, snapshot().zoom])
  return <svg className='routing-edges' width={size.width} height={size.height} aria-hidden='true'>
    {paths.map(path => <path key={path.id} d={path.d} className={cn('routing-edge', path.off && 'is-off', !path.off && runningProviders.has(path.id) && 'is-running')} />)}
  </svg>
}

type Drag = { kind: 'account' | 'provider'; provider: string; id: string; disabled: boolean; pointerId: number; x: number; y: number; active: boolean; target?: string; after?: boolean }
function useRoutingDrag() {
  const ref = React.useRef<Drag | null>(null)
  const [dragging, setDragging] = React.useState('')
  const [drop, setDrop] = React.useState<{ id: string; after: boolean } | null>(null)
  React.useEffect(() => {
    const move = (event: PointerEvent) => {
      const drag = ref.current
      if (!drag || drag.pointerId !== event.pointerId) return
      if (!drag.active && Math.hypot(event.clientX - drag.x, event.clientY - drag.y) < 5) return
      drag.active = true
      setDragging(drag.id)
      event.preventDefault()
      const hit = document.elementFromPoint(event.clientX, event.clientY)
      const target = hit?.closest<HTMLElement>(drag.kind === 'account' ? '[data-routing-account]' : '[data-routing-provider]')
      const id = drag.kind === 'account' ? target?.dataset.routingAccount : target?.dataset.routingProvider
      if (!target || !id || id === drag.id || (drag.kind === 'account' && (target.dataset.provider !== drag.provider || target.dataset.accountDisabled !== String(drag.disabled)))
        || (drag.kind === 'provider' && target.dataset.routingConnect !== String(!drag.disabled))) {
        drag.target = undefined
        setDrop(null)
        return
      }
      const bounds = target.getBoundingClientRect()
      drag.target = id
      drag.after = event.clientY > bounds.top + bounds.height / 2
      setDrop({ id, after: drag.after })
    }
    const end = (event: PointerEvent) => {
      const drag = ref.current
      if (!drag || event.pointerId !== drag.pointerId) return
      ref.current = null
      setDragging('')
      setDrop(null)
      if (event.type === 'pointercancel' || !drag.active || !drag.target || snapshot().busy) return
      if (drag.kind === 'account' && accountOrderLocked(drag.provider)) {
        notify(manualOrderBlockedMessage(accountOrderRules(drag.provider)), 'err')
        return
      }
      const profile = cloneProfile()
      const ids = drag.kind === 'account' ? profile.accountOrder[drag.provider] : profile.providers
      if (!ids) return
      const from = ids.indexOf(drag.id)
      if (from < 0) return
      ids.splice(from, 1)
      const target = ids.indexOf(drag.target)
      if (target < 0) return
      ids.splice(target + (drag.after ? 1 : 0), 0, drag.id)
      void saveProfile(profile, snapshot().model, drag.kind === 'account'
        ? { kind: 'account', provider: drag.provider } : { kind: 'provider' })
    }
    window.addEventListener('pointermove', move, { passive: false })
    window.addEventListener('pointerup', end)
    window.addEventListener('pointercancel', end)
    return () => { window.removeEventListener('pointermove', move); window.removeEventListener('pointerup', end); window.removeEventListener('pointercancel', end) }
  }, [])
  const start = (event: React.PointerEvent, kind: Drag['kind'], provider: string, id: string) => {
    if (event.button !== 0 || snapshot().busy) return
    if (kind === 'account' && accountOrderLocked(provider)) {
      event.preventDefault()
      notify(manualOrderBlockedMessage(accountOrderRules(provider)), 'err')
      return
    }
    if (snapshot().search.trim()) return
    event.preventDefault()
    event.currentTarget.setPointerCapture(event.pointerId)
    const workspace = snapshot().workspace
    const disabled = kind === 'provider' ? Boolean(workspace && inactiveProvider(workspace, id)) : workspace?.accounts.find(account => account.id === id)?.enabled === false
    ref.current = { kind, provider, id, disabled, pointerId: event.pointerId, x: event.clientX, y: event.clientY, active: false }
  }
  return { start, dragging, drop }
}

export function RoutingCanvas({ workspace, search, zoom, selected, onAccount, onProvider, onSettings, onAddProvider, onSticky, onReplay }: {
  workspace: RoutingWorkspace
  search: string
  zoom: number
  selected: string
  onAccount(id: string): void
  onProvider(id: string): void
  onSettings(): void
  onAddProvider(): void
  onSticky(): void
  onReplay(): void
}) {
  const canvas = React.useRef<HTMLDivElement>(null)
  const gateway = React.useRef<HTMLDivElement>(null)
  const drag = useRoutingDrag()
  const [inactiveExpanded, setInactiveExpanded] = React.useState(false)
  const [disabledAccountsExpanded, setDisabledAccountsExpanded] = React.useState<Set<string>>(new Set())
  const profile = workspace.profile
  const automatic = profile.providerScope === 'all' && (!workspace.model || workspace.customized)
  const addProviderLabel = automatic ? t('恢复提供商') : t('添加提供商')
  const accounts = new Map(workspace.accounts.map(account => [account.id, account]))
  const queue = new Map<string, RouteEntry>([...workspace.queue.ordered, ...workspace.queue.blocked].map(entry => [entry.id, entry]))
  const providerRanks = new Map<string, number>()
  for (const entry of workspace.queue.ordered) {
    if (!profile.providers.includes(entry.provider) || profile.disabledProviders.includes(entry.provider)
      || workspace.providers.find(provider => provider.id === entry.provider)?.available === false) continue
    if (!providerRanks.has(entry.provider)) providerRanks.set(entry.provider, providerRanks.size + 1)
  }
  const query = search.trim().toLocaleLowerCase()
  const groupedProviders = orderedProviders(workspace)
  const activeProviderRanks = new Map(groupedProviders.active.map((provider, index) => [provider, index + 1]))
  const providers = [...groupedProviders.active, ...groupedProviders.inactive].filter(id => {
    if (!query || id.toLocaleLowerCase().includes(query)) return true
    const provider = workspace.providers.find(provider => provider.id === id)
    return provider?.label.toLocaleLowerCase().includes(query) || accountOrder(id).some(id => displayNameOf(accounts.get(id)).toLocaleLowerCase().includes(query))
  })
  const gatewayBase = host().wbPortPanel?.gatewayBase() || `http://127.0.0.1:${workspace.gateway.port}`
  const metrics = workspace.metrics
  const runningProviders = new Set(workspace.accounts.filter(account => (metrics.accountRunning?.[account.id] || 0) > 0).map(providerOf))
  const { busy, expandedProviders } = snapshot()
  const visibleActive = providers.filter(id => !inactiveProvider(workspace, id))
  const visibleInactive = providers.filter(id => inactiveProvider(workspace, id))
  const renderedProviders = providers.filter(id => !inactiveProvider(workspace, id) || inactiveExpanded || Boolean(query))
  const allCollapsed = visibleActive.every(provider => !expandedProviders.has(provider))
  const inactiveToggle = visibleInactive.length ? <Button variant='ghost' className='routing-inactive-toggle' disabled={Boolean(query)} aria-expanded={inactiveExpanded || Boolean(query)} onClick={() => setInactiveExpanded(value => !value)}><RouteIcon name='chevronDown' />{t('已停用提供商 · {n}', { n: visibleInactive.length })}<span>{inactiveExpanded || query ? t('收起') : t('展开')}</span></Button> : null
  return <div className='routing-canvas-viewport'>
    <div className='routing-canvas' ref={canvas} style={{ zoom }}>
      <Connections canvas={canvas} gateway={gateway} runningProviders={runningProviders} />
      <div className='routing-gateway-column'>
        <div className='routing-column-label'><RouteIcon name='traffic' />{t('本地网关')}</div>
        <div className='routing-gateway-node' ref={gateway}>
          <div className='routing-gateway-title'><span className='routing-gateway-icon'><RouteIcon name='route' size={24} /></span><div><h3>Agent2API</h3><span>{t('当前视图的请求入口')}</span></div></div>
          <div className='routing-running'><strong>{metrics.running ?? '—'}</strong><span>{t('当前请求')}</span><Badge shape='tag' variant='success'>{t('运行中')}</Badge></div>
          <div className='routing-gateway-url'><code title={gatewayBase}>{gatewayBase}/v1</code><IconButton name='copy' title={t('复制网关地址')} data-copy={`${gatewayBase}/v1`} /></div>
          <div className='routing-gateway-stats'>
            <div><span>{t('今日请求')}</span><b>{metrics.todayRequests?.toLocaleString() ?? '—'}</b></div>
            <div><span>{t('成功率')}</span><b>{metrics.todaySuccessRate == null ? '—' : `${metrics.todaySuccessRate.toFixed(1)}%`}</b></div>
            <div><span>{t('平均耗时')}</span><b>{formatDuration(metrics.averageDurationMs)}</b></div>
          </div>
          <div className='routing-gateway-policy'><Badge shape='tag' variant='brand'>{strategyLabel(workspace.settings.routingStrategy)}</Badge><Badge shape='tag' variant='outline'>{t('粘性：{mode}', { mode: stickyLabel(workspace.settings.routingSticky) })}</Badge></div>
          <Button variant='outline' className='w-full' onClick={onSettings}><RouteIcon name='settings' />{t('统一路由设置')}</Button>
          <div className='routing-gateway-shortcuts'><Button variant='ghost' size='sm' onClick={onReplay}>{t('请求跟踪')}</Button><Button variant='ghost' size='sm' onClick={onSticky}>{t('会话粘滞')} · {workspace.stickyCount}</Button></div>
        </div>
        <div className='routing-scope-note'><RouteIcon name={workspace.customized ? 'lock' : 'proxies'} /><p>{workspace.customized ? t('当前模型配置独立保存，不影响全局和其他模型。')
          : profile.providerScope === 'all' ? t('默认自动参与；新增可用提供商无需再添加路由，明确停用项保持不变。')
            : t('当前继承全局固定名单；新增提供商需手动添加或在全局配置中改为自动参与。')}</p></div>
      </div>
      <div className='routing-provider-column'>
        <div className='routing-column-head'>
          <span className='routing-column-label'><RouteIcon name='plugins' />{t('提供商与账号')} <Badge variant='secondary' shape='tag'>{profile.providers.length}</Badge></span>
          <div className='routing-column-actions'>
            <Input className='routing-account-search' placeholder={t('搜索提供商或账号')} value={search} onChange={event => patch({ search: event.currentTarget.value })} />
            <Button variant='outline' size='sm' disabled={!visibleActive.length}
              title={allCollapsed ? t('展开全部分组') : t('折叠全部分组（只留组头）')}
              onClick={() => setAllProvidersExpanded(allCollapsed)}>{allCollapsed ? t('展开全部') : t('折叠全部')}</Button>
          </div>
        </div>
        <div className='routing-provider-list'>
        {!providers.length ? <Empty>{query ? t('没有匹配的提供商或账号') : t('还没有参与路由的提供商')}<Button variant='outline' onClick={onAddProvider}>{addProviderLabel}</Button></Empty> : null}
        {renderedProviders.map((provider) => {
          const label = workspace.providers.find(item => item.id === provider)?.label || provider
          const unavailable = workspace.providers.find(item => item.id === provider)?.available === false
          const disabled = profile.disabledProviders.includes(provider) || unavailable
          const inactive = inactiveProvider(workspace, provider)
          const displayGroup = inactive ? visibleInactive : visibleActive
          const providerIndex = displayGroup.indexOf(provider)
          const ids = accountOrder(provider).filter(id => accounts.has(id))
          const orderRules = accountOrderRules(provider)
          const orderLocked = orderRules.length > 0
          const orderLockMessage = orderLocked ? manualOrderBlockedMessage(orderRules) : ''
          const rank = activeProviderRanks.get(provider)
          const effectiveRank = providerRanks.get(provider)
          const accountStates = ids.map(id => {
            const account = accounts.get(id)!
            return queue.get(id)?.state || (account.enabled === false ? 'disabled' : account.hasCredentials === false ? 'no-credentials' : 'unready')
          })
          const filteredIds = ids.filter(id => !query || label.toLocaleLowerCase().includes(query) || displayNameOf(accounts.get(id)).toLocaleLowerCase().includes(query))
          const activeIds = filteredIds.filter(id => accounts.get(id)?.enabled !== false)
          const disabledIds = filteredIds.filter(id => accounts.get(id)?.enabled === false)
          const showDisabled = Boolean(query) || disabledAccountsExpanded.has(provider)
          const visibleIds = [...activeIds, ...(showDisabled ? disabledIds : [])]
          const running = ids.reduce((sum, id) => sum + (metrics.accountRunning?.[id] || 0), 0)
          const expanded = expandedProviders.has(provider)
          const disabledToggle = disabledIds.length ? <Button variant='ghost' size='sm' className='routing-disabled-accounts-toggle' disabled={Boolean(query)} aria-expanded={showDisabled} onClick={() => setDisabledAccountsExpanded(current => {
            const next = new Set(current)
            if (next.has(provider)) next.delete(provider)
            else next.add(provider)
            return next
          })}><RouteIcon name='chevronDown' size={13} />{t('已禁用账号 · {n}', { n: disabledIds.length })}<span>{showDisabled ? t('收起') : t('展开')}</span></Button> : null
          return <React.Fragment key={provider}>
            {inactive && providerIndex === 0 ? inactiveToggle : null}
          <Collapsible render={<section />} open={expanded} onOpenChange={open => setProviderExpanded(provider, open)}
            className={cn('routing-provider-card', (disabled || inactive) && 'is-disabled', !inactive && selected === provider && 'is-selected', drag.dragging === provider && 'is-dragging', drag.drop?.id === provider && (drag.drop.after ? 'drop-after' : 'drop-before'))}
            data-routing-provider={provider} data-routing-connect={!inactive} data-disabled={disabled} onPointerEnter={() => patch({ selectedProvider: inactive ? '' : provider })}>
            <header>
              <Button variant='ghost' size='icon' className='routing-grip' disabled={busy || !!query} title={t('拖动调整提供商顺序')} aria-label={t('拖动调整提供商顺序')}
                onPointerDown={event => drag.start(event, 'provider', provider, provider)}><RouteIcon name='gripVertical' size={14} /></Button>
              <CollapsibleTrigger className='routing-provider-toggle' title={expanded ? t('折叠这一组') : t('展开这一组')}>
                <ProviderMark id={provider} label={label} /><span className='routing-provider-name'><b>{label}</b><span>{t('{n} 个账号', { n: ids.length })} · {t('{n} 个请求', { n: running })}</span></span>
                <ProviderRouteBadge rank={rank} effectiveRank={effectiveRank} unavailable={unavailable} disabled={disabled} states={accountStates} />
                {orderLocked ? <Badge shape='tag' variant='secondary' title={orderLockMessage}>{groupSortSummary(orderRules)}</Badge> : null}
                <RouteIcon name='chevronDown' size={14} />
              </CollapsibleTrigger>
              <IconButton name='arrowUp' title={t('提高优先级')} disabled={busy || providerIndex === 0 || !!query} onClick={() => void moveProvider(provider, -1)} />
              <IconButton name='arrowDown' title={t('降低优先级')} disabled={busy || providerIndex >= displayGroup.length - 1 || !!query} onClick={() => void moveProvider(provider, 1)} />
              <IconButton name='settings' title={t('提供商路由设置')} onClick={() => onProvider(provider)} />
            </header>
            <CollapsibleContent className='routing-account-list'>
              {!filteredIds.length ? <div className='routing-empty-accounts'>{t('没有参与路由的账号')}<Button variant='ghost' size='sm' onClick={() => onProvider(provider)}>{t('设置参与范围')}</Button></div> : null}
              {visibleIds.map(id => {
                const account = accounts.get(id)!
                const peerIds = account.enabled === false ? disabledIds : activeIds
                const index = peerIds.indexOf(id)
                const entry = queue.get(id)
                const state = disabled ? 'excluded' : entry?.state || (account.enabled === false ? 'disabled' : account.hasCredentials === false ? 'no-credentials' : 'unready')
                const picked = workspace.queue.pickedId === id && !disabled
                const description = workspace.queue.strategy === 'order' && state === 'usable'
                  ? picked ? t('首选账号') : t('后备账号')
                  : entry?.reason || (disabled ? t('该提供商在当前视图中已停用') : t('按当前视图的顺序参与选路'))
                const name = displayNameOf(account)
                return <React.Fragment key={id}>{account.enabled === false && id === disabledIds[0] ? disabledToggle : null}<div className={cn('routing-account-row', picked && 'is-picked', disabled && 'is-muted', drag.dragging === id && 'is-dragging', drag.drop?.id === id && (drag.drop.after ? 'drop-after' : 'drop-before'))}
                  data-routing-account={id} data-provider={provider} data-account-disabled={account.enabled === false} title={entry?.reason}>
                  <Button variant='ghost' size='icon' className={cn('routing-grip', orderLocked && 'is-locked')} disabled={busy || (!!query && !orderLocked)}
                    aria-disabled={orderLocked || undefined} aria-label={t('拖动调整账号顺序')} title={orderLockMessage || t('拖动调整账号顺序')}
                    onClick={event => { if (orderLocked && event.detail === 0) notify(orderLockMessage, 'err') }}
                    onPointerDown={event => drag.start(event, 'account', provider, id)}><RouteIcon name='gripVertical' size={14} /></Button>
                  <span className='routing-account-rank'>{(profile.accountOrder[provider] || []).indexOf(id) + 1}</span><span className='routing-account-avatar'>{name.slice(0, 1).toUpperCase()}</span>
                  <div className='routing-account-name'><b>{name}</b><span>{description}</span></div>
                  {picked ? <Badge variant='brand' shape='tag'>{t('队首')}</Badge> : <RouteStateBadge state={state} />}
                  <span className='routing-account-concurrency' title={t('在途请求 / 并发上限')}>{metrics.accountConnections?.[id] || 0}<em>/ {Number(account.maxConcurrent) || '∞'}</em></span>
                  <IconButton name='arrowUp' title={t('提高优先级')} disabled={busy || index === 0 || !!query || orderLocked} onClick={() => void moveAccount(provider, id, -1)} />
                  <IconButton name='arrowDown' title={t('降低优先级')} disabled={busy || index >= peerIds.length - 1 || !!query || orderLocked} onClick={() => void moveAccount(provider, id, 1)} />
                  <IconButton name='settings' title={t('账号设置')} onClick={() => onAccount(id)} />
                </div></React.Fragment>
              })}
              {!showDisabled ? disabledToggle : null}
            </CollapsibleContent>
          </Collapsible>
          </React.Fragment>
        })}
        {!inactiveExpanded && !query ? inactiveToggle : null}
        <Button variant='ghost' className='routing-add-provider' disabled={automatic && !workspace.providers.some(provider => profile.disabledProviders.includes(provider.id))} onClick={onAddProvider}><RouteIcon name='plus' />{addProviderLabel}</Button>
        </div>
      </div>
    </div>
  </div>
}
