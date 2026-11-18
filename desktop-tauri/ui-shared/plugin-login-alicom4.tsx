import * as React from 'react'
import { Alert, Button } from '@ui'

export type Alicom4Config = { engine: 'alicom4'; captchaId: string }

export function Alicom4Captcha({ config, onChange, t }: {
  config: Alicom4Config
  onChange: (value: unknown) => void
  t: (key: string) => string
}) {
  const frame = React.useRef<HTMLIFrameElement>(null)
  const nonce = React.useId()
  const [started, setStarted] = React.useState(false)
  const [failed, setFailed] = React.useState(false)
  React.useEffect(() => {
    const receive = (event: MessageEvent) => {
      if (event.source !== frame.current?.contentWindow || event.origin !== 'null' || event.data?.nonce !== nonce) return
      if (event.data.type === 'ready') { frame.current?.contentWindow?.postMessage({ nonce, type: 'start' }, '*'); return }
      if (event.data.type === 'verified') {
        const value = event.data.value
        const keys = ['lot_number', 'captcha_output', 'pass_token', 'gen_time']
        if (!value || typeof value !== 'object' || Array.isArray(value) || Object.keys(value).length !== keys.length
          || keys.some(key => typeof value[key] !== 'string' || !value[key] || value[key].length > 8192)) {
          onChange(null); setFailed(true); return
        }
        onChange(value); setFailed(false)
      } else if (event.data.type === 'reset' || event.data.type === 'error') {
        onChange(null); setFailed(event.data.type === 'error')
      }
    }
    if (!started) return
    const timeout = window.setTimeout(() => { onChange(null); setFailed(true) }, 180000)
    window.addEventListener('message', receive)
    return () => { window.clearTimeout(timeout); window.removeEventListener('message', receive); onChange(null) }
  }, [nonce, onChange, started])
  if (config.engine !== 'alicom4' || config.captchaId !== 'bb7f810ecc9f6d21dcf72bcae120c8c5') {
    return <Alert variant='destructive'>{t('人机验证地址无效，请重新发起登录')}</Alert>
  }
  const values = JSON.stringify({ id: config.captchaId, nonce }).replace(/</g, '\\u003c')
  const page = `<!doctype html><meta name="referrer" content="no-referrer"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-inline' https://static.alicaptcha.com https://captcha.alicaptcha.com; connect-src https://captcha.alicaptcha.com https://monitor.geetest.com; img-src data: https://static.alicaptcha.com https://captcha.alicaptcha.com; frame-src https://static.alicaptcha.com https://captcha.alicaptcha.com; style-src 'unsafe-inline' https://static.alicaptcha.com; base-uri 'none'; form-action 'none';"><script>const c=${values};const send=(type,value)=>parent.postMessage({nonce:c.nonce,type,value},'*');</script><script src="https://static.alicaptcha.com/v4/ct4.js" onerror="send('error')"></script><script>let widget=null,ready=false,requested=false;const show=()=>{if(!widget||!ready||!requested)return;requested=false;send('reset');try{widget.reset();widget.showCaptcha()}catch{send('error')}};window.addEventListener('message',event=>{if(event.source!==parent||event.data?.nonce!==c.nonce||event.data.type!=='start')return;requested=true;show()});const timeout=setTimeout(()=>send('error'),35000);if(typeof initAlicom4==='function'){initAlicom4({captchaId:c.id,product:'bind',language:'zh',protocol:'https://',https:true,apiServers:['captcha.alicaptcha.com'],staticServers:['static.alicaptcha.com'],onError:()=>send('error')},value=>{widget=value;value.onReady(()=>{clearTimeout(timeout);ready=true;send('ready');show()}).onSuccess(()=>{const raw=value.getValidate();const keys=['lot_number','captcha_output','pass_token','gen_time'];if(!raw||keys.some(key=>raw[key]==null)){send('error');return}const result={};for(const key of keys)result[key]=String(raw[key]);send('verified',result)}).onClose(()=>send('reset')).onError(()=>send('error'))})}else{clearTimeout(timeout);send('error')}</script>`
  return <div className='flex flex-col gap-2'>
    <Button variant='outline' onClick={() => {
      onChange(null); setFailed(false)
      if (started) frame.current?.contentWindow?.postMessage({ nonce, type: 'start' }, '*')
      else setStarted(true)
    }}>{t('打开人机验证')}</Button>
    {started ? <iframe ref={frame} title={t('人机验证')} sandbox='allow-scripts' referrerPolicy='no-referrer'
      className='min-h-80 w-full border-0' srcDoc={page} /> : null}
    {failed ? <Alert variant='destructive'>{t('登录步骤读取失败，请刷新当前步骤或重新发起登录。')}</Alert> : null}
  </div>
}
