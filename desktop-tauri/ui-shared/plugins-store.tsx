import type * as React from 'react'
import { Avatar, AvatarFallback, AvatarImage, Badge, Button, Card, StatusDot, Spinner, cn } from '@ui'
import { pluginFailed, pluginPending, pluginPhaseLabel, type PluginLoadStatus } from './plugin-startup'

export type PluginTranslate = (key: string, params?: Record<string, string | number>) => string
export type PluginArtworkKind = 'web' | 'providers' | 'script'
export type PluginDisplayStatus = 'running' | 'disabled' | 'unavailable' | 'missing'

export function PluginArtwork({ src, kind = 'script', fallback }: {
  src?: string | null
  kind?: PluginArtworkKind
  fallback?: React.ReactNode
}) {
  // 固定底色不随主题变化，插件原图不做反色或亮度处理。
  return <Avatar className='size-12 rounded-lg border border-black/10 bg-[#808080]' aria-hidden='true'>
    {src ? <AvatarImage src={src} alt='' referrerPolicy='no-referrer' className='m-auto size-9 object-contain' /> : null}
    <AvatarFallback className='bg-transparent text-white'>
      {fallback ?? <svg viewBox='0 0 24 24' className='size-6' fill='none' stroke='currentColor' strokeWidth='1.8'
        strokeLinecap='round' strokeLinejoin='round'>
        {kind === 'web' ? <>
          <circle cx='12' cy='12' r='8.4' />
          <path d='M3.6 12h16.8M12 3.6c2.2 2.4 3.3 5.2 3.3 8.4S14.2 18 12 20.4C9.8 18 8.7 15.2 8.7 12S9.8 6 12 3.6Z' />
        </> : kind === 'providers' ? <path d='m12 3.4 2.4 5.4 5.4 2.4-5.4 2.4L12 19l-2.4-5.4L4.2 11.2l5.4-2.4Z' />
          : <>
            <rect x='4.5' y='4.5' width='15' height='15' rx='4' />
            <path d='m10 8-4 4 4 4m4-8 4 4-4 4' />
          </>}
      </svg>}
    </AvatarFallback>
  </Avatar>
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

export function PluginGetButton({ disabled, onClick, t }: { disabled: boolean; onClick: () => void; t: PluginTranslate }) {
  return <Button variant='secondary' size='sm' disabled={disabled} onClick={onClick}
    className='rounded-pill border-primary-bd bg-primary-soft px-4 font-semibold text-primary-fg hover:bg-primary-tint'>
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
  return <div className='mb-3 flex flex-wrap items-center justify-between gap-x-3 gap-y-2'>
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
  providerCount, enabledCount, actions, feedback, footerAction, metadata, startup, t }: {
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
  t: PluginTranslate
}) {
  const pending = status !== 'disabled' && pluginPending(startup)
  const failed = status !== 'disabled' && pluginFailed(startup)
  const running = status !== 'disabled' && (startup ? startup.phase === 'ready' : status === 'running')
  const statusText = status === 'disabled' ? t('已停用') : status === 'missing' ? t('未安装')
    : startup ? pluginPhaseLabel(startup, t) : running ? t('运行中') : t('状态待确认')
  return <Card render={<article />} data-plugin-card='' aria-label={name} className='flex h-full flex-col gap-3 p-4'>
    <div className='flex items-start gap-3.5'>
      <PluginArtwork src={icon} kind={artwork} />
      <div className='min-w-0 flex-1'>
        <h3 className='m-0 break-words text-[14px] font-semibold leading-5'>{name}</h3>
        <div className='mt-1.5 flex flex-wrap items-center gap-1.5'>
          <Badge variant={highlightedSource ? 'brand' : 'outline'} shape='tag'>{sourceLabel ?? t('社区插件')}</Badge>
          {badges}
        </div>
      </div>
      <div className='flex shrink-0 items-center gap-2.5'>{actions}</div>
    </div>
    <p className='m-0 min-h-9 break-words text-[12px] leading-[18px] text-subtle'>{description}</p>
    {metadata}
    <div className='mt-auto flex flex-wrap items-center gap-x-1.5 gap-y-1 text-[11.5px] leading-[18px] text-muted-foreground'>
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
      {version ? <><span className='opacity-40' aria-hidden='true'>·</span><span className='tabular-nums'>v{version}</span></> : null}
      {footerAction ? <span className='ml-auto'>{footerAction}</span> : null}
    </div>
    {feedback}
  </Card>
}
