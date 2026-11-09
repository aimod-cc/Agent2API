import { Badge, Button, Progress, Switch } from '@ui'
import { errorMessage, formatSize, formatTime, pluginDescription, pluginName } from './plugins-shared'
import type { HostState, PackSummary, PluginOperation, PluginUpdate, StorePlugin } from './plugins-types'
import { t } from '../i18n'
import { GithubFailureText } from '../../../ui-shared/github-failure'
import { PluginGetButton, PluginSettingsButton, PluginStoreCard } from '../../../ui-shared/plugins-store'
import { requestPluginDiagnostics, usePluginLoadStatus } from '../../../ui-shared/plugin-startup-view'
import { pluginFailed } from '../../../ui-shared/plugin-startup'

export function OperationProgress({ operation }: { operation: PluginOperation }) {
  const stage = operation.stage === 'downloading' ? t('正在下载…')
    : operation.stage === 'verifying' ? t('正在校验包内容…')
    : operation.stage === 'waiting' ? t('等待当前任务结束，插件继续服务…')
    : operation.stage === 'loading' ? t('正在加载新插件…')
    : operation.stage === 'restoring' ? t('正在恢复旧版本插件…') : t('正在安装…')
  const download = operation.stage === 'downloading'
  return <div aria-live='polite'>
    <div className='mb-1.5 flex flex-wrap items-center justify-between gap-2 text-[11.5px]'>
      <span className='text-subtle'>{operation.required ? `${t('正在补齐插件…')} · ${stage}` : stage}</span>
      <span className='tabular-nums text-muted-foreground'>
        {download ? (operation.total > 0
          ? `${formatSize(operation.received) || '0 B'} / ${formatSize(operation.total)} · ${Math.round(operation.percent || 0)}%`
          : formatSize(operation.received) || '0 B') : t('请稍候')}
      </span>
    </div>
    <Progress value={download ? operation.percent : null} aria-label={stage} />
  </div>
}

export function PluginUpdateDetails({ pack, update, operation, disabled, onUpdate, onNotes }: {
  pack: PackSummary
  update?: PluginUpdate
  operation?: PluginOperation
  disabled: boolean
  onUpdate: () => void
  onNotes: () => void
}) {
  const required = pack.updateRequired === true
  return <div className='flex flex-col gap-3'>
    {required ? <div className='text-[12px] leading-relaxed'>
      <p className='m-0 font-medium text-destructive'>{pack.compatibilityError || t('插件与当前网关的公共接口不兼容，暂未加载。')}</p>
      <p className='m-0 mt-1 text-subtle'>{t('请根据不兼容原因更新插件或网关；不会要求两者版本号一致。')}</p>
      {update?.retryAt && update.retryAt > Date.now() ? <p className='m-0 mt-1 text-subtle'>
        {t('下次自动重试：{time}', { time: formatTime(update.retryAt) })}
      </p> : null}
    </div> : null}
    <div className='flex flex-wrap items-center justify-between gap-3'>
      <span className='text-[12px] text-muted-foreground'>
        {update?.available ? t('可更新到 v{version}', { version: update.latest || '' })
          : required || update?.error ? null : update?.checked ? errorMessage(update.reason) || t('已是最新') : t('尚未检查更新')}
      </span>
      <div className='flex flex-wrap items-center gap-2'>
        {update?.latest ? <Button variant='outline' size='sm' onClick={onNotes}>{t('查看更新日志')}</Button> : null}
        {update?.available ? <Button size='sm'
          disabled={disabled || operation?.active || update.error} onClick={onUpdate}>
          {t('更新到 v{version}', { version: update.latest || '' })}
        </Button> : null}
      </div>
    </div>
    {update?.reason && (update.error || required) ? <div className='text-[12px] text-destructive'><GithubFailureText value={update} t={t} /></div> : null}
    {!pack.entryPresent ? <p className='m-0 text-[12px] text-destructive'>{t('插件包的可执行文件不在磁盘上（可能被手工删除或移动），请重新安装。')}</p> : null}
    {operation?.error ? <p className='m-0 text-[12px] text-destructive'>{errorMessage(operation.error)}</p> : null}
    {operation?.hostError ? <p className='m-0 text-[12px] text-warning'>{operation.hostError}</p> : null}
    {operation?.active ? <OperationProgress operation={operation} /> : null}
  </div>
}

