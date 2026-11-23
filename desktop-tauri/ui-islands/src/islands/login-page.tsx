/**
 * Agent2API · 登录 / 首次注册页（ui/login.html）—— React 岛。
 *
 * 这一页独立于主面板（没有 index.html 那套骨架，也不属于任何 .page），界面整块由
 * 本岛渲染。但**登录逻辑仍归页面底部那段内联脚本**：它按 id 直接读写
 * #username / #password / #submit / #error / #mode-tip，还把 submit 处理器挂在
 * #form 上（capture 阶段，先于页面里其它 submit 监听）。所以本岛刻意做成
 * 「渲染一次就不再更新」—— 不持 state、不重渲染，脚本对这些节点的命令式改动
 * （按钮文案与禁用、错误行、占位符、提示行）才不会被 React 覆盖回去。
 * 动这个文件之前请先读那段脚本，它是这一页真正的行为来源。
 *
 * 首次提交必须同步（flushSync）：脚本是紧随本岛之后的经典脚本，求值时就要拿到
 * 这些节点（`widget.addEventListener` 拿到 null 会直接抛错，整段脚本作废、点登录
 * 毫无反应）；而 createRoot().render() 默认交给调度器异步提交，同步脚本会跑在提交
 * 之前、拿到空 DOM。代价是本页的 `<script src="islands/login.js">` 必须排在
 * #login-app 之后，且不能加 defer/async —— 加了顺序就散了。
 *
 * 这个岛**不在主面板的 island 注册表里**：它单独构建成 ui/islands/login.js
 * （入口 src/login-entry.tsx，见 vite.login.config.ts），否则登录页为了显示
 * 一张卡片也要下载整份 2.3 MB 的 islands/ui.js。
 *
 * 视觉上不再自己硬编码一套深色变量：颜色一律走组件库令牌（ui.css 的 --ui-*，
 * 跟随 `<html data-theme="dark">`），布局尺寸用 Tailwind 工具类还原原样式。
 * 页面级样式（html/body、第三方 widget）留在 login.html 的 <style> 里。
 */

import * as React from 'react'
import { flushSync } from 'react-dom'
import { createRoot } from 'react-dom/client'
import { Button, Card, Input, Label, Verification } from '@ui'
import { TurnstileWidget } from '../../../ui-shared/turnstile-widget'
import type { PanelCaptchaStatus } from '../../../ui-shared/panel-captcha'
import { t } from '../i18n'
import { useDesktopBranding } from '../branding'
import { BrandLogo } from '../../../ui-shared/brand-logo'

/** 挂载点 id：login.html 里唯一的岛容器 */
const HOST_ID = 'login-app'

/**
 * 机器人校验组件：`<panel-captcha>` 由 ui/panel-captcha.js 定义（协议见
 * server::altcha，文件头注释解释了为什么不用官方 altcha widget —— 它在
 * http://<内网IP> 这类非安全上下文里没有 crypto.subtle，会一直卡在「验证中…」）。
 *
 * 用 createElement 而不是 JSX 标签：TS 的 IntrinsicElements 里没有这个标签，而为一个
 * 自家自定义元素去扩全局 JSX 命名空间会波及其他岛（也可能与并行迁移的文件撞车）。
 * 元素接收登录脚本下发的校验配置；Turnstile 在网关域名下的隔离页面运行。
 */
type CaptchaConfig = PanelCaptchaStatus & { gatewayBase?: string }
type CaptchaElement = HTMLElement & {
  state?: 'idle' | 'verifying' | 'verified' | 'error'
  config?: CaptchaConfig
  start?(): Promise<void>
  acceptToken?(token: string | null): void
  setState?(state: string): void
}

function CaptchaControl() {
  const [state, setState] = React.useState<'idle' | 'verifying' | 'verified' | 'error'>('idle')
  const [config, setConfig] = React.useState<CaptchaConfig | null>(null)
  const [resetKey, setResetKey] = React.useState(0)
  const ref = React.useRef<CaptchaElement | null>(null)
  React.useEffect(() => {
    const element = ref.current
    if (!element) return
    const sync = () => setState(element.getAttribute('state') as typeof state || 'idle')
    const configure = () => setConfig(element.config || null)
    const reset = () => setResetKey(value => value + 1)
    const observer = new MutationObserver(sync)
    observer.observe(element, { attributes: true, attributeFilter: ['state'] })
    element.addEventListener('configuration', configure)
    element.addEventListener('reset', reset)
    sync()
    configure()
    return () => {
      observer.disconnect()
      element.removeEventListener('configuration', configure)
      element.removeEventListener('reset', reset)
    }
  }, [])
  return React.createElement('panel-captcha', { id: 'captcha', ref },
    config?.captchaProvider === 'turnstile' && config.captchaEnabled !== false
      ? <div className='mt-5 w-full'><TurnstileWidget url={`${config.gatewayBase || ''}/api/panel/turnstile`} t={t} resetKey={resetKey}
          onToken={token => ref.current?.acceptToken?.(token)} onState={value => ref.current?.setState?.(value)} /></div>
      : <Verification state={state} className='mx-auto mt-5' onClick={() => { void ref.current?.start?.() }}>
          {state === 'verifying' ? t('验证中…') : state === 'verified' ? t('验证成功') : state === 'error' ? t('验证失败，请重试') : t('我不是机器人')}
        </Verification>,
  )
}

