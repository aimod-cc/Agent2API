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
 * 之前、拿到空 DOM。代价是本页的 `<script src="islands/ui.js">` 必须排在 #login-app
 * 之后，且不能加 defer/async —— 加了顺序就散了。
 *
 * 视觉上不再自己硬编码一套深色变量：颜色一律走组件库令牌（ui.css 的 --ui-*，
 * 跟随 `<html data-theme="dark">`），布局尺寸用 Tailwind 工具类还原原样式。
 * 页面级样式（html/body、第三方 widget）留在 login.html 的 <style> 里。
 */

import * as React from 'react'
import { flushSync } from 'react-dom'
import { createRoot } from 'react-dom/client'
import { Button, Input, Label } from '@ui'
import { t } from '../i18n'

/** 挂载点 id：login.html 里唯一的岛容器 */
const HOST_ID = 'login-app'

/**
 * 机器人校验组件：`<panel-captcha>` 由 ui/panel-captcha.js 定义（协议见
 * server::altcha，文件头注释解释了为什么不用官方 altcha widget —— 它在
 * http://<内网IP> 这类非安全上下文里没有 crypto.subtle，会一直卡在「验证中…」）。
 *
 * 用 createElement 而不是 JSX 标签：TS 的 IntrinsicElements 里没有这个标签，而为一个
 * 自家自定义元素去扩全局 JSX 命名空间会波及其他岛（也可能与并行迁移的文件撞车）。
 * 元素只认 id：领题端点、文案、事件都在组件内部，这里不传任何属性。
 */
const captchaWidget = React.createElement('panel-captcha', { id: 'captcha' })

/**
 * 卡片外壳与其中的表单。
 *
 * 五个 id 一个都不能少（见文件头）。#mode-tip 与 #error 渲染成空节点：文案由底部
 * 脚本填，这里只提供位置与占位高度（min-h-5 让错误行出现前后布局不跳）。
 */
function LoginPage() {
  return (
    <div className='w-[min(400px,100%)] rounded-lg border border-border bg-card p-[36px_34px_30px] shadow-3'>
      {/* 品牌标与桌面端 / 面板侧栏同一造型（icons.js 的 brand 项），底色是品牌色常量而非主题令牌 */}
      <div className='mb-[26px] flex flex-col items-center gap-3 text-center'>
        <svg
          viewBox='0 0 24 24'
          role='img'
          aria-label='Agent2API'
          className='size-[52px] rounded-[12px] shadow-[0_6px_18px_rgba(0,122,255,0.35)]'
        >
          <rect width='24' height='24' rx='5.4' fill='#007AFF' />
          <g fill='none' stroke='#fff' strokeWidth='2.2' strokeLinecap='round' strokeLinejoin='round'>
            <path d='M5.23 9.24h10.71' />
            <path d='M15.94 6.72 18.77 9.24 15.94 11.76' />
            <path d='M18.77 14.76H8.06' />
            <path d='M8.06 12.24 5.23 14.76 8.06 17.28' />
          </g>
        </svg>
        <div>
          <h1 className='m-0 text-[21px] font-bold tracking-[0.2px]'>Agent2API</h1>
          <div className='text-[12.5px] text-muted-foreground'>{t('OpenAI 兼容网关 · 管理面板')}</div>
        </div>
      </div>

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
        {captchaWidget}
        {/* type='submit' 必须显式给：Base UI 的 useButton 会给原生 button 补一个
            type="button"，只有显式传入（合并时外部 props 优先）才盖得掉 */}
        <Button id='submit' type='submit' size='lg' className='w-full'>
          {t('继续')}
        </Button>
        <div id='error' className='mt-3 min-h-5 text-center text-[13px] whitespace-pre-wrap text-destructive' />
      </form>

      <div className='mt-[22px] text-center text-[12px] text-muted-foreground'>
        {t('登录后可在「网关 Key」页为 API 客户端创建密钥')}
      </div>
    </div>
  )
}

/**
 * 挂载。主面板里没有 #login-app（岛随同一个 bundle 一起加载），直接跳过 ——
 * 这是预期行为，不是漏挂。
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
