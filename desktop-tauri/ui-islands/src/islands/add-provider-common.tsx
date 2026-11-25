import * as React from 'react'
import { toast } from './add-account-bridge'
import { t } from '../i18n'

/** 手填按钮的忙态：全局一把锁（旧实现的 addBusy 就是模块级，弹窗内互斥） */
let addBusy = false

export async function runAdd(setBusy: (value: boolean) => void, task: () => Promise<void>): Promise<void> {
  if (addBusy) return
  addBusy = true
  setBusy(true)
  try {
    await task()
  } finally {
    addBusy = false
    setBusy(false)
  }
}

/** 带行内标记的说明：调用方给的是可信常量（与旧实现的 innerHTML 同口径），纯文本走文本节点 */
export function Note({ html, text }: { html?: string; text?: string }): React.ReactElement {
  if (html) return <p dangerouslySetInnerHTML={{ __html: html }} />
  return <p>{text || ''}</p>
}

/** 批量添加的失败明细最多列这么多条，其余只体现在总数里（提示框塞不下长清单） */
const MAX_FAILURE_LINES = 5

/**
 * 批量添加 / 导入后的失败汇总：总数，加上逐条的「是哪一份、为什么」。
 * `added` 给出时把成功数一并写进来——面板同一时刻只显示一条提示，分两条发时成功那条会被这条顶掉。
 * 提示框不保留换行，所以明细用分号隔开，而不是一行一条。
 */
export function toastFailures(lines: string[], added?: number): void {
  const shown = lines.slice(0, MAX_FAILURE_LINES)
  if (lines.length > shown.length) shown.push('…')
  const summary = added === undefined
    ? t('导入失败 {count} 个：', { count: lines.length })
    : t('已添加 {added} 个，导入失败 {count} 个：', { added, count: lines.length })
  toast(summary + shown.join('；'), 'err')
}

