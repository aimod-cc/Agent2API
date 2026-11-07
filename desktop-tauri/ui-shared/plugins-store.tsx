import type * as React from 'react'
import { Avatar, AvatarFallback, AvatarImage, Badge, Button, Card, StatusDot, cn } from '@ui'

export type PluginTranslate = (key: string, params?: Record<string, string | number>) => string
export type PluginArtworkKind = 'web' | 'providers' | 'minimax' | 'script'
export type PluginDisplayStatus = 'running' | 'disabled' | 'unavailable' | 'missing'

const artworkColors: Record<PluginArtworkKind, string> = {
  web: 'linear-gradient(140deg, #0a84ff, #2fd0c5)',
  providers: 'linear-gradient(140deg, #5e5ce6, #0a84ff)',
  minimax: 'linear-gradient(140deg, #e2167e, #fe603c)',
  script: 'linear-gradient(140deg, #ff9f0a, #ff603c)',
}

export function PluginArtwork({ src, kind = 'script' }: { src?: string | null; kind?: PluginArtworkKind }) {
  return <Avatar className='size-12 rounded-lg' aria-hidden='true'>
    {src ? <AvatarImage src={src} alt='' /> : null}
    <AvatarFallback className='text-white' style={{ background: artworkColors[kind] }}>
      <svg viewBox='0 0 24 24' className='size-6' fill='none' stroke='currentColor' strokeWidth='1.8'
        strokeLinecap='round' strokeLinejoin='round'>
        {kind === 'web' ? <>
          <circle cx='12' cy='12' r='8.4' />
          <path d='M3.6 12h16.8M12 3.6c2.2 2.4 3.3 5.2 3.3 8.4S14.2 18 12 20.4C9.8 18 8.7 15.2 8.7 12S9.8 6 12 3.6Z' />
        </> : kind === 'providers' ? <path d='m12 3.4 2.4 5.4 5.4 2.4-5.4 2.4L12 19l-2.4-5.4L4.2 11.2l5.4-2.4Z' />
          : kind === 'minimax' ? <g fill='currentColor' stroke='none'>
            <rect x='3' y='9' width='2.6' height='6' rx='1.3' />
            <rect x='7.4' y='5' width='2.6' height='14' rx='1.3' />
            <rect x='11.8' y='7.6' width='2.6' height='8.8' rx='1.3' />
            <rect x='16.2' y='4' width='2.6' height='16' rx='1.3' />
          </g> : <>
            <rect x='4.5' y='4.5' width='15' height='15' rx='4' />
            <path d='m10 8-4 4 4 4m4-8 4 4-4 4' />
          </>}
      </svg>
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

export function PluginSectionHeading({ title, count, hint, actions, t }: {
  title: string
  count: number
  hint?: string
  actions?: React.ReactNode
  t: PluginTranslate
}) {
  return <div className='mb-3 flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1'>
    <h2 className='m-0 flex items-baseline gap-2 text-[15px] font-semibold leading-5'>
      {title}<span className='text-[12px] font-normal text-muted-foreground'>{t('{count} 个插件', { count })}</span>
    </h2>
    <div className='flex flex-wrap items-center gap-3'>
      {hint ? <span className='text-[12px] text-muted-foreground'>{hint}</span> : null}
      {actions}
    </div>
  </div>
}

export function PluginStoreCard({ name, description, version, icon, artwork, official, badges, status,
  providerCount, enabledCount, actions, feedback, footerAction, t }: {
  name: string
  description: string
  version?: string | null
  icon?: string | null
  artwork?: PluginArtworkKind
  official?: boolean
  badges?: React.ReactNode
  status: PluginDisplayStatus
  providerCount?: number
  enabledCount?: number
  actions: React.ReactNode
  feedback?: React.ReactNode
  footerAction?: React.ReactNode
  t: PluginTranslate
}) {
  const statusText = status === 'running' ? t('运行中') : status === 'disabled' ? t('已停用')
    : status === 'missing' ? t('未安装') : t('未运行')
  return <Card render={<article />} data-plugin-card='' aria-label={name} className='flex h-full flex-col gap-3 p-4'>
    <div className='flex items-start gap-3.5'>
      <PluginArtwork src={icon} kind={artwork} />
      <div className='min-w-0 flex-1'>
        <h3 className='m-0 break-words text-[14px] font-semibold leading-5'>{name}</h3>
        <div className='mt-1.5 flex flex-wrap items-center gap-1.5'>
          <Badge variant={official ? 'brand' : 'outline'} shape='tag'>{official ? t('官方') : t('社区')}</Badge>
          {badges}
        </div>
      </div>
      <div className='flex shrink-0 items-center gap-2.5'>{actions}</div>
    </div>
    <p className='m-0 min-h-9 break-words text-[12px] leading-[18px] text-subtle'>{description}</p>
    <div className='mt-auto flex flex-wrap items-center gap-x-1.5 gap-y-1 text-[11.5px] leading-[18px] text-muted-foreground'>
      <span className={cn('inline-flex items-center gap-1.5', status === 'running' && 'text-success', status === 'unavailable' && 'text-warning')}>
        {status !== 'missing' ? <StatusDot variant={status === 'running' ? 'success' : status === 'disabled' ? 'muted' : 'warning'} /> : null}
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
