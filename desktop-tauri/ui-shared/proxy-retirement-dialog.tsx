import * as React from 'react'
import {
  Button, Checkbox, Dialog, DialogBody, DialogContent, DialogDescription,
  DialogFooter, DialogTitle, Label,
} from '../ui-kit/src'
import { acknowledgeProxyRetirement } from './proxy-retirement'

type Translate = (key: string) => string

export function ProxyRetirementDialog({ open, onClose, t, scope = '' }: {
  open: boolean
  onClose: () => void
  t: Translate
  scope?: string
}) {
  const [hide, setHide] = React.useState(false)
  const checkboxId = React.useId()
  React.useEffect(() => { if (open) setHide(false) }, [open])

  function acknowledge(): void {
    if (hide) acknowledgeProxyRetirement(scope)
    onClose()
  }

  return (
    <Dialog open={open} disablePointerDismissal onOpenChange={next => { if (!next) onClose() }}>
      <DialogContent className='max-w-[540px]'>
        <DialogBody className='gap-0 px-7 pt-7 pb-6 max-sm:px-5'>
          <div className='mb-5 grid size-11 place-items-center rounded-md border border-primary-bd bg-primary-soft text-primary'>
            <svg width='23' height='23' viewBox='0 0 24 24' fill='none' stroke='currentColor' strokeWidth='1.5' aria-hidden='true'>
              <circle cx='12' cy='12' r='9' /><path d='M12 11v5m0-9h.01' strokeLinecap='round' />
            </svg>
          </div>
          <span className='text-[11px] tracking-wide text-muted-foreground'>{t('功能调整通知')}</span>
          <DialogTitle className='mt-1.5 text-[22px] font-semibold leading-snug max-sm:text-xl'>
            {t('代理订阅功能已下线')}
          </DialogTitle>
          <DialogDescription className='mt-3 text-xs leading-[1.9] text-subtle'>
            {t('为降低维护成本、资源占用及合规风险，Agent2API 已停止提供代理订阅与内置代理内核相关功能。')}
          </DialogDescription>
          <div className='mt-5 divide-y divide-border overflow-hidden rounded-md border border-border bg-surface-2'>
            <Change retained={false} title={t('不再提供')}>
              {t('订阅导入与更新、订阅节点管理，以及代理内核的下载、安装和运行。')}
            </Change>
            <Change retained title={t('手动代理与 EasyClash 同步仍然保留')}>
              {t('已有手动配置和 EasyClash 端点引用不受影响。仍可添加 HTTP / SOCKS5 地址、同步 EasyClash 端点并测试连通性；EasyClash 由你自行安装和运行。')}
            </Change>
          </div>
          <div className='mt-5 text-xs leading-[1.85]'>
            <strong className='font-semibold'>{t('之前使用订阅节点？')}</strong>
            <p className='mt-1.5 text-muted-foreground'>
              {t('请填写已有代理服务的 HTTP / SOCKS5 地址，或自行运行 EasyClash 后同步端点，并重新为相关账号和服务线路选择代理。')}
            </p>
          </div>
        </DialogBody>
        <DialogFooter className='justify-between px-7 max-sm:px-5'>
          <Label htmlFor={checkboxId} className='inline-flex cursor-pointer items-center gap-2 text-xs font-normal text-subtle'>
            <Checkbox id={checkboxId} checked={hide} onCheckedChange={next => setHide(next === true)} />
            {t('不再提醒')}
          </Label>
          <Button onClick={acknowledge}>{t('我知道了')}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function Change({ retained, title, children }: { retained: boolean; title: string; children: React.ReactNode }) {
  return (
    <div className='flex items-start gap-3 p-4'>
      <span className={`mt-0.5 grid size-[22px] shrink-0 place-items-center rounded-full ${retained ? 'bg-success-soft text-success' : 'bg-border text-subtle'}`}>
        <svg width='13' height='13' viewBox='0 0 24 24' fill='none' stroke='currentColor' strokeWidth='1.8' strokeLinecap='round' aria-hidden='true'>
          <path d={retained ? 'm5 12 4 4L19 6' : 'm6 6 12 12M6 18 18 6'} />
        </svg>
      </span>
      <div><strong className='text-xs font-semibold'>{title}</strong><p className='mt-1 text-xs leading-[1.85] text-subtle'>{children}</p></div>
    </div>
  )
}
