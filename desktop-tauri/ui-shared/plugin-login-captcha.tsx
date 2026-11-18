import * as React from 'react'
import { Button } from '@ui'

export type GeetestConfig = { gt: string; challenge: string; success: boolean; new_captcha: boolean }

export function Captcha({ config, onChange, t }: { config: GeetestConfig; onChange: (value: unknown) => void; t: (key: string) => string }) {
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
