import * as React from 'react'
import {
  Alert,
  Button,
  Dialog,
  DialogBody,
  DialogContent,
  DialogHeader,
  DialogTitle,
  Spinner,
} from '@ui'
import { setCaptchaBusy } from './plugin-captcha-busy'
import { useCaptchaPopupSession } from './plugin-login-captcha-session'

export function PluginCaptchaDialog({
  buildPage,
  parseResult,
  onChange,
  disabled = false,
  t,
}: {
  buildPage: (nonce: string) => string
  parseResult?: (value: unknown) => unknown
  onChange: (value: unknown) => void
  disabled?: boolean
  t: (key: string) => string
}) {
  const owner = React.useMemo(() => Symbol('plugin-captcha-dialog'), [])
  const onBusyChange = React.useCallback((busy: boolean) => {
    setCaptchaBusy(owner, busy)
  }, [owner])

  const {
    frame,
    nonce,
    open,
    ready,
    verified,
    failed,
    attempt,
    start,
    dismiss,
  } = useCaptchaPopupSession({ onChange, parseResult, onBusyChange })

  const page = open ? buildPage(nonce) : ''

  return (
    <div className='flex flex-col gap-2'>
      <Button
        type='button'
        variant={verified ? 'secondary' : 'outline'}
        disabled={disabled}
        onClick={start}
      >
        {verified ? t('人机验证已通过') : t('打开人机验证')}
      </Button>
      {failed ? <Alert variant='destructive'>{t('登录步骤读取失败，请刷新当前步骤或重新发起登录。')}</Alert> : null}
      <Dialog
        open={open}
        onOpenChange={(next, details) => {
          // Clicking inside the sandboxed iframe blurs the parent document; ignore focus-out so solving doesn't dismiss the popup.
          if (!next && details.reason === 'focus-out') {
            details.cancel()
            return
          }
          if (!next) dismiss()
        }}
      >
        <DialogContent overlayForceRender className='w-[min(460px,calc(100vw-24px))] gap-0'>
          <DialogHeader>
            <DialogTitle>{t('人机验证')}</DialogTitle>
          </DialogHeader>
          <DialogBody className='relative overflow-hidden p-0'>
            {!ready ? (
              <div className='pointer-events-none absolute inset-0 z-10 flex flex-col items-center justify-center gap-2 bg-surface text-xs text-muted-foreground'>
                <Spinner />
                <span>{t('正在进行人机验证…')}</span>
              </div>
            ) : null}
            {open ? (
              <iframe
                key={attempt}
                ref={frame}
                title={t('人机验证')}
                sandbox='allow-scripts'
                referrerPolicy='no-referrer'
                className='block h-[min(480px,calc(100vh-140px))] w-full border-0 bg-transparent'
                srcDoc={page}
              />
            ) : null}
          </DialogBody>
        </DialogContent>
      </Dialog>
    </div>
  )
}
