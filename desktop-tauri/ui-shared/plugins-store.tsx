import * as React from 'react'
import { Badge, Button, Card, Checkbox, StatusDot, Spinner, cn } from '@ui'
import { pluginFailed, pluginPending, pluginPhaseLabel, type PluginLoadStatus } from './plugin-startup'
import { PluginArtwork, type PluginArtworkKind } from './plugin-artwork'

export { PluginArtwork, type PluginArtworkKind } from './plugin-artwork'
export type PluginTranslate = (key: string, params?: Record<string, string | number>) => string
export type PluginDisplayStatus = 'running' | 'disabled' | 'unavailable' | 'missing'
export type PluginViewMode = 'cards' | 'list'
export type PluginCardSelection = { checked: boolean; disabled?: boolean; onChange: (checked: boolean) => void }

// 断点包含卡片间距，保证多列时卡片空间舒适不拥挤。
export const PLUGIN_GRID_CLASS = 'grid grid-cols-1 gap-3 @min-[600px]/plugin-gallery:grid-cols-2 @min-[920px]/plugin-gallery:grid-cols-3 @min-[1280px]/plugin-gallery:grid-cols-4'

export function formatPluginDownloads(count?: number | null): string | null {
  if (typeof count !== 'number' || !Number.isFinite(count) || count <= 0) return null
  const value = Math.floor(count)
  if (value >= 100_000) {
    const k = value / 1000
    return `${(value >= 1_000_000 ? k.toFixed(0) : k.toFixed(1)).replace(/\.0$/, '')}k`
  }
  return value.toLocaleString()
}

export function PluginDownloadCount({ downloads, t, className }: {
  downloads?: number | null
  t: PluginTranslate
  className?: string
}) {
  const formatted = formatPluginDownloads(downloads)
  if (!formatted || !downloads) return null
  const exact = Math.floor(downloads).toLocaleString()
  const label = t('{count} 次下载', { count: exact })
  return <span className={cn('inline-flex shrink-0 items-center gap-1 tabular-nums', className)}
    title={label} aria-label={label}>
    <svg viewBox='0 0 16 16' className='size-3 shrink-0' fill='none' stroke='currentColor' strokeWidth='1.6'
      strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
      <path d='M8 2.5v7.5m0 0-3-3m3 3 3-3M3 13h10' />
    </svg>
    {formatted}
  </span>
}

export function PluginSettingsButton({ name, onClick, t }: { name: string; onClick: () => void; t: PluginTranslate }) {
  const label = `${name} · ${t('设置')}`
  return <Button variant='ghost' size='icon-sm' aria-label={label} title={label} aria-haspopup='dialog' onClick={onClick}>
    <svg viewBox='0 0 24 24' className='size-4' fill='none' stroke='currentColor' strokeWidth='1.6'
      strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
      <circle cx='12' cy='12' r='3' />
      <path d='m10 3-.6 2.3-1.7 1-2.3-.6-2 3.5L5 11v2l-1.6 1.8 2 3.5 2.3-.6 1.7 1L10 21h4l.6-2.3 1.7-1 2.3.6 2-3.5L19 13v-2l1.6-1.8-2-3.5-2.3.6-1.7-1L14 3Z' />
    </svg>
  </Button>
}

export function PluginInstallButton({ disabled, onClick, t, size = 'sm' }: {
  disabled: boolean
  onClick: () => void
  t: PluginTranslate
  size?: 'xs' | 'sm'
}) {
  return <Button variant='secondary' size={size} disabled={disabled} onClick={onClick}
    className='rounded-sm border-primary-bd bg-primary-soft px-3 font-medium text-primary-fg hover:bg-primary-tint'>
    {t('安装')}
  </Button>
}

export function PluginSectionHeading({ title, count, hint, filters, actions, t }: {
  title: string
  count: number
  hint?: string
  filters?: React.ReactNode
  actions?: React.ReactNode
  t: PluginTranslate
}) {
  return <div className='mb-4 flex flex-wrap items-center justify-between gap-x-3 gap-y-2'>
    <div className='flex min-w-0 flex-wrap items-center gap-x-3 gap-y-2'>
      <h2 className='m-0 flex items-baseline gap-2 text-[15px] font-semibold leading-5'>
        {title}<span className='text-[12px] font-normal text-muted-foreground'>{t('{count} 个插件', { count })}</span>
      </h2>
      {filters}
    </div>
    <div className='flex flex-wrap items-center gap-3'>
      {hint ? <span className='text-[12px] text-muted-foreground'>{hint}</span> : null}
      {actions}
    </div>
  </div>
}

