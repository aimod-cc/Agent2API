import { Button, Progress, Switch, Checkbox, cn, Card, Label, Collapsible, CollapsibleTrigger, CollapsibleContent } from '@ui'
import { Badge } from '../../components/ui'
import type { PackSummary, PluginOperation, PluginUpdate, StorePlugin } from '../../api/plugins'
import { pluginErrorMessage } from '../../api/plugins'
import { t } from '../../i18n'
import { PluginArtwork, PluginSettingsButton, type PluginCardSelection } from '../../../../ui-shared/plugins-store'
import { usePluginLoadStatus, requestPluginDiagnostics } from '../../../../ui-shared/plugin-startup-view'
import { pluginFailed, pluginPhaseLabel } from '../../../../ui-shared/plugin-startup'

function PluginProgress({ operation }: { operation?: PluginOperation }) {
  if (!operation) return null
  const stage = operation.stage === 'downloading' ? t('网关正在下载…')
    : operation.stage === 'verifying' ? t('网关正在校验…')
    : operation.stage === 'waiting' ? t('等待当前任务结束，插件继续服务…')
    : operation.stage === 'loading' ? t('正在加载新插件…')
    : operation.stage === 'restoring' ? t('正在恢复旧版本插件…') : t('网关正在安装…')
  return <>
    {operation.active ? <div className='my-3' aria-live='polite'>
      <p className='m-ck-note'>{operation.required ? `${t('正在补齐插件…')} · ` : ''}{stage}{operation.stage === 'downloading' && operation.percent != null ? ` ${Math.round(operation.percent)}%` : ''}</p>
      <Progress value={operation.stage === 'downloading' ? operation.percent : null} aria-label={stage} />
    </div> : null}
    {operation.error ? <p className='m-ck-error'>{pluginErrorMessage(operation.error)}</p> : null}
    {operation.hostError ? <p className='m-ck-error'>{operation.hostError}</p> : null}
  </>
}

function Head({ name, version, icon }: { name: string; version?: string | null; icon?: string | null }) {
  return <div className='m-ck-toolbar'>
    <PluginArtwork src={icon} fallback={name.slice(0, 1)} />
    <div className='m-ck-grow'>
      <div className='m-ck-title'>{t(name)}</div>
      <div className='m-ck-note'>{version ? `v${version}` : t('暂未发布')}</div>
    </div>
  </div>
}

export function StorePluginCard({ item, target, busy, operation, selection, onInstall, onDetails }: {
  item: StorePlugin
  target: string
  busy: boolean
  operation?: PluginOperation
  selection?: PluginCardSelection
  onInstall: () => void
  onDetails: () => void
}) {
  const asset = item.platforms?.find(platform => platform.target === 'wasm32-unknown-unknown')
  return <Card render={<article />} className={cn('m-card m-card-pad', selection?.checked && 'border-primary bg-primary-soft/30')}>
    <div className='flex items-start justify-between gap-3'><Head name={item.name || item.id} version={item.version} icon={item.icon} />
      {selection ? <Checkbox className='size-4' checked={selection.checked} disabled={selection.disabled} onCheckedChange={value => selection.onChange(value === true)} aria-label={t('选择插件 {name}', { name: item.name })} /> : null}
    </div>
    <p className='m-ck-note line-clamp-3 break-words'>{t(item.description)}</p>
    <Button variant='link' size='sm' className='px-0' aria-haspopup='dialog'
      aria-label={`${t(item.name || item.id)} · ${t('查看详情')}`} onClick={onDetails}>{t('查看详情')}</Button>
    <Badge tone='mute'>{t('社区插件')}</Badge>
    {item.reason ? <p className={item.error ? 'm-ck-error' : 'm-ck-note'}>{pluginErrorMessage(item.reason)}</p> : null}
    <PluginProgress operation={operation} />
    {!operation?.active && !selection ? <div className='m-ck-actions'>
      <Button size='sm' disabled={busy || !asset || !item.version || item.error} onClick={onInstall}>{t('在网关下载并安装')}</Button>
    </div> : null}
    {!asset ? <p className='m-ck-note'>{t('当前网关平台暂无插件包')}</p> : null}
  </Card>
}

