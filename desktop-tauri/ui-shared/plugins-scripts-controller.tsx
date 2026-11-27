import * as React from 'react'
import {
  Badge, Button, Dialog, DialogBody, DialogContent, DialogDescription,
  DialogFooter, DialogHeader, DialogTitle, Input, Label, Switch,
} from '@ui'
import type { ScriptOperation, ScriptPackage, ScriptPluginsState, ScriptRecommendation, ScriptUpdate } from './plugins'
import { ScriptOperationProgress, scriptOperationMatches } from './plugins-script-progress'
import {
  PluginInstallButton, PluginSettingsButton, PluginStoreItem,
  type PluginCardSelection, type PluginTranslate, type PluginViewMode,
} from './plugins-store'
import { ScriptPluginSettings } from './plugins-script-settings'
import { PluginDetailsDialog, type PluginDetailsPane } from './plugins-details'
import { ScriptPluginInstallNotice } from './plugins-script-install-notice'
import { cachedPluginIconPath } from './provider-icons'
import { getPluginStartup, pluginFailed, pluginPending, refreshPluginStartup, startupForPack, subscribePluginStartup } from './plugin-startup'
import { requestPluginDiagnostics, usePluginLoadStatus } from './plugin-startup-view'
import {
  isRecommendationInstalled, recommendationKey, recommendedDescription, reinstallRequest,
  scriptOfficialBadge, scriptPluginDescription, scriptPluginName, scriptRecommendations,
  scriptSourceLabel, updateLabel,
} from './plugins-script-display'
import { ScriptRepository, scriptUnavailableReason, type PluginOpenExternal } from './plugins-script-catalog'

type InstallDraft = { name: string; version: string; id?: string }
type ScriptCardOptions = {
  viewMode: PluginViewMode
  selection?: PluginCardSelection
  selecting?: boolean
  selected?: boolean
  selectionDisabled?: boolean
  selectionKey?: string
  onToggleSelect?: (key: string, checked: boolean) => void
}

const EMPTY_RECS: ScriptRecommendation[] = []

function useResolvedSelection(
  selection: PluginCardSelection | undefined,
  selecting: boolean | undefined,
  selected: boolean | undefined,
  selectionDisabled: boolean | undefined,
  selectionKey: string | undefined,
  onToggleSelect: ((key: string, checked: boolean) => void) | undefined,
): PluginCardSelection | undefined {
  return React.useMemo(() => {
    if (selection) return selection
    if (!selecting || !selectionKey || !onToggleSelect) return undefined
    return {
      checked: Boolean(selected),
      disabled: selectionDisabled,
      onChange: checked => onToggleSelect(selectionKey, checked),
    }
  }, [selection, selecting, selectionKey, selected, selectionDisabled, onToggleSelect])
}