/**
 * 卡片外壳与其中的表单。
 *
 * 五个 id 一个都不能少（见文件头）。#mode-tip 与 #error 渲染成空节点：文案由底部
 * 脚本填，这里只提供位置与占位高度（min-h-5 让错误行出现前后布局不跳）。
 */
function LoginBrand() {
  const branding = useDesktopBranding(t('登录'))
  return <div className='mb-[26px] flex min-w-0 flex-col items-center gap-3 text-center'>
    <BrandLogo branding={branding} size={52} className='rounded-lg' />
    <div className='min-w-0 max-w-full'>
      <h1 className='m-0 break-words text-[21px] font-bold tracking-[0.2px]'>{branding.displayName}</h1>
      <div className='text-[12.5px] text-muted-foreground'>{t('OpenAI 兼容网关 · 管理面板')}</div>
    </div>
  </div>
}

function LoginPage() {
  return (
    <Card className='w-[min(400px,100%)] p-[36px_34px_30px] shadow-3'>
      <LoginBrand />

      <div id='mode-tip' className='mb-[18px] text-center text-[13px] text-muted-foreground' />

      <form id='form' autoComplete='on'>
        <Label htmlFor='username' className='mt-3.5 mb-[5px] text-[12.5px]'>
          {t('账号')}
        </Label>
        <Input id='username' autoComplete='username' placeholder={t('管理员账号')} />
        <Label htmlFor='password' className='mt-3.5 mb-[5px] text-[12.5px]'>
          {t('密码')}
        </Label>
        <Input id='password' type='password' autoComplete='new-password' placeholder={t('至少 8 位')} />
        {/* 领题求解放后台跑 → 绿勾已验证（组件见 ui/panel-captcha.js） */}
        <CaptchaControl />
        {/* type='submit' 必须显式给：Base UI 的 useButton 会给原生 button 补一个
            type="button"，只有显式传入（合并时外部 props 优先）才盖得掉 */}
        <Button id='submit' type='submit' size='lg' className='w-full'>
          {t('继续')}
        </Button>
        <div id='error' className='mt-3 min-h-5 text-center text-[13px] whitespace-pre-wrap text-destructive' />
      </form>

      <div id='oauth-section' style={{ display: 'none' }} className='mt-4'>
        <div className='relative mb-3.5 flex items-center justify-center'>
          <div className='w-full border-t border-border' />
          <span className='bg-card px-2.5 text-[12px] text-muted-foreground whitespace-nowrap'>
            {t('或使用第三方账号登录')}
          </span>
          <div className='w-full border-t border-border' />
        </div>
        <div className='flex flex-col gap-2.5'>
          <Button
            id='oauth-google'
            type='button'
            variant='outline'
            size='lg'
            style={{ display: 'none' }}
            className='w-full justify-center gap-2'
          >
            <svg width='16' height='16' viewBox='0 0 24 24' aria-hidden='true'>
              <path fill='#4285F4' d='M23.745 12.27c0-.7-.06-1.4-.19-2.07H12v4.51h6.6c-.29 1.52-1.14 2.82-2.4 3.68v3.05h3.88c2.27-2.09 3.665-5.17 3.665-9.17Z'/>
              <path fill='#34A853' d='M12 24c3.24 0 5.95-1.08 7.93-2.91l-3.88-3.05c-1.08.72-2.45 1.16-4.05 1.16-3.12 0-5.77-2.11-6.72-4.96H1.29v3.14C3.26 21.3 7.31 24 12 24Z'/>
              <path fill='#FBBC05' d='M5.28 14.24c-.24-.72-.38-1.49-.38-2.24s.14-1.52.38-2.24V6.62H1.29C.47 8.24 0 10.06 0 12s.47 3.76 1.29 5.38l3.99-3.14Z'/>
              <path fill='#EA4335' d='M12 4.75c1.77 0 3.35.61 4.6 1.8l3.42-3.42C17.95 1.19 15.24 0 12 0 7.31 0 3.26 2.7 1.29 6.62l3.99 3.14c.95-2.85 3.6-4.96 6.72-4.96Z'/>
            </svg>
            <span>{t('使用 Google 账号登录')}</span>
          </Button>
          <Button
            id='oauth-github'
            type='button'
            variant='outline'
            size='lg'
            style={{ display: 'none' }}
            className='w-full justify-center gap-2'
          >
            <svg width='16' height='16' viewBox='0 0 24 24' fill='currentColor' aria-hidden='true'>
              <path d='M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61C4.422 18.07 3.633 17.7 3.633 17.7c-1.087-.744.084-.729.084-.729 1.205.084 1.838 1.236 1.838 1.236 1.07 1.835 2.809 1.305 3.495.998.108-.776.417-1.305.76-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.399 3-.405 1.02.006 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12'/>
            </svg>
            <span>{t('使用 GitHub 账号登录')}</span>
          </Button>
        </div>
      </div>

      <div className='mt-[22px] text-center text-[12px] text-muted-foreground'>
        {t('登录后可在「网关 Key」页为 API 客户端创建密钥')}
      </div>
    </Card>
  )
}

/**
 * 挂载。本文件由独立的 login.js 引入，正常只在登录页执行；保留 #login-app
 * 存在性判断是为了防止将来有人把 login.js 误引到其它页面 —— 那种情况下
 * 静默跳过即可，不该报错。
 */
function mount(): void {
  const host = document.getElementById(HOST_ID)
  if (!host) return
  // 同步提交，理由见文件头
  flushSync(() => {
    createRoot(host).render(<LoginPage />)
  })
}

mount()
