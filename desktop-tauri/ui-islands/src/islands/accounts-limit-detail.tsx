/**
 * 「限流」列上的明细弹层 —— 这个账号**当前限流中的模型**逐行列出：模型名、还要等多久、
 * 恢复时刻与上游给的原因，以及「清除标记」（单条）与「全部清除」。
 *
 * ── 为什么是弹层，而不是行下插一块面板 ────────────────────────
 * 旧版是「点徽章 → 表格里插一整块横跨全表的面板」。三个毛病：
 *   ① 面板宽度由表格决定（真实表约 1330px），内容只占左边一段，右端那片空着的正是
 *      「清除标记」按钮的去处 —— 它离它要清的模型一千多像素远；
 *   ② 每行的原因要与模型名、恢复时刻、按钮挤在同一行，只能靠省略号收口，读全要悬停；
 *   ③ 展开会把整行顶开，扫表时行高忽高忽低，且同一时刻只能开一个账号。
 * 改成与余额列「积分明细」同款的弹层（见 accounts-credit-detail.tsx）之后：表格行高
 * 恒定、弹层悬浮在表格之上，多出来的宽度用来把「还要等多久」和完整的恢复时刻放下。
 *
 * ── 与「积分明细」共用同一套外壳与类名 ───────────────────────
 * 触发器是 `.usage-toggle`（整格可点、悬停有底色、caret 绝对定位、展开态走
 * aria-expanded），内容层是 `.usage-pop-*`（分组头 / 行 / 名称与读数两端对齐）。
 * 两列看起来、点起来是同一件事，用户不必再学第二套。
 *
 * ── 两种时间都给：相对用于急不急，绝对用于到点了没 ─────────────
 * 行右侧是相对倒计时（「还有 14 分钟」），第二行是绝对时刻（「今天 14:20 恢复」）。
 * 旧版列里只有绝对时刻（`最早 {soonest} 恢复`），「还要等多久」得用户自己减；
 * 而日期措辞（今天 / 明天 / M月d日）一旦跨天，光看绝对时刻也答不出急不急。
 * 两处并存才两个问题都答得上 —— 列只有 132px，放不下两行，弹层放得下。
 *
 * ── 冷却进度线读的是**触发时刻**，不是剩余量 ──────────────────
 * 线画的是「这条冷却走了多少」（满格 = 马上恢复），靠落盘里已经有的 `at`
 * （`store_admin::mark_rate_limited` 每次都会写）。这条线**不复用**余额弹层的
 * `.usage-pop-track` —— 那条明确画「剩余」（越长越有余量），两者方向相反，
 * 共用一个元素迟早会读错。`at` 缺失（旧版本落盘的记录）或与 resetAt 不成区间时
 * 就不画（`progress: null`），与余额明细「画不出比例就不装作有」同一处置。
 */

import * as React from 'react'
import { Button, Popover, PopoverContent, PopoverTrigger } from '@ui'
import { t } from '../i18n'
import { activeLimits, formatResetText, RESET_UNKNOWN } from './accounts-domain'
import type { AccountRecord } from './accounts-shared'

/** 明细里的一行（`limitRowsOf` 的产物） */
export type LimitRow = {
  /** 被限流的模型名（后端给的键，原样透出） */
  model: string
  /** 右侧读数：相对倒计时；恢复时刻读不到时是「恢复时间未知」 */
  wait: string
  /** 读数语义档：正常有恢复时刻 = warn，读不到 = muted */
  tone: 'warn' | 'muted'
  /** 第二行左半：`今天 14:20 恢复`；读不到时为空串 */
  reset: string
  /** 第二行右半：上游给的原因（可能为空） */
  reason: string
  /** 悬停全文（相对 + 绝对 + 原因 + 这条标记怎么解除） */
  title: string
  /** 冷却进度（0–100）；读不到触发时刻时 null = 不画线 */
  progress: number | null
}

/**
 * 剩余毫秒 → 倒计时文案。
 *
 * 分档按「读的人要拿它做什么」定：一小时内给分钟（这段时间是要盯着它回来的），
 * 一天内给小时 + 分钟，超过一天只给天（那时分钟是噪音）。取整用 `Math.round`：
 * 「还有 1 分钟」比「还有 0 分钟」有意义，而后者会让已经等了 30 秒的人以为该好了。
 */
function waitText(remainingMs: number): string {
  const minutes = Math.round(remainingMs / 60000)
  if (minutes < 1) return t('不到 1 分钟')
  if (minutes < 60) return t('还有 {n} 分钟', { n: minutes })
  const hours = Math.floor(minutes / 60)
  const rest = minutes % 60
  if (hours < 24) {
    return rest ? t('还有 {h} 小时 {m} 分', { h: hours, m: rest }) : t('还有 {h} 小时', { h: hours })
  }
  return t('还有 {d} 天', { d: Math.floor(hours / 24) })
}

/**
 * 账号**当前生效**的限流记录 → 明细行。
 *
 * 顺序照 `activeLimits` 给的（按恢复时间升序，最早恢复的排最前）：先回来的排最前，
 * 与列上那句「最早 … 恢复」说的是同一条记录，两处不会对不上。
 */