export type PluginStoreCardProps = {
  name: string
  description: string
  version?: string | null
  icon?: string | null
  artwork?: PluginArtworkKind
  highlightedSource?: boolean
  sourceLabel?: string
  badges?: React.ReactNode
  status: PluginDisplayStatus
  startup?: PluginLoadStatus | null
  providerCount?: number
  enabledCount?: number
  downloads?: number | null
  actions: React.ReactNode
  feedback?: React.ReactNode
  footerAction?: React.ReactNode
  metadata?: React.ReactNode
  selection?: PluginCardSelection
  onDetails: () => void
  t: PluginTranslate
}

function resolveStatusInfo(status: PluginDisplayStatus, startup: PluginLoadStatus | null | undefined, t: PluginTranslate) {
  const pending = status !== 'disabled' && pluginPending(startup)
  const failed = status !== 'disabled' && (pluginFailed(startup) || startup?.phase === 'blocked' || status === 'unavailable')
  const running = status !== 'disabled' && (startup ? startup.phase === 'ready' : status === 'running')
  const statusText = status === 'disabled' ? t('已停用') : status === 'missing' ? t('未安装')
    : running ? t('运行中') : pluginPhaseLabel(startup, t)
  return { pending, failed, running, statusText }
}

function isInteractiveTarget(target: EventTarget | null): boolean {
  return Boolean(target instanceof Element && target.closest('button, input, a, [role="checkbox"], [role="switch"]'))
}

export const PluginStoreCard = React.memo(function PluginStoreCard({ name, description, version, icon, artwork, highlightedSource = false, sourceLabel, badges, status,
  providerCount, enabledCount, downloads, actions, feedback, footerAction, metadata, startup, selection, onDetails, t }: PluginStoreCardProps) {
  const { pending, failed, running, statusText } = resolveStatusInfo(status, startup, t)
  return <Card render={<article />} data-plugin-card='' aria-label={name}
    onClick={event => {
      if (isInteractiveTarget(event.target)) return
      if (selection) {
        if (!selection.disabled) selection.onChange(!selection.checked)
      } else {
        onDetails()
      }
    }}
    className={cn(
      'flex h-full cursor-pointer flex-col gap-3 border-hairline p-3.5 transition-colors hover:border-border-strong dark:shadow-none',
      selection?.disabled && 'cursor-default',
      selection?.checked && 'border-primary bg-primary-soft/30 hover:border-primary',
      failed && !selection?.checked && 'border-warning-bd/60',
    )}>
    <div className='flex items-start gap-2.5'>
      <PluginArtwork src={icon} kind={artwork} />
      <div className='min-w-0 flex-1'>
        <h3 className='m-0 truncate text-[13.5px] font-semibold leading-5' title={name}>{name}</h3>
        <div className='mt-1 flex flex-wrap items-center gap-1'>
          <Badge variant={highlightedSource ? 'brand' : 'outline'} shape='tag'
            className={cn('font-normal', !highlightedSource && 'bg-muted text-muted-foreground dark:bg-surface-2')}>
            {sourceLabel ?? t('社区插件')}
          </Badge>
          {badges}
        </div>
      </div>
      <div className='flex shrink-0 items-center gap-1'>{selection ? <Checkbox className='size-4'
        checked={selection.checked} disabled={selection.disabled} onCheckedChange={value => selection.onChange(value === true)}
        aria-label={t('选择插件 {name}', { name })} /> : actions}</div>
    </div>
    <p className='m-0 line-clamp-2 min-h-[36px] w-full min-w-0 break-words text-[12px] leading-[18px] text-subtle'>{description}</p>
    {metadata}
    <div className='mt-auto flex flex-wrap items-center gap-x-2 gap-y-1 border-t border-hairline/70 pt-2.5 text-[11px] leading-4 text-muted-foreground'>
      <span className={cn('inline-flex items-center gap-1.5', running && 'text-success', failed && 'text-warning')}>
        {pending ? <Spinner className='size-3' /> : status !== 'missing' ? <StatusDot variant={running ? 'success' : failed ? 'warning' : 'muted'} /> : null}
        {statusText}
      </span>
      {providerCount !== undefined && providerCount > 1 ? <>
        <span className='opacity-40' aria-hidden='true'>·</span>
        <span className='tabular-nums'>{t('{count} 家提供商', {
          count: enabledCount === undefined ? providerCount : `${enabledCount}/${providerCount}`,
        })}</span>
      </> : null}
      {downloads && downloads > 0 ? <>
        <span className='opacity-40' aria-hidden='true'>·</span>
        <PluginDownloadCount downloads={downloads} t={t} />
      </> : null}
      {version ? <span className='ml-auto tabular-nums'>v{version}</span> : null}
      {footerAction ? <span className='flex flex-wrap items-center justify-end gap-1.5'>{footerAction}</span> : null}
    </div>
    {feedback}
  </Card>
})

