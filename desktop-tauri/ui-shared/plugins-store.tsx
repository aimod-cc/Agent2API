import type * as React from 'react'
import { Badge, Button, Card, Checkbox, StatusDot, Spinner, cn } from '@ui'
import { pluginFailed, pluginPending, pluginPhaseLabel, type PluginLoadStatus } from './plugin-startup'
import { PluginArtwork, type PluginArtworkKind } from './plugin-artwork'

export { PluginArtwork, type PluginArtworkKind } from './plugin-artwork'
export type PluginTranslate = (key: string, params?: Record<string, string | number>) => string
export type PluginDisplayStatus = 'running' | 'disabled' | 'unavailable' | 'missing'
export type PluginCardSelection = { checked: boolean; disabled?: boolean; onChange: (checked: boolean) => void }

// 断点包含 16px 卡片间距，保证多列时每张卡片至少 320px。
export const PLUGIN_GRID_CLASS = 'grid grid-cols-1 gap-4 @min-[656px]/plugin-gallery:grid-cols-2 @min-[992px]/plugin-gallery:grid-cols-3 @min-[1328px]/plugin-gallery:grid-cols-4'

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

export function PluginGetButton({ disabled, onClick, t }: { disabled: boolean; onClick: () => void; t: PluginTranslate }) {
  return <Button variant='secondary' size='sm' disabled={disabled} onClick={onClick}
    className='rounded-sm border-primary-bd bg-primary-soft px-3 font-medium text-primary-fg hover:bg-primary-tint'>
    {t('获取')}
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

export function PluginStoreCard({ name, description, version, icon, artwork, highlightedSource = false, sourceLabel, badges, status,
  providerCount, enabledCount, actions, feedback, footerAction, metadata, startup, selection, onDetails, t }: {
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
  actions: React.ReactNode
  feedback?: React.ReactNode
  footerAction?: React.ReactNode
  metadata?: React.ReactNode
  selection?: PluginCardSelection
  onDetails: () => void
  t: PluginTranslate
}) {
  const pending = status !== 'disabled' && pluginPending(startup)
  const failed = status !== 'disabled' && (pluginFailed(startup) || startup?.phase === 'blocked')
  const running = status !== 'disabled' && (startup ? startup.phase === 'ready' : status === 'running')
  const statusText = status === 'disabled' ? t('已停用') : status === 'missing' ? t('未安装')
    : running ? t('运行中') : pluginPhaseLabel(startup, t)
  return <Card render={<article />} data-plugin-card='' aria-label={name}
    onClick={selection ? event => {
      if (selection.disabled || (event.target as Element).closest('button, input, a, [role="checkbox"], [role="switch"]')) return
      selection.onChange(!selection.checked)
    } : undefined}
    className={cn('flex h-full min-h-[220px] flex-col gap-4 border-hairline p-4 transition-colors hover:border-border-strong dark:shadow-none',
      selection && !selection.disabled && 'cursor-pointer', selection?.checked && 'border-primary bg-primary-soft/30 hover:border-primary')}>
    <div className='flex items-start gap-3'>
      <PluginArtwork src={icon} kind={artwork} />
      <div className='min-w-0 flex-1'>
        <h3 className='m-0 break-words text-[14px] font-semibold leading-5'>{name}</h3>
        <div className='mt-1.5 flex flex-wrap items-center gap-1.5'>
          <Badge variant={highlightedSource ? 'brand' : 'ghost'} shape='tag'
            className={cn('font-normal', !highlightedSource && 'border-0 px-0')}>{sourceLabel ?? t('社区插件')}</Badge>
          {badges}
        </div>
      </div>
      <div className='flex shrink-0 items-center gap-1'>{selection ? <Checkbox className='size-4'
        checked={selection.checked} disabled={selection.disabled} onCheckedChange={value => selection.onChange(value === true)}
        aria-label={t('选择插件 {name}', { name })} /> : actions}</div>
    </div>
    <div className='flex flex-col items-start gap-2'>
      <p className='m-0 line-clamp-3 min-h-[60px] w-full min-w-0 break-words text-[12px] leading-5 text-subtle'>{description}</p>
      <Button variant='link' size='xs' className='gap-1 px-0 text-primary-fg' aria-haspopup='dialog'
        aria-label={`${name} · ${t('查看详情')}`} onClick={onDetails}>
        {t('查看详情')}<svg viewBox='0 0 24 24' className='size-3 text-muted-foreground' fill='none' stroke='currentColor'
          strokeWidth='1.6' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'><path d='m9 5 7 7-7 7' /></svg>
      </Button>
    </div>
    {metadata}
    <div className='mt-auto flex flex-wrap items-center gap-x-2 gap-y-1 text-[11px] leading-4 text-muted-foreground'>
      <span className={cn('inline-flex items-center gap-1.5', running && 'text-success', failed && 'text-warning')}>
        {pending ? <Spinner className='size-3' /> : status !== 'missing' ? <StatusDot variant={running ? 'success' : failed ? 'warning' : 'muted'} /> : null}
        {statusText}
      </span>
      {providerCount !== undefined ? <>
        <span className='opacity-40' aria-hidden='true'>·</span>
        <span className='tabular-nums'>{t('{count} 家提供商', {
          count: enabledCount === undefined ? providerCount : `${enabledCount}/${providerCount}`,
        })}</span>
      </> : null}
      {version ? <span className='ml-auto tabular-nums'>v{version}</span> : null}
      {footerAction ? <span className='ml-auto flex flex-wrap items-center justify-end gap-2'>{footerAction}</span> : null}
    </div>
    {feedback}
  </Card>
}