export function limitRowsOf(account: AccountRecord | null | undefined): LimitRow[] {
  const now = Date.now()
  return activeLimits(account).map(entry => {
    const resetAt = Number(entry.resetAt)
    const started = Number(entry.at)
    const reset = formatResetText(entry.resetAt)
    const unknown = reset === RESET_UNKNOWN
    const reason = entry.message || (entry.status ? t('上游返回 {status}', { status: entry.status }) : '')
    const span = resetAt - started
    const progress = started > 0 && Number.isFinite(span) && span > 0
      ? Math.max(0, Math.min(100, ((now - started) / span) * 100))
      : null
    const wait = unknown ? t('恢复时间未知') : waitText(resetAt - now)
    const resetLine = unknown ? '' : t('{time} 恢复', { time: reset })
    return {
      model: entry.model,
      wait,
      tone: unknown ? 'muted' : 'warn',
      reset: resetLine,
      reason,
      title: [
        entry.model,
        wait,
        resetLine,
        reason,
        t('到恢复时间后自动解除，无需手动操作'),
      ].filter(Boolean).join(' · '),
      progress,
    }
  })
}

/**
 * 限流列的触发器 + 明细弹层。
 *
 * 触发器整格可点（`.usage-toggle` 向两侧出血到格子边缘），格下那行小字「最早 … 恢复」
 * 留在触发器**外面** —— 它是常显的补充读数，不该跟着按钮一起被点。
 *
 * `soonest` 由调用方用 `formatResetText(entries[0].resetAt)` 算好传进来（那一份口径
 * 在 accounts-domain，列与弹层共用一条记录，见 `limitRowsOf` 的顺序说明）。
 */
export function LimitDetailButton({ accountName, rows, count, soonest, onClear }: {
  accountName: string
  rows: LimitRow[]
  count: number
  soonest: string
  onClear: (model: string) => void
}) {
  const [open, setOpen] = React.useState(false)

  /**
   * 初始焦点落在滚动容器上而不是「第一个可聚焦的元素」，与积分明细同一处置
   * （理由见 accounts-credit-detail.tsx：库的默认行为会把焦点给第一个可聚焦元素、
   * 浏览器再把它滚进视野，列表一长就"一开就停在中间"）。容器带 `tabIndex={-1}`。
   */
  const boxRef = React.useRef<HTMLDivElement | null>(null)
  const attachBox = React.useCallback((node: HTMLDivElement | null) => {
    boxRef.current = node
    if (node) node.scrollTop = 0
  }, [])
  const initialFocus = React.useCallback(() => boxRef.current ?? null, [])

  const earliest = soonest === RESET_UNKNOWN ? t('待定') : soonest

  return (
    <>
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger
          render={<button type='button' className='usage-toggle warn'
            title={t('点击查看各模型的限流明细')} aria-label={t('限流明细')} />}
        >
          <span className='val'>{t('{n} 个模型', { n: count })}</span>
          <span className='caret' aria-hidden>▾</span>
        </PopoverTrigger>
        {/* `align='center'`：与「积分明细」同一处置（理由见 accounts-credit-detail
            的同名参数）—— 这两列的按钮都靠右，'end' 会把弹层甩到表格右外侧。 */}
        <PopoverContent align='center' sideOffset={6} className='w-[320px]' initialFocus={initialFocus}>
          {/* ⚠️ 这一层 div 是**滚动容器本身**，不是装饰：组件库给 PopoverContent 编出的
              `overflow-hidden` / `max-h-[min(70vh,560px)]` 都带 `!important`，把
              max-height / overflow-y 写在弹层自己身上会被压掉。与 .usage-pop-credit
              同一处置，见 page-accounts-table.css 里那两处说明。 */}
          <div className='usage-pop-limit' ref={attachBox} tabIndex={-1}>
            {accountName ? <div className='usage-pop-sub'>{accountName}</div> : null}
            <div className='usage-pop-group'>{t('限流明细')}</div>
            {rows.map(row => (
              <div className='usage-pop-row' key={row.model} title={row.title}>
                <div className='usage-pop-line'>
                  <span className='usage-pop-name lp-pop-model'>{row.model}</span>
                  <span className={`usage-pop-view ${row.tone}`}>{row.wait}</span>
                </div>
                <div className='lp-pop-subline'>
                  <span className='lp-pop-reason'>
                    {row.reset ? `${row.reset} · ` : ''}{row.reason}
                  </span>
                  <Button variant='ghost' size='xs'
                    title={t('清掉本机的限流标记，立刻重新尝试该模型（上游若仍在限流会再次被标记）')}
                    onClick={() => onClear(row.model)}>{t('清除')}</Button>
                </div>
                {row.progress !== null ? (
                  <span className='lp-pop-track'>
                    <i style={{ width: `${row.progress}%` }}
                      aria-label={t('冷却进度 {percent}%', { percent: Math.round(row.progress) })} />
                  </span>
                ) : null}
              </div>
            ))}
            <div className='lp-pop-foot'>
              <span className='lp-pop-note'>{t('{n} 个模型限流中 · 到恢复时间自动解除', { n: count })}</span>
              <Button variant='outline' size='xs' title={t('清掉该账号全部模型的限流标记')}
                onClick={() => onClear('')}>{t('全部清除')}</Button>
            </div>
          </div>
        </PopoverContent>
      </Popover>
      <span className='lim-sub' title={t('最早恢复')}>
        {t('最早 {soonest} 恢复', { soonest: earliest })}
      </span>
    </>
  )
}
