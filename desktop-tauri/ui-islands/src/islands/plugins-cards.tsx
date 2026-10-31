import * as React from 'react'
import { Badge, Button, Progress, Switch, Label, Card as UiCard } from '@ui'
import { errorMessage, formatSize, formatTime, platformLabel, pluginDescription, pluginName } from './plugins-shared'
import type { HostState, PackSummary, PluginOperation, PluginUpdate, StorePlugin } from './plugins-types'
import { t } from '../i18n'

function Card({ children, installed }: { children: React.ReactNode; installed?: boolean }) {
  return <UiCard render={<article />} className={'flex flex-col ' + (installed ? 'border-success/40' : '')}>{children}</UiCard>
}

function CardHead({ id, name, version, icon, children }: {
  id: string; name: string; version?: string | null; icon?: string | null; children?: React.ReactNode
}) {
  const title = pluginName(id, name)
  return <div className='flex gap-3 px-4 pt-4'>
    <div className='grid size-11 shrink-0 place-items-center overflow-hidden rounded-xl bg-primary-soft text-[19px] font-bold text-primary-fg'>
      {icon ? <img src={icon} alt='' className='size-full object-cover' /> : title.slice(0, 1)}
    </div>
    <div className='min-w-0 flex flex-1 flex-col gap-1.5'>
      <h3 className='flex flex-wrap items-center gap-2 text-[14px] font-semibold leading-snug'>
        {title}{version ? <span className='text-[11.5px] font-normal tabular-nums text-muted-foreground'>v{version}</span> : null}
      </h3>
      <div className='flex flex-wrap gap-1.5'>{children}</div>
    </div>
  </div>
}

