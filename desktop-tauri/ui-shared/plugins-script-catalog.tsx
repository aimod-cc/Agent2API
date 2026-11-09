import * as React from 'react'
import { Alert, Button, Spinner } from '@ui'
import type { ScriptCatalogInfo, ScriptPluginsState, ScriptRecommendation } from './plugins'
import type { PluginTranslate } from './plugins-store'

export type PluginOpenExternal = (url: string) => void | Promise<void>

export function ScriptRepository({ item, onOpenExternal, t }: {
  item: ScriptCatalogInfo
  onOpenExternal?: PluginOpenExternal
  t: PluginTranslate
}) {
  const repository = item.repository?.trim()
  if (!repository || !/^[a-z\d_-][a-z\d._-]*\/[a-z\d_-][a-z\d._-]*$/i.test(repository)) return null
  const url = `https://github.com/${repository}`
  const stars = typeof item.stars === 'number' && Number.isFinite(item.stars) && item.stars >= 0 ? item.stars : null
  const updated = item.starsAt ? t('Star 更新于 {time}', { time: new Date(item.starsAt).toLocaleString() }) : undefined
  return <div className='flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 text-[11.5px] text-muted-foreground'>
    <Button variant='link' size='xs' nativeButton={false} className='h-auto min-w-0 max-w-full shrink px-0 text-[11.5px]'
      render={<a href={url} target='_blank' rel='noopener noreferrer' />}
      title={url} aria-label={t('查看 GitHub 仓库：{repository}', { repository })}
      onClick={event => {
        if (!onOpenExternal || event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return
        event.preventDefault()
        void onOpenExternal(url)
      }}>
      <span className='shrink-0'>GitHub</span><span className='truncate'>{repository}</span>
    </Button>
    <span className='inline-flex shrink-0 items-center gap-1 tabular-nums' title={updated}
      aria-label={stars === null ? t('Star 数暂未读取') : t('{count} 个 Star', { count: stars.toLocaleString() })}>
      <svg viewBox='0 0 24 24' className='size-3.5' fill='none' stroke='currentColor' strokeWidth='1.6'
        strokeLinejoin='round' aria-hidden='true'><path d='m12 3 2.8 5.8 6.4.9-4.6 4.5 1.1 6.4L12 17.6l-5.7 3 1.1-6.4-4.6-4.5 6.4-.9Z' /></svg>
      {stars === null ? '—' : stars.toLocaleString()}
    </span>
  </div>
}

export function scriptUnavailableReason(item: ScriptRecommendation, t: PluginTranslate): string {
  switch (item.unavailableReason) {
    case '此插件不是提供商插件，当前无法安装': return t('此插件不是提供商插件，当前无法安装')
    case '仓库没有已提交的可加载入口，请等待作者发布构建产物': return t('仓库没有已提交的可加载入口，请等待作者发布构建产物')
    case '插件尚未声明有效版本，暂时无法安装': return t('插件尚未声明有效版本，暂时无法安装')
    case '插件尚未提供可下载的发布包': return t('插件尚未提供可下载的发布包')
    case '插件发布信息读取失败，请稍后刷新': return t('插件发布信息读取失败，请稍后刷新')
    default: return item.unavailableReason || t('当前暂不可安装')
  }
}

export function ScriptCatalogStatus({ catalog, request, refresh, disabled, t }: {
  catalog: NonNullable<ScriptPluginsState['catalog']>
  request: (path: string, body: Record<string, unknown>) => Promise<unknown>
  refresh: () => Promise<void>
  disabled?: boolean
  t: PluginTranslate
}) {
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')
  const pending = React.useRef(false)
  const refreshing = busy || catalog.refreshing
  const checkedAt = Math.max(catalog.marketCheckedAt, catalog.githubCheckedAt)
  const now = Date.now()
  const retryAt = [catalog.marketRetryAt, catalog.githubRetryAt].filter((value): value is number => Boolean(value && value > now))
  const problems = [
    catalog.marketError ? `${t('Magpie 官方名单')}：${catalog.marketError}` : '',
    catalog.githubError ? `GitHub：${catalog.githubError}` : '',
    error,
  ].filter(Boolean)

  async function reload(): Promise<void> {
    if (pending.current || refreshing || disabled) return
    pending.current = true
    setBusy(true); setError('')
    try {
      await request('/api/plugins/scripts/catalog/refresh', {})
      await refresh()
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
    } finally { pending.current = false; setBusy(false) }
  }

  return <div className='mb-3 space-y-2'>
    <div className='flex flex-wrap items-center justify-between gap-x-3 gap-y-2 text-[11.5px] text-muted-foreground'>
      <p className='m-0 min-w-0 flex-1 leading-relaxed'>{t('官方与推荐来自 Magpie 官方名单；第三方来自 GitHub 的 magpie-plugin 标签，不代表官方推荐。')}</p>
      <div className='flex flex-wrap items-center gap-2.5'>
        <span className='inline-flex items-center gap-1.5' role='status'>
          {refreshing ? <><Spinner className='size-3' />{catalog.total > 0
            ? t('正在读取插件信息 {done}/{total}', { done: catalog.completed, total: catalog.total })
            : t('正在读取插件清单…')}</>
            : checkedAt ? t('清单更新于 {time}', { time: new Date(checkedAt).toLocaleString() })
              : t('尚未取得插件清单')}
        </span>
        <Button variant='ghost' size='xs' disabled={disabled || refreshing} onClick={() => void reload()}>{t('刷新清单')}</Button>
      </div>
    </div>
    {problems.length ? <Alert variant='warning' className='text-xs'>
      <p className='m-0'>{t('部分信息未能更新，已读取的插件仍可查看。')}</p>
      {problems.map((problem, index) => <p key={index} className='m-0 mt-1 whitespace-pre-wrap break-words'>{problem}</p>)}
      {retryAt.length ? <p className='m-0 mt-1'>{t('受影响的来源将在 {time} 后自动重试', { time: new Date(Math.min(...retryAt)).toLocaleTimeString() })}</p> : null}
    </Alert> : null}
  </div>
}
