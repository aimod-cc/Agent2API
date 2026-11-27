import * as React from 'react'
import {
  Alert,
  AlertDialog,
  AlertDialogBody,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  Button,
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Progress,
  Spinner,
} from '@ui'
import type { PluginTranslate } from './plugins-store'

export function PluginTaskDialog({
  open,
  confirmOpen = false,
  onOpenChange,
  onClose,
  hasRows,
  title,
  subtitle,
  top,
  filterBar,
  listHeader,
  listHeaderAction,
  rowsRef,
  children,
  belowList,
  footerStatus,
  footerActions,
  nestedDialog,
}: {
  open: boolean
  confirmOpen?: boolean
  onOpenChange: (open: boolean) => void
  onClose: () => void
  hasRows: boolean
  title: React.ReactNode
  subtitle?: React.ReactNode
  top?: React.ReactNode
  filterBar?: React.ReactNode
  listHeader?: React.ReactNode
  listHeaderAction?: React.ReactNode
  rowsRef?: React.RefObject<HTMLDivElement | null>
  children: React.ReactNode
  belowList?: React.ReactNode
  footerStatus: React.ReactNode
  footerActions: React.ReactNode
  nestedDialog?: React.ReactNode
}) {
  return (
    <Dialog
      open={open}
      onOpenChange={(next, details) => {
        if (next) onOpenChange(true)
        else if (confirmOpen) details.cancel()
        else onClose()
      }}
    >
      <DialogContent
        className={
          hasRows
            ? 'h-[min(720px,calc(100vh-48px))] w-[min(860px,calc(100vw-48px))]'
            : 'w-[min(860px,calc(100vw-48px))]'
        }
      >
        <DialogHeader className='shrink-0'>
          <div>
            <DialogTitle>{title}</DialogTitle>
            {subtitle ? <p className='m-0 mt-1 text-xs text-muted-foreground'>{subtitle}</p> : null}
          </div>
        </DialogHeader>
        <DialogBody className={hasRows ? 'min-h-0 flex-1 gap-3.5' : 'min-h-0 gap-3.5'}>
          {top}
          {filterBar ? <div className='shrink-0'>{filterBar}</div> : null}
          <div className={hasRows ? 'plugins-file-list min-h-[160px] flex-1' : 'plugins-file-list'}>
            {listHeader || listHeaderAction ? (
              <div className='plugins-file-list-head'>
                <span>{listHeader}</span>
                {listHeaderAction}
              </div>
            ) : null}
            <div ref={rowsRef} className='plugins-file-rows'>
              {children}
            </div>
          </div>
          {belowList}
        </DialogBody>
        <DialogFooter className='shrink-0 flex-wrap'>
          <div className='mr-auto min-w-[180px] text-xs text-muted-foreground'>{footerStatus}</div>
          {footerActions}
        </DialogFooter>
        {nestedDialog}
      </DialogContent>
    </Dialog>
  )
}

export function PluginTaskProgressBar({
  running,
  title,
  done,
  total,
  detail,
  progressLabel,
  onInspect,
  onClose,
  t,
}: {
  running: boolean
  title: React.ReactNode
  done: number
  total: number
  detail?: React.ReactNode
  progressLabel: string
  onInspect: () => void
  onClose?: () => void
  t: PluginTranslate
}) {
  return (
    <div className='plugins-task' role='status' aria-live='polite'>
      <div className='plugins-task-head'>
        {running ? <Spinner className='size-3.5' /> : null}
        <span className='text-xs'>{title}</span>
        <span className='text-xs tabular-nums text-muted-foreground'>
          {done}/{total}
        </span>
        {detail ? <span className='plugins-task-current'>{detail}</span> : null}
        <div className='ml-auto flex items-center gap-2'>
          <Button variant='link' size='xs' onClick={onInspect}>
            {t('查看详情')}
          </Button>
          {!running && onClose ? (
            <Button variant='ghost' size='xs' onClick={onClose}>
              {t('关闭')}
            </Button>
          ) : null}
        </div>
      </div>
      <Progress value={done} max={Math.max(1, total)} aria-label={progressLabel} />
    </div>
  )
}

export function PluginTaskRemoveButton({
  label,
  onClick,
}: {
  label: string
  onClick: () => void
}) {
  return (
    <Button variant='ghost' size='icon-sm' aria-label={label} onClick={onClick}>
      <svg
        viewBox='0 0 16 16'
        className='size-3'
        fill='none'
        stroke='currentColor'
        strokeWidth='1.7'
        strokeLinecap='round'
        aria-hidden='true'
      >
        <path d='m4 4 8 8M12 4l-8 8' />
      </svg>
    </Button>
  )
}

export function ScriptTrustConfirmDialog({
  open,
  action,
  onCancel,
  onConfirm,
  t,
}: {
  open: boolean
  action: 'install' | 'update'
  onCancel: () => void
  onConfirm: () => void
  t: PluginTranslate
}) {
  const cancelRef = React.useRef<HTMLButtonElement>(null)
  const updating = action === 'update'
  return (
    <AlertDialog open={open} onOpenChange={next => { if (!next) onCancel() }}>
      <AlertDialogContent
        overlayForceRender
        className='w-[min(520px,calc(100vw-48px))] text-foreground'
        initialFocus={cancelRef}
      >
        <AlertDialogHeader>
          <AlertDialogTitle>{updating ? t('确认更新脚本插件') : t('确认安装脚本插件')}</AlertDialogTitle>
        </AlertDialogHeader>
        <AlertDialogBody className='gap-3'>
          <Alert variant='warning'>
            {t('脚本不是安全沙箱，可以访问网关所在电脑的文件和网络。不要安装来历不明的插件。')}
          </Alert>
          <div className='space-y-1.5 rounded-md border border-hairline bg-surface-inset px-3.5 py-3 text-xs leading-relaxed text-subtle'>
            <p className='m-0'>{t('Magpie 官方和社区插件均通过兼容方式接入 Agent2API，不保证 100% 可用。')}</p>
            <p className='m-0'>{t('签到及附带任务默认允许，实际执行遵循签到中心的开关和计划。')}</p>
            <p className='m-0 text-muted-foreground'>{t('部分插件的附带任务可能消耗额度、发送对话或创建及解散群聊。')}</p>
          </div>
          <AlertDialogDescription className='m-0 text-[13px] leading-relaxed text-foreground'>
            {t('我信任本次所选脚本插件的来源，并允许它们在网关电脑上运行。')}
          </AlertDialogDescription>
        </AlertDialogBody>
        <AlertDialogFooter>
          <Button ref={cancelRef} variant='outline' onClick={onCancel}>
            {t('取消')}
          </Button>
          <Button onClick={onConfirm}>{updating ? t('继续更新') : t('继续安装')}</Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}