const ScriptInstalledRow = React.memo(function ScriptInstalledRow({
  pack,
  matchedRecommendation,
  update,
  operation,
  icon,
  viewMode,
  locked,
  selection,
  selecting,
  selected,
  selectionDisabled,
  selectionKey,
  onToggleSelect,
  onOpenDetails,
  onReinstall,
  onToggleEnabled,
  onOpenExternal,
  t,
}: {
  pack: ScriptPackage
  matchedRecommendation?: ScriptRecommendation
  update?: ScriptUpdate
  operation?: ScriptOperation
  icon?: string
  viewMode: PluginViewMode
  locked: boolean
  selection?: PluginCardSelection
  selecting?: boolean
  selected?: boolean
  selectionDisabled?: boolean
  selectionKey?: string
  onToggleSelect?: (key: string, checked: boolean) => void
  onOpenDetails: (pack: ScriptPackage, pane: PluginDetailsPane) => void
  onReinstall: (pack: ScriptPackage) => void
  onToggleEnabled: (id: string, enabled: boolean) => void
  onOpenExternal?: PluginOpenExternal
  t: PluginTranslate
}) {
  const startup = usePluginLoadStatus(pack.id, pack.startup)
  const matchedList = React.useMemo(
    () => (matchedRecommendation ? [matchedRecommendation] : EMPTY_RECS),
    [matchedRecommendation],
  )
  const name = scriptPluginName(pack, matchedList, t)
  const resolvedSelection = useResolvedSelection(
    selection, selecting, selected, selectionDisabled, selectionKey, onToggleSelect,
  )
  const showDiagnostics = pack.enabled && (pluginFailed(startup) || startup?.phase === 'blocked' || (!startup && pack.error))
  const progress = operation ? <ScriptOperationProgress operation={operation} t={t} /> : null
  const handleAbout = React.useCallback(() => onOpenDetails(pack, 'about'), [onOpenDetails, pack])
  const handleSettings = React.useCallback(() => onOpenDetails(pack, 'settings'), [onOpenDetails, pack])
  const handleReinstall = React.useCallback(() => onReinstall(pack), [onReinstall, pack])
  const handleEnabled = React.useCallback((enabled: boolean) => onToggleEnabled(pack.id, enabled), [onToggleEnabled, pack.id])
  return <PluginStoreItem
    viewMode={viewMode}
    t={t}
    name={name}
    description={scriptPluginDescription(pack, matchedList, t)}
    version={pack.version}
    icon={icon}
    highlightedSource={scriptOfficialBadge(pack)}
    sourceLabel={scriptSourceLabel(pack, t)}
    badges={update ? <Badge variant='warning' shape='tag'>{updateLabel(update, t)}</Badge> : undefined}
    metadata={<ScriptRepository item={pack} onOpenExternal={onOpenExternal} t={t} />}
    status={!pack.enabled ? 'disabled' : pack.running && !pack.error ? 'running' : 'unavailable'}
    startup={startup}
    selection={resolvedSelection}
    onDetails={handleAbout}
    footerAction={showDiagnostics || update ? <>
      {showDiagnostics ? <Button variant='link' size='xs'
        onClick={() => requestPluginDiagnostics(pack.id)}>{t('加载诊断')}</Button> : null}
      {update && !resolvedSelection ? <Button variant='outline' size='xs' disabled={locked}
        onClick={handleReinstall}>{t('更新')}</Button> : null}
    </> : null}
    providerCount={pack.providers.length > 1 ? pack.providers.length : undefined}
    enabledCount={pack.providers.length > 1 ? pack.providers.filter(provider => provider.enabled).length : undefined}
    actions={<>
      <PluginSettingsButton name={name} t={t} onClick={handleSettings} />
      <Switch checked={pack.enabled} disabled={locked} aria-label={`${t('启用')} ${name}`}
        onCheckedChange={handleEnabled} />
    </>}
    feedback={<>
      {pack.enabled && (startup ? (pluginFailed(startup) || startup.phase === 'blocked') && startup.error : pack.error)
        ? <p className='m-0 line-clamp-2 break-words text-xs text-destructive'>{startup?.error || pack.error}</p> : null}
      {progress}
    </>}
  />
})

const ScriptRecommendedRow = React.memo(function ScriptRecommendedRow({
  item,
  operation,
  icon,
  viewMode,
  locked,
  selection,
  selecting,
  selected,
  selectionDisabled,
  selectionKey,
  onToggleSelect,
  onOpenDetails,
  onInstall,
  onOpenExternal,
  t,
}: {
  item: ScriptRecommendation
  operation?: ScriptOperation
  icon?: string
  viewMode: PluginViewMode
  locked: boolean
  selection?: PluginCardSelection
  selecting?: boolean
  selected?: boolean
  selectionDisabled?: boolean
  selectionKey?: string
  onToggleSelect?: (key: string, checked: boolean) => void
  onOpenDetails: (item: ScriptRecommendation) => void
  onInstall: (item: ScriptRecommendation) => void
  onOpenExternal?: PluginOpenExternal
  t: PluginTranslate
}) {
  const resolvedSelection = useResolvedSelection(
    selection, selecting, selected, selectionDisabled, selectionKey, onToggleSelect,
  )
  const unavailable = item.installable === false ? scriptUnavailableReason(item, t) : ''
  const progress = operation ? <ScriptOperationProgress operation={operation} t={t} /> : null
  const handleDetails = React.useCallback(() => onOpenDetails(item), [onOpenDetails, item])
  const handleInstall = React.useCallback(() => onInstall(item), [onInstall, item])
  return <PluginStoreItem
    viewMode={viewMode}
    t={t}
    name={item.label}
    description={recommendedDescription(item, t)}
    version={item.version}
    onDetails={handleDetails}
    highlightedSource={scriptOfficialBadge(item)}
    sourceLabel={scriptSourceLabel(item, t)}
    icon={icon}
    status='missing'
    providerCount={item.providers.length > 1 ? item.providers.length : undefined}
    selection={resolvedSelection}
    metadata={<ScriptRepository item={item} onOpenExternal={onOpenExternal} t={t} />}
    feedback={<>{unavailable ? <p className='m-0 break-words text-xs text-muted-foreground'>{unavailable}</p> : null}{progress}</>}
    actions={unavailable ? <Badge variant='secondary' shape='tag'>{t('暂不可安装')}</Badge>
      : <PluginInstallButton size={viewMode === 'list' ? 'xs' : 'sm'} t={t} disabled={locked}
          onClick={handleInstall} />}
  />
})

