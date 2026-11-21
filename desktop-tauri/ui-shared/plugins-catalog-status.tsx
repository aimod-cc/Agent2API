import * as React from 'react'
import { Alert, Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, Spinner } from '@ui'
import type { ReleaseState } from './plugins'
import type { PluginTranslate } from './plugins-store'
import { summarizeNetworkError } from './updates'

/**
 * 「Agent2API 社区」插件目录拉取异常时的板块级提示条与详情弹窗。
 * 与下方 Magpie 插件的 ScriptCatalogStatus 保持同一视觉与交互口径：
 * 共性网络失败不在每张插件卡片上刷屏，只在板块顶部给出一条轻量提醒，需要排查时再点「查看详情」。
 */
export function PluginCatalogStatus({ catalog, refreshing, disabled, onRetry, t }: {
  catalog?: ReleaseState | null
  refreshing: boolean
  disabled?: boolean
  onRetry: () => void | Promise<void>
  t: PluginTranslate
}) {
  const [open, setOpen] = React.useState(false)
  if (!catalog?.error) return null
  const reason = catalog.reason ? summarizeNetworkError(catalog.reason, t) : { summary: t('无法连接插件仓库，请稍后重试'), detail: '' }
  const retryAt = catalog.retryAt && catalog.retryAt > Date.now() ? catalog.retryAt : 0

  return <>
    <Alert variant='warning' className='mb-3 flex flex-wrap items-center justify-between gap-x-3 gap-y-1.5 py-2 text-xs'>
      <p className='m-0 min-w-0 flex-1'>
        {t('暂未连上插件仓库，当前仅影响检查更新与目录刷新；已安装插件可正常使用。')}
      </p>
      <div className='flex shrink-0 items-center gap-1'>
        <Button variant='link' size='xs' className='text-inherit' disabled={disabled || refreshing} onClick={() => void onRetry()}>
          {refreshing ? <Spinner className='size-3' /> : null}{refreshing ? t('重试中…') : t('重试')}
        </Button>
        <Button variant='link' size='xs' className='text-inherit' onClick={() => setOpen(true)}>
          {t('查看详情')}
        </Button>
      </div>
    </Alert>
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent className='w-[min(560px,calc(100vw-32px))] text-foreground'>
        <DialogHeader className='shrink-0'>
          <DialogTitle>{t('插件仓库连接异常')}</DialogTitle>
        </DialogHeader>
        <DialogBody className='min-h-0 gap-3 text-sm'>
          <p className='m-0 text-muted-foreground'>
            {t('拉取社区插件列表或检查更新时发生网络错误，下方仍保留本地缓存的插件信息。')}
          </p>
          <p className='m-0 break-words text-warning'>{reason.summary}</p>
          {reason.detail ? (
            <div className='rounded-md border border-hairline bg-surface-2 px-3 py-2 text-[11.5px] leading-relaxed text-muted-foreground break-all'>
              {reason.detail}
            </div>
          ) : null}
          {catalog.checkedAt ? (
            <p className='m-0 text-xs text-muted-foreground'>
              {t('上次成功更新于 {time}', { time: new Date(catalog.checkedAt).toLocaleString() })}
            </p>
          ) : null}
          {retryAt ? (
            <p className='m-0 text-xs text-muted-foreground'>
              {t('将在 {time} 后自动重试', { time: new Date(retryAt).toLocaleTimeString() })}
            </p>
          ) : null}
        </DialogBody>
        <DialogFooter className='shrink-0'>
          <Button variant='outline' disabled={disabled || refreshing} onClick={() => void onRetry()}>
            {refreshing ? <Spinner className='size-3' /> : null}{t('立即重试')}
          </Button>
          <Button variant='ghost' onClick={() => setOpen(false)}>{t('关闭')}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </>
}