export function OperationProgress({ operation }: { operation: PluginOperation }) {
  const stage = operation.stage === 'downloading' ? t('正在下载…')
    : operation.stage === 'verifying' ? t('正在校验包内容…')
    : operation.stage === 'waiting' ? t('等待当前任务结束，插件继续服务…')
    : operation.stage === 'loading' ? t('正在加载新插件…')
    : operation.stage === 'restoring' ? t('正在恢复旧版本插件…') : t('正在安装…')
  const download = operation.stage === 'downloading'
  return <div className='px-4 pb-4 pt-3' aria-live='polite'>
    <div className='mb-1.5 flex items-center justify-between gap-2 text-[11.5px]'>
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

export function StorePluginCard({ item, target, operation, disabled, onInstall, onNotes }: {
  item: StorePlugin
  target: string
  operation?: PluginOperation
  disabled: boolean
  onInstall: () => void
  onNotes: () => void
}) {
  const platform = item.platforms.find(platform => platform.target === target)
  const installable = Boolean(item.version && platform && !item.error)
  return <Card>
    <CardHead id={item.id} name={item.name} version={item.version} icon={item.icon}>
      {item.official ? <Badge variant='brand'>{t('官方')}</Badge> : null}
      {item.prerelease ? <Badge variant='warning'>{t('beta 版')}</Badge> : null}
      {!item.version ? <Badge variant='outline'>{t('暂未发布')}</Badge> : null}
    </CardHead>
    <p className='px-4 pt-2.5 text-[12px] leading-relaxed text-subtle'>
      {pluginDescription(item.id, item.description)}
    </p>
    <div className='flex flex-wrap gap-3 px-4 pt-3 text-[11px] text-muted-foreground'>
      <span>{platformLabel(target)}</span><span>{formatSize(platform?.size || 0) || t('当前平台暂无包')}</span>
    </div>
    {item.error ? <p className='px-4 pt-2 text-[12px] text-destructive'>{errorMessage(item.reason)}</p> : null}
    {operation?.error ? <p className='px-4 pt-2 text-[12px] text-destructive'>{errorMessage(operation.error)}</p> : null}
    {operation?.active ? <OperationProgress operation={operation} /> : <div className='mt-auto flex gap-2 px-4 pb-4 pt-3.5'>
      <Button size='sm' disabled={disabled || !installable} onClick={onInstall}>{t('下载并安装')}</Button>
      {item.version ? <Button variant='outline' size='sm' onClick={onNotes}>{t('查看更新日志')}</Button> : null}
    </div>}
  </Card>
}

export function InstalledPluginCard({ pack, store, host, update, operation, disabled, onEnabled, onSettings, onRemove, onUpdate, onNotes }: {
  pack: PackSummary
  store?: StorePlugin
  host: HostState
  update?: PluginUpdate
  operation?: PluginOperation
  disabled: boolean
  onEnabled: (enabled: boolean) => void
  onSettings: () => void
  onRemove: () => void
  onUpdate: () => void
  onNotes: () => void
}) {
  const required = pack.updateRequired === true
  const blocked = update?.error === true && !required
  const packHost = pack.host === undefined ? host : pack.host
  return <Card installed>
    <CardHead id={pack.id} name={store?.name || pack.name} version={pack.version} icon={pack.icon || store?.icon}>
      <Badge variant={pack.enabled ? 'default' : 'secondary'}>{pack.enabled ? t('已启用') : t('已停用')}</Badge>
      <Badge variant={pack.signed ? 'success' : 'warning'}>{pack.signed ? t('已签名') : t('未签名')}</Badge>
      {pack.version.includes('-') ? <Badge variant='warning'>{t('beta 版')}</Badge> : null}
      {required ? <Badge variant='destructive'>{t('需要补齐插件')}</Badge>
        : update?.available ? <Badge variant='brand'>{t('有更新')}</Badge> : null}
      {!pack.entryPresent ? <Badge variant='destructive'>{t('文件缺失')}</Badge> : null}
    </CardHead>
    <p className='px-4 pt-2.5 text-[12px] leading-relaxed text-subtle'>
      {pluginDescription(pack.id, store?.description || pack.description)}
    </p>
    <p className='px-4 pt-2 text-[11px] text-muted-foreground'>
      {t('{count} 家提供商 · 安装于 {time}', { count: String(pack.providers.length), time: formatTime(pack.installedAt) })}
    </p>
    <div className='flex flex-wrap gap-3 px-4 pt-3 text-[11px] text-muted-foreground'>
      <span>{packHost?.running ? t('运行中') : t('未运行')}</span>
      <span>{t('已加载')} {t('{count} 家', { count: String(pack.providers.filter(provider => provider.loaded).length) })}</span>
    </div>
    {required ? <div className='mx-4 mt-3 rounded-md border border-destructive/30 bg-destructive/5 px-3 py-2 text-[12px]' aria-live='polite'>
      <p className='font-medium text-destructive'>{t('当前网关要求插件版本不低于 v{version}，补齐前暂不加载。', { version: pack.requiredVersion || '' })}</p>
      <p className='mt-1 text-subtle'>{pack.signed
        ? t('正在自动补齐官方插件，不受日常更新开关和通道限制。')
        : t('未签名包不会自动覆盖；点击「补齐插件」将安装官方签名包。')}</p>
      {update?.reason ? <p className={'mt-1 ' + (update.error ? 'text-destructive' : 'text-subtle')}>{errorMessage(update.reason)}</p> : null}
      {update?.retryAt && update.retryAt > Date.now() ? <p className='mt-1 text-subtle'>
        {t('下次自动重试：{time}', { time: formatTime(update.retryAt) })}
      </p> : null}
    </div> : null}
    {!pack.entryPresent ? <p className='px-4 pt-2 text-[12px] text-destructive'>
      {t('插件包的可执行文件不在磁盘上（可能被手工删除或移动），请重新安装。')}
    </p> : null}
    {update?.available && !required ? <div className='mx-4 mt-3 flex flex-wrap items-center justify-between gap-2 border-t border-hairline pt-3'>
      <span className='text-[12px] font-medium text-primary-fg'>
        {t('可更新到 v{version}', { version: update.latest || '' })}
      </span>
      <div className='flex items-center gap-2 text-[12px] text-subtle'>
        {t('更新日志')}<Button variant='outline' size='xs' onClick={onNotes}>{t('查看')}</Button>
      </div>
    </div> : null}
    {update?.error && !required ? <p className='px-4 pt-2 text-[12px] text-destructive'>{errorMessage(update.reason)}</p> : null}
    {operation?.error ? <p className='px-4 pt-2 text-[12px] text-destructive'>{errorMessage(operation.error)}</p> : null}
    {packHost?.lastError ? <p className='px-4 pt-2 text-[12px] text-destructive'>{packHost.lastError}</p> : null}
    {operation?.hostError ? <p className='px-4 pt-2 text-[12px] text-warning'>{operation.hostError}</p> : null}
    {operation?.active ? <OperationProgress operation={operation} /> : <div className='mt-auto flex flex-wrap items-center gap-2 px-4 pb-4 pt-3.5'>
      <Label className='flex items-center gap-1.5 text-[12px] text-subtle'>
        {t('启用')}<Switch checked={pack.enabled} disabled={disabled} onCheckedChange={value => onEnabled(value === true)} />
      </Label>
      <Button variant='outline' size='sm' onClick={onSettings} aria-haspopup='dialog'>{t('设置')}</Button>
      <Button variant='outline' size='sm' disabled={disabled} onClick={onRemove}>{t('卸载')}</Button>
      <span className='flex-1' />
      {required ? <Button size='sm' disabled={disabled || update?.repairing === true} onClick={onUpdate}>
        {update?.repairing ? t('正在补齐插件…') : t('补齐插件')}
      </Button> : update?.available ? <Button size='sm' disabled={disabled || blocked} onClick={onUpdate}>
        {t('更新到 v{version}', { version: update.latest || '' })}
      </Button> : <span className={'text-[11px] ' + (update?.error ? 'text-destructive' : 'text-muted-foreground')}>
        {update?.checked ? errorMessage(update.reason) || t('已是最新') : t('尚未检查更新')}
      </span>}
    </div>}
  </Card>
}