export type ScriptPluginsController = {
  operation: ScriptOperation
  recommended: ScriptRecommendation[]
  locked: boolean
  busy: boolean
  failure: string
  iconUrl: (value?: string | null) => string | undefined
  renderInstalled: (pack: ScriptPackage, options: ScriptCardOptions) => React.ReactElement
  renderRecommended: (item: ScriptRecommendation, options: ScriptCardOptions) => React.ReactElement
  dialogs: React.ReactElement
}

export function useScriptPluginsController({
  state,
  request,
  refresh,
  t,
  disabled = false,
  installRequest = 0,
  onBusyChange,
  resolveIcon = path => path,
  onOpenExternal,
}: {
  state: ScriptPluginsState | null | undefined
  request: (path: string, body: Record<string, unknown>) => Promise<unknown>
  refresh: () => Promise<void>
  t: PluginTranslate
  disabled?: boolean
  installRequest?: number
  onBusyChange?: (busy: boolean) => void
  resolveIcon?: (path: string) => string
  onOpenExternal?: PluginOpenExternal
}): ScriptPluginsController {
  React.useSyncExternalStore(subscribePluginStartup, getPluginStartup)
  const [install, setInstall] = React.useState<InstallDraft | null>(null)
  const [installNotice, setInstallNotice] = React.useState<InstallDraft | null>(null)
  const [remove, setRemove] = React.useState<ScriptPackage | null>(null)
  const [settingsId, setSettingsId] = React.useState<string | null>(null)
  const [detailsPane, setDetailsPane] = React.useState<PluginDetailsPane>('settings')
  const [detailsRecommendation, setDetailsRecommendation] = React.useState<ScriptRecommendation | null>(null)
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')
  const [operationTarget, setOperationTarget] = React.useState<ScriptOperation['target']>()
  const pending = React.useRef(false)
  const lastInstallRequest = React.useRef(0)
  const prefix = React.useId()

  const packages = state?.packages ?? []
  const recommended = React.useMemo(() => (state ? scriptRecommendations(state) : []), [state])
  const recommendedByPackId = React.useMemo(() => {
    const byName = new Map<string, ScriptRecommendation[]>()
    for (const item of recommended) {
      const list = byName.get(item.name)
      if (list) list.push(item)
      else byName.set(item.name, [item])
    }
    const matched = new Map<string, ScriptRecommendation>()
    for (const pack of packages) {
      const hit = byName.get(pack.name)?.find(item => isRecommendationInstalled(item, [pack]))
      if (hit) matched.set(pack.id, hit)
    }
    return matched
  }, [packages, recommended])
  const scriptUpdateById = React.useMemo(() => {
    const map = new Map<string, ScriptUpdate>()
    for (const item of state?.updates?.updates || []) {
      if (item.available === true) map.set(item.id, item)
    }
    return map
  }, [state?.updates?.updates])

  const locked = disabled || busy || state?.operation.active === true
  const stateRef = React.useRef(state)
  stateRef.current = state
  const lockedRef = React.useRef(locked)
  lockedRef.current = locked
  const requestRef = React.useRef(request)
  requestRef.current = request
  const refreshRef = React.useRef(refresh)
  refreshRef.current = refresh

  const detailsItem = detailsRecommendation
    ? recommended.find(item => recommendationKey(item) === recommendationKey(detailsRecommendation)) || detailsRecommendation
    : null
  const github = /^(github:|https:\/\/github\.com\/)/i.test(install?.name.trim() || '')
  const magpie = install?.name.trim().startsWith('magpie:') === true
  const settingsPack = packages.find(pack => pack.id === settingsId)
  const operation: ScriptOperation = React.useMemo(() => (
    state
      ? (state.operation.target ? state.operation : { ...state.operation, target: operationTarget })
      : { active: false, action: '', stage: '', message: '', error: '', updatedAt: 0 }
  ), [state, operationTarget])

  const iconUrl = React.useCallback((value?: string | null) => {
    const path = cachedPluginIconPath(value)
    return path ? resolveIcon(path) : undefined
  }, [resolveIcon])

  React.useEffect(() => { onBusyChange?.(busy) }, [busy, onBusyChange])
  React.useEffect(() => () => { onBusyChange?.(false) }, [onBusyChange])
  React.useEffect(() => {
    if (!state?.operation.active) setOperationTarget(undefined)
  }, [state?.operation.active, state?.operation.updatedAt])
  React.useEffect(() => {
    if (installRequest <= lastInstallRequest.current) return
    lastInstallRequest.current = installRequest
    setError(''); setSettingsId(null); setDetailsRecommendation(null); setInstallNotice(null)
    setInstall({ name: '', version: '' })
  }, [installRequest])
  React.useEffect(() => {
    if (settingsId && !settingsPack) setSettingsId(null)
  }, [settingsId, settingsPack])
  React.useEffect(() => {
    if (detailsRecommendation && isRecommendationInstalled(detailsRecommendation, packages)) setDetailsRecommendation(null)
  }, [detailsRecommendation, packages])

  const act = React.useCallback(async (path: string, body: Record<string, unknown>): Promise<boolean> => {
    const currentState = stateRef.current
    if (pending.current || lockedRef.current || !currentState) return false
    pending.current = true
    setBusy(true); setError('')
    const id = typeof body.id === 'string' ? body.id : undefined
    const pack = currentState.packages.find(item => item.id === id)
    setOperationTarget({ id, name: pack?.name, source: typeof body.name === 'string' ? body.name.trim() : undefined })
    try {
      await requestRef.current(path, body)
      refreshPluginStartup()
      await refreshRef.current()
      return true
    } catch (failure) {
      setOperationTarget(undefined)
      setError(failure instanceof Error ? failure.message : String(failure))
      return false
    } finally { pending.current = false; setBusy(false) }
  }, [])

  const openInstall = React.useCallback((value: InstallDraft): void => {
    setSettingsId(null); setDetailsRecommendation(null); setError(''); setInstallNotice(null); setInstall(value)
  }, [])

  const openDetails = React.useCallback((pack: ScriptPackage, pane: PluginDetailsPane): void => {
    setError(''); setDetailsRecommendation(null); setDetailsPane(pane); setSettingsId(pack.id)
  }, [])

  const handleToggleEnabled = React.useCallback((id: string, enabled: boolean) => {
    void act('/api/plugins/scripts/change', { id, action: 'enable', enabled })
  }, [act])

  const handleReinstall = React.useCallback((pack: ScriptPackage) => {
    openInstall(reinstallRequest(pack))
  }, [openInstall])

  const handleInstallRecommended = React.useCallback((item: ScriptRecommendation) => {
    openInstall({ name: item.source || item.name, version: item.selector ?? item.version })
  }, [openInstall])

  const handleOpenRecommendationDetails = React.useCallback((item: ScriptRecommendation) => {
    setSettingsId(null)
    setDetailsRecommendation(item)
  }, [])

  async function confirmInstall(): Promise<void> {
    if (!installNotice || locked || pending.current) return
    const source = installNotice.name.trim()
    const accepted = await act('/api/plugins/scripts/install', {
      id: installNotice.id, name: source, source, version: installNotice.version.trim(), trusted: true,
      checkinAllowed: true,
    })
    if (accepted) { setInstallNotice(null); setInstall(null) }
  }

  const starting = packages.some(pack => pluginPending(startupForPack(pack.id, pack.startup)))
  const rawFailure = error || state?.error || (!starting && (state?.hostError || state?.runtime.error || state?.operation.error))
  const failure = typeof rawFailure === 'string' ? rawFailure : ''

  const renderInstalled = React.useCallback((pack: ScriptPackage, options: ScriptCardOptions) => (
    <ScriptInstalledRow
      key={`script-installed:${pack.id}`}
      pack={pack}
      matchedRecommendation={recommendedByPackId.get(pack.id)}
      update={scriptUpdateById.get(pack.id)}
      operation={scriptOperationMatches(operation, pack) ? operation : undefined}
      icon={iconUrl(pack.icon)}
      viewMode={options.viewMode}
      locked={locked}
      selection={options.selection}
      selecting={options.selecting}
      selected={options.selected}
      selectionDisabled={options.selectionDisabled}
      selectionKey={options.selectionKey}
      onToggleSelect={options.onToggleSelect}
      onOpenDetails={openDetails}
      onReinstall={handleReinstall}
      onToggleEnabled={handleToggleEnabled}
      onOpenExternal={onOpenExternal}
      t={t}
    />
  ), [recommendedByPackId, scriptUpdateById, operation, iconUrl, locked, openDetails, handleReinstall, handleToggleEnabled, onOpenExternal, t])

  const renderRecommended = React.useCallback((item: ScriptRecommendation, options: ScriptCardOptions) => (
    <ScriptRecommendedRow
      key={`script-recommended:${recommendationKey(item)}`}
      item={item}
      operation={scriptOperationMatches(operation, item) ? operation : undefined}
      icon={iconUrl(item.icon)}
      viewMode={options.viewMode}
      locked={locked}
      selection={options.selection}
      selecting={options.selecting}
      selected={options.selected}
      selectionDisabled={options.selectionDisabled}
      selectionKey={options.selectionKey}
      onToggleSelect={options.onToggleSelect}
      onOpenDetails={handleOpenRecommendationDetails}
      onInstall={handleInstallRecommended}
      onOpenExternal={onOpenExternal}
      t={t}
    />
  ), [operation, iconUrl, locked, handleOpenRecommendationDetails, handleInstallRecommended, onOpenExternal, t])

  const dialogs = <>
    {detailsItem ? <PluginDetailsDialog name={detailsItem.label} version={detailsItem.version} sourceLabel={scriptSourceLabel(detailsItem, t)}
      description={recommendedDescription(detailsItem, t)} icon={iconUrl(detailsItem.icon)} t={t} onClose={() => setDetailsRecommendation(null)}
      metadata={<>
        <ScriptRepository item={detailsItem} onOpenExternal={onOpenExternal} t={t} />
        {detailsItem.installable === false ? <p className='m-0 text-xs text-warning'>{scriptUnavailableReason(detailsItem, t)}</p> : null}
      </>}
      primaryAction={<PluginInstallButton t={t} disabled={locked || detailsItem.installable === false}
        onClick={() => openInstall({ name: detailsItem.source || detailsItem.name, version: detailsItem.selector ?? detailsItem.version })} />} /> : null}
    {settingsPack ? <ScriptPluginSettings key={settingsPack.id} pack={settingsPack} icon={iconUrl(settingsPack.icon)} recommended={recommended} t={t}
      initialPane={detailsPane}
      disabled={locked} error={error} request={request} refresh={refresh} onOpenExternal={onOpenExternal} onClose={() => setSettingsId(null)}
      onEnabled={enabled => { void act('/api/plugins/scripts/change', { id: settingsPack.id, action: 'enable', enabled }) }}
      onProvider={(provider, enabled) => { void act('/api/plugins/scripts/change', { id: settingsPack.id, action: 'provider', provider, enabled }) }}
      onReinstall={() => openInstall(reinstallRequest(settingsPack))}
      onRemove={() => { setError(''); setSettingsId(null); setRemove(settingsPack) }} /> : null}
    <Dialog open={install !== null} onOpenChange={(open, details) => {
      if (!open) { if (busy || installNotice) details.cancel(); else setInstall(null) }
    }}>
      <DialogContent className='text-foreground'>
        <DialogHeader><DialogTitle>{install?.id ? t('更新脚本插件') : t('安装脚本插件')}</DialogTitle></DialogHeader>
        <DialogBody className='gap-4'>
          <DialogDescription className='text-xs leading-relaxed text-subtle'>{t('仅安装你信任的 npm 包、GitHub 仓库或 Magpie 社区分发包；完整性校验不等于安全审计。')}</DialogDescription>
          <div className='space-y-2'><Label htmlFor={`${prefix}-package`}>{t('npm 包名、GitHub 地址或 magpie:包名')}</Label>
            <Input id={`${prefix}-package`} value={install?.name || ''} disabled={busy} autoComplete='off' placeholder='@scope/plugin / https://github.com/owner/repo'
              onChange={event => { setInstall(value => value ? { ...value, name: event.target.value } : value) }} /></div>
          <div className='space-y-2'><Label htmlFor={`${prefix}-version`}>{github ? t('分支、标签或提交（留空使用默认分支）') : t('插件版本（留空使用 latest）')}</Label>
            <Input id={`${prefix}-version`} value={install?.version || ''} disabled={busy} placeholder={github ? t('默认分支') : 'latest'} autoComplete='off'
              onChange={event => { setInstall(value => value ? { ...value, version: event.target.value } : value) }} /></div>
          {github ? <p className='m-0 text-xs leading-relaxed text-muted-foreground'>{t('从 GitHub 官方源下载固定提交。仓库须包含可运行的提供商入口；安装运行依赖，但不执行构建和安装脚本。')}</p> : null}
          {magpie ? <p className='m-0 text-xs leading-relaxed text-muted-foreground'>{t('从 Magpie 社区官方分发目录下载，核验 SHA-512；更新与重装继续使用此来源。')}</p> : null}
          <p className='m-0 text-xs leading-relaxed text-muted-foreground'>{t('也可输入 file: 加网关电脑上的插件目录或 tgz 文件绝对路径；本地插件会复制安装，不修改源目录。')}</p>
          <p className='m-0 text-xs leading-relaxed text-muted-foreground'>{t('npm 未发布的包会尝试 Magpie 社区分发源；不会绕过完整性校验或执行安装脚本。')}</p>
          {state ? <div className='space-y-2 text-xs leading-relaxed text-muted-foreground'>
            <Badge variant='outline'>Bun {state.runtime.version} · {state.runtime.installed ? t('已下载') : t('按需下载')}</Badge>
            <p className='m-0'>{t('运行时不内置于安装包，首次安装脚本插件时自动下载并校验。')}</p>
            <p className='m-0'>{t('兼容 OpenCode v1 / Magpie 提供商插件及其运行依赖；不加载界面、独立工具或 Pi 扩展。')}</p>
            <p className='m-0'>{t('支持自动授权、设备码、手动授权码和插件声明的凭据表单；本机读取作用于网关电脑。')}</p>
          </div> : null}
          {error ? <p role='alert' className='m-0 text-xs text-destructive'>{error}</p> : null}
        </DialogBody>
        <DialogFooter><Button variant='outline' disabled={busy} onClick={() => setInstall(null)}>{t('取消')}</Button>
          <Button disabled={locked || !install?.name.trim()} onClick={() => {
            if (install && install.name.trim() && !locked && !pending.current) {
              setError('')
              setInstallNotice({ ...install, name: install.name.trim(), version: install.version.trim() })
            }
          }}>{install?.id ? t('继续更新') : t('继续安装')}</Button></DialogFooter>
        <ScriptPluginInstallNotice open={installNotice !== null} updating={Boolean(installNotice?.id)} source={installNotice?.name || ''}
          version={installNotice?.version || ''} disabled={locked} busy={busy} error={error} t={t}
          onCancel={() => { setInstallNotice(null); setError('') }} onConfirm={() => void confirmInstall()} />
      </DialogContent>
    </Dialog>
    <Dialog open={remove !== null} onOpenChange={open => { if (!open && !busy) setRemove(null) }}>
      <DialogContent className='text-foreground'><DialogHeader><DialogTitle>{t('卸载插件包')}</DialogTitle></DialogHeader>
        <DialogBody><p className='m-0 text-sm leading-relaxed'>{t('确定卸载「{name}」吗？对应提供商将停止转发，账号数据仍然保留，重新安装后恢复。', { name: remove?.name || '' })}</p>
          {error ? <p role='alert' className='m-0 text-xs text-destructive'>{error}</p> : null}
        </DialogBody>
        <DialogFooter><Button variant='outline' disabled={busy} onClick={() => setRemove(null)}>{t('取消')}</Button>
          <Button variant='destructive' disabled={locked} onClick={async () => {
            if (remove && await act('/api/plugins/scripts/change', { id: remove.id, action: 'remove' })) setRemove(null)
          }}>{t('卸载')}</Button></DialogFooter>
      </DialogContent>
    </Dialog>
  </>

  return { operation, recommended, locked, busy, failure, iconUrl, renderInstalled, renderRecommended, dialogs }
}