export function PluginCard({ pack, target, busy, update, operation, selection, onEnabled, onProvider, onUpdate, onRemove, onSettings, onDetails }: {
  pack: PackSummary
  target: string
  busy: boolean
  update?: PluginUpdate
  operation?: PluginOperation
  selection?: PluginCardSelection
  onEnabled: (enabled: boolean) => void
  onProvider: (id: string, enabled: boolean) => void
  onUpdate: () => void
  onRemove: () => void
  onSettings: () => void
  onDetails: () => void
}) {
  const startup = usePluginLoadStatus(pack.id, pack.startup)
  const legacy = pack.supportState === 'legacy_unsupported'
  const required = pack.updateRequired === true
  const locked = busy || operation?.active === true
  return <Card render={<article />} className={cn('m-card m-card-pad', selection?.checked && 'border-primary bg-primary-soft/30')}>
    <div className='flex items-start justify-between gap-2'>
      <Head name={pack.name || pack.id} version={pack.version} icon={pack.icon} />
      {selection ? <Checkbox className='size-4' checked={selection.checked} disabled={selection.disabled} onCheckedChange={value => selection.onChange(value === true)} aria-label={t('选择插件 {name}', { name: pack.name })} /> : <PluginSettingsButton name={pack.name || pack.id} t={t} onClick={onSettings} />}
    </div>
    <div className='m-ck-actions'>
      <Badge tone='mute'>{t('社区插件')}</Badge>
      {!legacy ? <Badge tone={pack.enabled ? 'ok' : 'mute'}>{pack.enabled ? t('已启用') : t('已停用')}</Badge> : null}
      <Badge tone={pack.signed ? 'ok' : 'warn'}>{pack.signed ? t('已签名') : t('未签名')}</Badge>
      {pack.unavailableReason ? <Badge tone='mute'>{legacy ? t('旧插件已停止支持') : t('暂未开放')}</Badge> : null}
      {required ? <Badge tone='warn'>{t('接口不兼容')}</Badge> : null}
      {pack.enabled && !legacy ? <Badge tone={pluginFailed(startup) || startup?.phase === 'blocked' ? 'warn' : startup?.phase === 'ready' ? 'ok' : 'mute'}>
        {startup?.phase === 'ready' ? t('运行中') : pluginPhaseLabel(startup, t)}
      </Badge> : null}
    </div>
    {pack.description ? <p className='m-ck-note line-clamp-3 break-words'>{t(pack.description)}</p> : null}
    <Button variant='link' size='sm' className='px-0' aria-haspopup='dialog'
      aria-label={`${t(pack.name || pack.id)} · ${t('查看详情')}`} onClick={onDetails}>{t('查看详情')}</Button>
    {required ? <div className='mt-3 rounded-md border border-destructive/30 px-3 py-2' aria-live='polite'>
      <p className='m-ck-error'>{pack.compatibilityError || t('插件与当前网关的公共接口不兼容，暂未加载。')}</p>
      <p className='m-ck-note'>{t('请根据不兼容原因更新插件或网关；不会要求两者版本号一致。')}</p>
      {update?.reason ? <p className={update.error ? 'm-ck-error' : 'm-ck-note'}>{pluginErrorMessage(update.reason)}</p> : null}
      {update?.retryAt && update.retryAt > Date.now() ? <p className='m-ck-note'>
        {t('下次自动重试：{time}', { time: new Date(update.retryAt).toLocaleString() })}
      </p> : null}
    </div> : null}
    {pack.unavailableReason ? <p className='m-ck-note'>{t(pack.unavailableReason)}</p> : null}
    {!pack.entryPresent ? <p className='m-ck-error'>{t('插件文件缺失，请重新安装插件包')}</p> : null}
    {pack.enabled && (startup ? (pluginFailed(startup) || startup.phase === 'blocked') && startup.error : pack.host?.lastError)
      ? <p className='m-ck-error line-clamp-2 break-words'>{startup?.error || pack.host?.lastError}</p> : null}
    {update?.reason && !required ? <p className={update.error ? 'm-ck-error' : 'm-ck-note'}>{pluginErrorMessage(update.reason)}</p> : null}
    <PluginProgress operation={operation} />
    {!selection ? <div className='m-ck-actions'>
      <Label className='m-ck-toolbar m-ck-note'>
        {t('启用')}<Switch checked={!legacy && pack.enabled} disabled={locked || Boolean(pack.unavailableReason)} aria-label={t('启用插件 {name}', { name: pack.name })}
          onCheckedChange={onEnabled} />
      </Label>
      {pack.enabled && (pluginFailed(startup) || startup?.phase === 'blocked' || (!startup && pack.host?.lastError))
        ? <Button size='sm' variant='ghost' onClick={() => requestPluginDiagnostics(pack.id)}>{t('加载诊断')}</Button> : null}
      <Button size='sm' variant='outline' disabled={locked} onClick={onRemove}>{t('卸载')}</Button>
      {update?.available && !legacy ? <Button size='sm' disabled={locked || update.error || !update.asset
        || update.asset.target !== 'wasm32-unknown-unknown'} onClick={onUpdate}>
        {t('更新到 v{version}', { version: update.latest || '' })}
      </Button> : null}
    </div> : null}
    {!selection ? <Collapsible className='mt-3 border-t border-hairline pt-3'>
      <CollapsibleTrigger className='m-ck-title'>{t('提供商启停')}</CollapsibleTrigger><CollapsibleContent keepMounted>
      {pack.providers?.map(provider => <Label className='m-ck-toolbar py-2' key={provider.id}>
        <Switch checked={!legacy && provider.enabled && !provider.overriddenBy} disabled={locked || !pack.enabled || Boolean(pack.unavailableReason) || Boolean(provider.overriddenBy)}
          aria-label={t(provider.label)} onCheckedChange={enabled => onProvider(provider.id, enabled)} />
        <span className='m-ck-note m-ck-grow'>{t(provider.label)}</span>
        {provider.overriddenBy ? <Badge tone='mute'>{t('由其他插件接管')}</Badge>
          : provider.enabled && !provider.loaded ? <Badge tone={pluginFailed(startup) ? 'warn' : 'mute'}>{pluginPhaseLabel(startup, t)}</Badge> : null}
      </Label>)}
    </CollapsibleContent></Collapsible> : null}
  </Card>
}