export const PluginStoreListRow = React.memo(function PluginStoreListRow({ name, description, version, icon, artwork, highlightedSource = false, sourceLabel, badges, status,
  providerCount, enabledCount, downloads, actions, feedback, footerAction, metadata, startup, selection, onDetails, t }: PluginStoreCardProps) {
  const { pending, failed, running, statusText } = resolveStatusInfo(status, startup, t)
  return <article data-plugin-row='' data-selected={selection?.checked ? 'true' : undefined} aria-label={name}
    onClick={event => {
      if (isInteractiveTarget(event.target)) return
      if (selection) {
        if (!selection.disabled) selection.onChange(!selection.checked)
      } else {
        onDetails()
      }
    }}
    className={cn(
      'plugins-list-row flex flex-col gap-2 border-b border-hairline px-3.5 py-2.5 transition-colors last:border-b-0 hover:bg-control-hover',
      !selection?.disabled && 'cursor-pointer',
      selection?.checked && 'bg-primary-soft/35 hover:bg-primary-soft/45',
    )}>
    <div className='flex min-w-0 items-center gap-3'>
      {selection ? <Checkbox className='size-4 shrink-0'
        checked={selection.checked} disabled={selection.disabled} onCheckedChange={value => selection.onChange(value === true)}
        aria-label={t('选择插件 {name}', { name })} /> : null}
      <div className='shrink-0 [&_.plugin-artwork]:size-9 [&_.plugin-artwork_img]:size-7'>
        <PluginArtwork src={icon} kind={artwork} />
      </div>
      <div className='min-w-0 flex-1'>
        <div className='flex min-w-0 flex-wrap items-center gap-1.5'>
          <h3 className='m-0 truncate text-[13px] font-medium leading-5 text-foreground' title={name}>{name}</h3>
          <Badge variant={highlightedSource ? 'brand' : 'outline'} shape='tag'
            className={cn('font-normal', !highlightedSource && 'bg-muted text-muted-foreground dark:bg-surface-2')}>
            {sourceLabel ?? t('社区插件')}
          </Badge>
          {badges}
          {providerCount !== undefined && providerCount > 1 ? (
            <span className='text-[11px] tabular-nums text-muted-foreground'>
              {t('{count} 家提供商', {
                count: enabledCount === undefined ? providerCount : `${enabledCount}/${providerCount}`,
              })}
            </span>
          ) : null}
        </div>
        <p className='m-0 mt-0.5 truncate text-[11.5px] leading-4 text-subtle'>{description}</p>
      </div>
      {metadata ? <div className='hidden shrink-0 items-center lg:flex'>{metadata}</div> : null}
      <div className='hidden shrink-0 items-center gap-3 text-[11px] text-muted-foreground sm:flex'>
        <PluginDownloadCount downloads={downloads} t={t} />
        {version ? <span className='w-14 text-right tabular-nums'>v{version}</span> : <span className='w-14' />}
        <span className={cn('inline-flex w-16 items-center gap-1.5', running && 'text-success', failed && 'text-warning')}>
          {pending ? <Spinner className='size-3' /> : status !== 'missing' ? <StatusDot variant={running ? 'success' : failed ? 'warning' : 'muted'} /> : null}
          <span className='truncate'>{statusText}</span>
        </span>
      </div>
      {!selection ? <div className='flex shrink-0 items-center gap-1.5'>
        {footerAction}
        {actions}
      </div> : null}
    </div>
    {feedback ? <div className='pl-12'>{feedback}</div> : null}
  </article>
})

export const PluginStoreItem = React.memo(function PluginStoreItem(props: PluginStoreCardProps & { viewMode?: PluginViewMode }) {
  const { viewMode = 'cards', ...rest } = props
  return viewMode === 'list' ? <PluginStoreListRow {...rest} /> : <PluginStoreCard {...rest} />
})