export function StorePluginCard({ item, target, operation, disabled, onInstall, onNotes }: {
  item: StorePlugin
  target: string
  operation?: PluginOperation
  disabled: boolean
  onInstall: () => void
  onNotes: () => void
}) {
  const platform = item.platforms.find(platform => platform.target === target || platform.target === 'wasm32-unknown-unknown')
  const installable = Boolean(item.version && platform && !item.error)
  return <PluginStoreCard t={t} name={pluginName(item.id, item.name)} description={pluginDescription(item.id, item.description)}
    version={item.version} icon={item.icon} artwork={item.id === 'agent2api-web-providers' ? 'web' : 'providers'}
    status='missing'
    badges={<>{item.prerelease ? <Badge variant='warning' shape='tag'>{t('beta 版')}</Badge> : null}
      {!item.version ? <Badge variant='outline' shape='tag'>{t('暂未发布')}</Badge> : null}</>}
    actions={<PluginGetButton t={t} disabled={disabled || operation?.active === true || !installable} onClick={onInstall} />}
    footerAction={item.version ? <Button variant='link' size='xs' onClick={onNotes}>{t('更新日志')}</Button> : null}
    feedback={<>
      {item.version && !platform ? <p className='m-0 text-[12px] text-warning'>{t('当前平台暂无包')}</p> : null}
      {item.error ? <div className='text-[12px] text-destructive'><GithubFailureText value={item} t={t} /></div> : null}
      {operation?.error ? <p className='m-0 text-[12px] text-destructive'>{errorMessage(operation.error)}</p> : null}
      {operation?.active ? <OperationProgress operation={operation} /> : null}
    </>} />
}

export function InstalledPluginCard({ pack, store, host, update, operation, disabled, onEnabled, onSettings }: {
  pack: PackSummary
  store?: StorePlugin
  host: HostState
  update?: PluginUpdate
  operation?: PluginOperation
  disabled: boolean
  onEnabled: (enabled: boolean) => void
  onSettings: () => void
}) {
  const startup = usePluginLoadStatus(pack.id, pack.startup)
  const packHost = pack.host === undefined ? host : pack.host
  const running = pack.enabled && packHost?.running === true && pack.entryPresent && !pack.updateRequired
  const name = pluginName(pack.id, store?.name || pack.name)
  return <PluginStoreCard t={t} name={name} description={pluginDescription(pack.id, store?.description || pack.description)}
    version={pack.version} icon={pack.icon || store?.icon} artwork={pack.id === 'agent2api-web-providers' ? 'web' : 'providers'}
    status={!pack.enabled ? 'disabled' : running ? 'running' : 'unavailable'} startup={startup}
    footerAction={<Button variant='link' size='xs' onClick={() => requestPluginDiagnostics(pack.id)}>{t('加载诊断')}</Button>}
    providerCount={pack.providers.length} enabledCount={pack.providers.filter(provider => provider.enabled && !provider.overriddenBy).length}
    badges={<>
      {pack.updateRequired ? <Badge variant='destructive' shape='tag'>{t('接口不兼容')}</Badge>
        : update?.available ? <Badge variant='brand' shape='tag'>{t('有更新')}{update.latest ? ` v${update.latest}` : ''}</Badge> : null}
      {pack.version.includes('-') ? <Badge variant='warning' shape='tag'>{t('beta 版')}</Badge> : null}
      {!pack.entryPresent ? <Badge variant='destructive' shape='tag'>{t('文件缺失')}</Badge> : null}
    </>}
    actions={<>
      <PluginSettingsButton t={t} name={name} onClick={onSettings} />
      <Switch checked={pack.enabled} disabled={disabled} aria-label={`${t('启用')} ${name}`} onCheckedChange={onEnabled} />
    </>}
    feedback={<>
      {pack.updateRequired ? <p className='m-0 text-[12px] leading-relaxed text-destructive'>
        {pack.compatibilityError || t('插件与当前网关的公共接口不兼容，暂未加载。')}
        <Button variant='link' size='xs' onClick={onSettings}>{t('查看详情')}</Button>
      </p> : null}
      {update?.error && !pack.updateRequired ? <div className='text-[12px] text-destructive'><GithubFailureText value={update} t={t} /></div> : null}
      {operation?.error ? <p className='m-0 break-words text-[12px] text-destructive'>{errorMessage(operation.error)}</p> : null}
      {(startup ? pluginFailed(startup) && startup.error : packHost?.lastError) ? <p className='m-0 break-words text-[12px] text-destructive'>{startup?.error || packHost?.lastError}</p> : null}
      {operation?.hostError ? <p className='m-0 break-words text-[12px] text-warning'>{operation.hostError}</p> : null}
      {operation?.active ? <OperationProgress operation={operation} /> : null}
    </>} />
}
