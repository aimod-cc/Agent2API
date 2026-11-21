import * as React from 'react'
import { Button, Label, Textarea } from '@ui'
import { t } from '../ui-kit/src/i18n'

export const SecretTextarea = React.forwardRef<HTMLTextAreaElement, React.ComponentProps<typeof Textarea> & {
  label: React.ReactNode
  secret?: boolean
  resetKey?: string
}>(function SecretTextarea({ label, secret = true, resetKey, id, className, ...props }, ref) {
  const generatedId = React.useId()
  const inputId = id || generatedId
  const [revealed, setRevealed] = React.useState(false)
  React.useEffect(() => { setRevealed(false) }, [resetKey])
  return <div className='plugin-login-field'>
    <div className='plugin-login-field-label'>
      <Label htmlFor={inputId}>{label}</Label>
      {secret ? <Button type='button' variant='ghost' size='sm' className='plugin-login-reveal' disabled={props.disabled}
        aria-controls={inputId} aria-pressed={revealed} onClick={() => setRevealed(previous => !previous)}>
        <svg width='16' height='16' viewBox='0 0 24 24' fill='none' stroke='currentColor' strokeWidth='1.8' aria-hidden='true'>
          <path d='M2 12s3.6-7 10-7 10 7 10 7-3.6 7-10 7S2 12 2 12Z' /><circle cx='12' cy='12' r='3' />
          {revealed ? <path d='m3 3 18 18' /> : null}
        </svg>{revealed ? t('隐藏内容') : t('显示内容')}
      </Button> : null}
    </div>
    <Textarea {...props} id={inputId} ref={ref} className={`plugin-login-textarea ${secret && !revealed ? 'plugin-login-concealed' : ''} ${className || ''}`} />
  </div>
})
