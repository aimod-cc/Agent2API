import * as React from 'react'
import { Button, Card, Spinner } from '@ui'
import { t } from '../i18n'

type Verification = {
  requestId: string
  accountId: string
  accountName: string
  planName: string
  phase: string
}

type VerificationWindow = {
  wbZcodeClaimVerification?: { cancel(): void; status(): Verification | null }
}

/** 常驻提示不抢焦点，阿里云弹出的滑块继续由用户本人操作。 */
export function CheckinVerificationNotice() {
  const api = () => (window as unknown as VerificationWindow).wbZcodeClaimVerification
  const [state, setState] = React.useState<Verification | null>(() => api()?.status() || null)
  React.useEffect(() => {
    const update = () => setState(api()?.status() || null)
    window.addEventListener('zcode-claim-verification', update)
    update()
    return () => window.removeEventListener('zcode-claim-verification', update)
  }, [])
  if (!state) return null
  const submitting = state.phase === 'submitting'
  return <Card className='fixed bottom-5 right-5 z-[2147482900] w-[min(380px,calc(100vw-40px))] space-y-2 p-4 shadow-lg' role='status' aria-live='polite'>
    <div className='flex items-center gap-2 text-sm font-semibold'>
      <Spinner className='size-4 shrink-0' />{t('ZCode 自动领取验证')}
    </div>
    <p className='break-words text-xs text-muted-foreground'>{state.accountName} · {state.planName}</p>
    <p className='text-xs text-muted-foreground'>{submitting
      ? t('验证凭据正在提交，领取结果请查看签到记录')
      : t('正在为本次领取验证；若出现滑块，请手动完成')}</p>
    <Button variant='outline' size='xs' disabled={submitting} onClick={() => api()?.cancel()}>{t('取消验证')}</Button>
  </Card>
}
