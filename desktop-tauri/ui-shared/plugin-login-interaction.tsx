import * as React from 'react'
import { Alert, Button, Input, Label } from '@ui'
import { SearchableSelect } from './searchable-select'

type Interaction = {
  id: string; title: string; message: string
  fields: Array<{ key: string; label: string; type: 'text' | 'select'; secret: boolean; optional: boolean; options: Array<{ value: string; label: string }> }>
  captcha?: { gt: string; challenge: string; success: boolean; new_captcha: boolean } | null
}
type Request = <T>(method: 'GET' | 'POST' | 'PATCH', path: string, body?: unknown) => Promise<T>

function Captcha({ config, onChange, t }: { config: NonNullable<Interaction['captcha']>; onChange: (value: unknown) => void; t: (key: string) => string }) {
  const frame = React.useRef<HTMLIFrameElement>(null)
  const [started, setStarted] = React.useState(false)
  const nonce = React.useId()
  React.useEffect(() => {
    const receive = (event: MessageEvent) => {
      if (event.source !== frame.current?.contentWindow || event.origin !== 'null' || event.data?.nonce !== nonce) return
      if (event.data?.type === 'verified') onChange(event.data.value)
      else if (event.data?.type === 'reset') onChange(null)
    }
    window.addEventListener('message', receive)
    return () => { window.removeEventListener('message', receive); onChange(null) }
  }, [nonce, onChange])
  const data = JSON.stringify({ ...config, nonce }).replace(/</g, '\\u003c')
  const page = `<!doctype html><meta name="referrer" content="no-referrer"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-inline' https://static.geetest.com https://*.geetest.com https://*.geevisit.com; connect-src https://*.geetest.com https://*.geevisit.com; img-src data: https://*.geetest.com https://*.geevisit.com; frame-src https://*.geetest.com https://*.geevisit.com; style-src 'unsafe-inline' https://*.geetest.com https://*.geevisit.com; base-uri 'none'; form-action 'none';"><div id="captcha"></div><script>const c=${data};const send=(type,value)=>parent.postMessage({nonce:c.nonce,type,value},'*');</script><script src="https://static.geetest.com/static/tools/gt.js"></script><script>initGeetest({gt:c.gt,challenge:c.challenge,offline:!c.success,new_captcha:c.new_captcha,product:'embed',https:true},function(widget){widget.appendTo('#captcha');widget.onSuccess(function(){send('verified',widget.getValidate())});widget.onError(function(){send('reset')});widget.onClose(function(){send('reset')})});</script>`
  return <div className='flex flex-col gap-2'>
    {!started ? <Button variant='outline' onClick={() => setStarted(true)}>{t('打开人机验证')}</Button>
      : <iframe ref={frame} title={t('人机验证')} sandbox='allow-scripts' referrerPolicy='no-referrer'
        className='min-h-80 w-full border-0' srcDoc={page} />}
  </div>
}

export function PluginLoginInteraction({ flow, request, t }: { flow: string; request: Request; t: (key: string) => string }) {
  const [interaction, setInteraction] = React.useState<Interaction | null>(null)
  const [values, setValues] = React.useState<Record<string, string>>({})
  const [captcha, setCaptcha] = React.useState<unknown>(null)
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')
  const [captchaGeneration, setCaptchaGeneration] = React.useState(0)
  const latest = React.useRef(request)
  const alive = React.useRef(true)
  const submitting = React.useRef(false)
  const signature = React.useRef('')
  latest.current = request
  const refresh = React.useCallback(async () => {
    if (!flow || submitting.current) return
    try {
      const result = await latest.current<{ interaction: Interaction | null }>('GET', `/api/session/login/interaction?state=${encodeURIComponent(flow)}`)
      if (!alive.current) return
      const next = JSON.stringify(result.interaction)
      if (signature.current !== next) {
        signature.current = next
        setInteraction(result.interaction)
        setValues({}); setCaptcha(null); setError('')
      }
    } catch { /* 终态由现有登录等待链处理。 */ }
  }, [flow])
  React.useEffect(() => {
    alive.current = true
    signature.current = ''
    setInteraction(null); setValues({}); setCaptcha(null)
    void refresh()
    const timer = window.setInterval(() => void refresh(), 1500)
    return () => { alive.current = false; window.clearInterval(timer) }
  }, [refresh])
  async function submit() {
    if (!interaction || submitting.current) return
    submitting.current = true; setBusy(true); setError('')
    try {
      await latest.current('POST', '/api/session/login/interaction', {
        state: flow, interaction: interaction.id, inputs: values, captcha,
      })
      if (alive.current) { setValues({}); setCaptcha(null); setCaptchaGeneration(value => value + 1) }
    } catch (error) {
      if (alive.current) { setError(error instanceof Error ? error.message : String(error)); setCaptcha(null); setCaptchaGeneration(value => value + 1) }
    }
    finally {
      submitting.current = false
      if (alive.current) { setBusy(false); await refresh() }
    }
  }
  if (!flow || !interaction) return null
  return <section className='flex flex-col gap-3 rounded-lg border border-hairline p-3' aria-label={interaction.title}>
    <h3 className='text-sm font-semibold'>{interaction.title}</h3>
    {interaction.message ? <p className='whitespace-pre-wrap text-xs text-muted-foreground'>{interaction.message}</p> : null}
    {interaction.fields.map(field => <Label key={field.key} className='flex flex-col items-start gap-2'>
      {field.label}
      {field.type === 'select' ? <SearchableSelect disabled={busy} value={values[field.key] || ''}
        onValueChange={value => setValues(previous => ({ ...previous, [field.key]: value }))}
        className='w-full' aria-label={field.label} options={field.options} searchable={field.options.length > 8}
        displayValue={field.options.find(item => item.value === values[field.key])?.label || t('请选择')} />
        : <Input disabled={busy} type={field.secret ? 'password' : 'text'} autoComplete='off' spellCheck={false}
        value={values[field.key] || ''} onChange={event => setValues(previous => ({ ...previous, [field.key]: event.currentTarget.value }))} />}
    </Label>)}
    {interaction.captcha ? <Captcha key={`${interaction.id}:${interaction.captcha.challenge}:${captchaGeneration}`} config={interaction.captcha} onChange={setCaptcha} t={t} /> : null}
    {error ? <Alert variant='destructive'>{error}</Alert> : null}
    <Button disabled={busy || Boolean(interaction.captcha && !captcha)} onClick={() => void submit()}>{t('继续')}</Button>
  </section>
}
