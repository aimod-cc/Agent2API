export const ALICOM4_CODAR_CAPTCHA_ID = 'bb7f810ecc9f6d21dcf72bcae120c8c5'

export type Alicom4Config = { engine?: 'alicom4'; type?: 'alicom4'; captchaId: string }
export type GeetestConfig = { gt: string; challenge: string; success: boolean; new_captcha: boolean }

export function isAlicom4Config(config: Alicom4Config | GeetestConfig): config is Alicom4Config {
  return 'captchaId' in config
}

export function isValidAlicom4Config(config: Alicom4Config): boolean {
  const kind = config.engine === 'alicom4' || config.type === 'alicom4'
  return kind && config.captchaId === ALICOM4_CODAR_CAPTCHA_ID
}

export function parseAlicom4Result(value: unknown): Record<string, string> | null {
  const keys = ['lot_number', 'captcha_output', 'pass_token', 'gen_time'] as const
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  const record = value as Record<string, unknown>
  if (Object.keys(record).length !== keys.length) return null
  const result: Record<string, string> = {}
  for (const key of keys) {
    const field = record[key]
    if (typeof field !== 'string' || !field || field.length > 8192) return null
    result[key] = field
  }
  return result
}

export function parseGeetestResult(value: unknown): Record<string, string> | null {
  const keys = ['geetest_challenge', 'geetest_validate', 'geetest_seccode'] as const
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  const record = value as Record<string, unknown>
  const result: Record<string, string> = {}
  for (const key of keys) {
    const field = record[key]
    if (typeof field !== 'string' || !field || field.length > 2048) return null
    result[key] = field
  }
  return result
}

const FRAME_STYLE = 'html,body{margin:0;padding:0;width:100%;height:100%;overflow:hidden;background:transparent}'

function buildCaptchaFramePage({
  csp,
  config,
  scriptSrc,
  body = '',
  initScript,
}: {
  csp: string
  config: Record<string, unknown>
  scriptSrc: string
  body?: string
  initScript: string
}): string {
  const values = JSON.stringify(config).replace(/</g, '\\u003c')
  return [
    '<!doctype html>',
    '<meta name="referrer" content="no-referrer">',
    `<meta http-equiv="Content-Security-Policy" content="${csp}">`,
    `<style>${FRAME_STYLE}</style>`,
    body,
    `<script>const c=${values};const send=(type,value)=>parent.postMessage({nonce:c.nonce,type,value},'*');</script>`,
    `<script src="${scriptSrc}" onerror="send('error')"></script>`,
    `<script>${initScript}</script>`,
  ].join('')
}

export function buildAlicom4Page(captchaId: string, nonce: string): string {
  return buildCaptchaFramePage({
    csp: "default-src 'none'; script-src 'unsafe-inline' https://static.alicaptcha.com https://captcha.alicaptcha.com; connect-src https://captcha.alicaptcha.com https://monitor.geetest.com; img-src data: https://static.alicaptcha.com https://captcha.alicaptcha.com; frame-src https://static.alicaptcha.com https://captcha.alicaptcha.com; style-src 'unsafe-inline' https://static.alicaptcha.com; base-uri 'none'; form-action 'none';",
    config: { id: captchaId, nonce },
    scriptSrc: 'https://static.alicaptcha.com/v4/ct4.js',
    initScript: "let widget=null,ready=false,requested=false;const show=()=>{if(!widget||!ready||!requested)return;requested=false;try{widget.reset();widget.showCaptcha()}catch{send('error')}};window.addEventListener('message',event=>{if(event.source!==parent||event.data?.nonce!==c.nonce||event.data.type!=='start')return;requested=true;show()});const timeout=setTimeout(()=>send('error'),35000);if(typeof initAlicom4==='function'){initAlicom4({captchaId:c.id,product:'bind',language:'zh',protocol:'https://',https:true,apiServers:['captcha.alicaptcha.com'],staticServers:['static.alicaptcha.com'],onError:()=>send('error')},value=>{widget=value;value.onReady(()=>{clearTimeout(timeout);ready=true;send('ready');show()}).onSuccess(()=>{const raw=value.getValidate();const keys=['lot_number','captcha_output','pass_token','gen_time'];if(!raw||keys.some(key=>raw[key]==null)){send('error');return}const result={};for(const key of keys)result[key]=String(raw[key]);send('verified',result)}).onClose(()=>send('close')).onError(()=>send('error'))})}else{clearTimeout(timeout);send('error')}",
  })
}

export function buildGeetestPage(config: GeetestConfig, nonce: string): string {
  return buildCaptchaFramePage({
    csp: "default-src 'none'; script-src 'unsafe-inline' https://static.geetest.com https://*.geetest.com https://*.geevisit.com; connect-src https://*.geetest.com https://*.geevisit.com; img-src data: https://*.geetest.com https://*.geevisit.com; frame-src https://*.geetest.com https://*.geevisit.com; style-src 'unsafe-inline' https://*.geetest.com https://*.geevisit.com; base-uri 'none'; form-action 'none';",
    config: { ...config, nonce },
    scriptSrc: 'https://static.geetest.com/static/tools/gt.js',
    body: '<div id="captcha" style="display:flex;align-items:center;justify-content:center;width:100%;height:100%"></div>',
    initScript: "let widget=null,ready=false,requested=false;const show=()=>{if(!widget||!ready||!requested)return;requested=false;try{if(typeof widget.verify==='function')widget.verify();else widget.appendTo('#captcha')}catch{send('error')}};window.addEventListener('message',event=>{if(event.source!==parent||event.data?.nonce!==c.nonce||event.data.type!=='start')return;requested=true;show()});const timeout=setTimeout(()=>send('error'),35000);if(typeof initGeetest==='function'){initGeetest({gt:c.gt,challenge:c.challenge,offline:!c.success,new_captcha:c.new_captcha,product:'bind',width:'100%',https:true},function(value){widget=value;value.onReady(function(){clearTimeout(timeout);ready=true;send('ready');show()});value.onSuccess(function(){const raw=value.getValidate();if(!raw){send('error');return}send('verified',raw)});value.onError(function(){send('error')});value.onClose(function(){send('close')})})}else{clearTimeout(timeout);send('error')}",
  })
}

export function safeHttpsUrl(value: string): string {
  try {
    const url = new URL(value)
    return url.protocol === 'https:' && !url.username && !url.password ? url.href : ''
  } catch {
    return ''
  }
}

export function safeLoginQrUrl(value: string): string {
  try {
    const url = new URL(value)
    return url.protocol === 'https:' && !url.username && !url.password && !url.hash
      && ['mp.weixin.qq.com', 'api.kuaipaper.com', 'api.claw.speedai.com'].includes(url.hostname)
      ? url.href
      : ''
  } catch {
    return ''
  }
}

export function formatSmsNotice(message: string, t: (key: string) => string): string {
  if (message === '检测到非常用网络环境，请完成二次人机验证并获取验证码') {
    return t('检测到非常用网络环境，请完成二次人机验证并获取验证码')
  }
  return t(message)
}
