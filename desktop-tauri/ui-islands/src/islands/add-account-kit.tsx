/**
 * Agent2API · 「登录 / 添加账号」弹窗里共用的两个排版件。
 *
 * 这一版把弹窗的排版统一成 macOS 设置面板那一套（分组名在框外、框内细线分行、
 * 主按钮收进底栏），各家表单块都按这套拼。放在单独文件里的原因：它被
 * add-provider-blocks / add-dynamic-provider / add-custom-provider 三处共用，
 * 挂在其中任何一个上都会让另外两个反向依赖。
 *
 * ── 为什么底栏要用 portal ────────────────────────────────
 * 登录引擎（web-login.js / sms-login.js / autoclaw-oauth.js）全部按 **DOM id**
 * 取按钮与提示节点（`document.getElementById`），不看父子结构 —— 所以把按钮从
 * 段落里搬到弹窗底栏不影响任何一条链路。DeepSeek 那一屏早就是这么做的
 * （见 add-provider-deepseek-block 的 footerHost），这里只是把它推广成通用件。
 *
 * `active` 必须与所在段落的可见性同一个判据：portal 出去的内容不在那个
 * `hidden` 子树里了，段落收起时若还渲染，底栏就会挂着一颗属于别的方式的按钮。
 */

import * as React from 'react'
import { createPortal } from 'react-dom'

/**
 * 一组设置：分组名在方框外，框内各行以 1px 细线分隔（样式在 styles/add-account-dialog.css）。
 * `action` 是分组名右侧的附加件；`hidden` 与原先 DialogSection 的同名属性等价。
 */
export function AddGroup({ label, action, hidden, onClick, children }: {
  label: React.ReactNode
  action?: React.ReactNode
  hidden?: boolean
  /** 整段的委托点击（小浣熊那条「登录态文件在哪」的链接就挂在段上） */
  onClick?: React.MouseEventHandler<HTMLElement>
  children: React.ReactNode
}): React.ReactElement {
  return (
    <section className='add-group' hidden={hidden} onClick={onClick}>
      <div className='add-group-label'>
        <span>{label}</span>
        {action ? <span className='label-action'>{action}</span> : null}
      </div>
      <div className='add-group-box'>{children}</div>
    </section>
  )
}

/**
 * 段落的主按钮与说明：搬到弹窗底栏（`host`），说明占左侧、按钮占右侧。
 *
 * ── 为什么是「常驻但隐藏」而不是「按需挂载」────────────────
 * sms-login.js 与 autoclaw-oauth.js 都在 `create()` 时按 id 给按钮绑 click
 * （`$(\`${prefix}-sms-submit\`).addEventListener(...)`），而各段落是整块常驻、
 * 只切显隐的。若这一段不生效时干脆不渲染，节点就不存在，`create()` 那一次绑定
 * 会静默落空 —— 用户后来切到「手机验证码」时按钮是死的。所以：整块始终 portal
 * 过去，只给外层加 `hidden`，节点一直在，绑定成立，同时底栏里只看得见当前这一组。
 *
 * `host` 为空时退回原位渲染（不该发生，但别让动作消失）。
 */
export function AddFoot({ host, note, active = true, children }: {
  host: HTMLElement | null
  note?: React.ReactNode
  active?: boolean
  children: React.ReactNode
}): React.ReactElement {
  const content = <>
    <div className='acct-add-foot-note'>{note}</div>
    <div className='acct-add-foot-buttons'>{children}</div>
  </>
  if (!host) return <div className='acct-add-foot-inline'>{content}</div>
  return createPortal(<div className='acct-add-foot-slot' hidden={!active}>{content}</div>, host)
}
