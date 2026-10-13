import * as React from 'react'
import {
  Input,
  Pager,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@ui'
import { t } from '../i18n'

/**
 * Agent2API · **通用表格外壳**：统一的页脚分页栏 + 客户端分页状态。
 *
 * 本次把「表格」这件事收成两半：
 *   · 列宽拖动 / 横向滚动 —— 在 ui/table-columns.js（纯 DOM 层，四种表共用，
 *     见那边的模块头：拖动可以拖出容器，装不下就横向滚动）；
 *   · 页脚分页栏 —— 在本文件（React 层，五张表共用）。
 * 两半各自只有一份实现，行为不再按页漂。
 *
 * ── 页脚里有什么（用户口径）────────────────────────────────
 *   共 N 条 · 当前第 a–b 条   ← 「显示当前多少条、一共多少条」
 *   每页 [50 ▾] 条            ← 每页显示多少条
 *   跳至 [ 3 ] 页             ← 跳转到第几页
 *   [上一页] [下一页] 第 x / y 页（组件库的 Pager）
 *
 * 跳页框常驻（哪怕只有一页）：口径统一、位置固定，翻页器禁用即可 ——
 * 时有时无的控件比一个灰着的框更让人困惑。
 *
 * ── 两种分页口径 ───────────────────────────────────────────
 *   · **客户端分页**（事件日志 / 模型管理 / 网关 Key / 账号）：数据一次全取回，
 *     本文件负责页码状态与切片（useClientPaging），「每页条数」里可以选「全部」。
 *   · **服务端真分页**（请求日志）：明细条数无上限，offset/limit 由后端执行，
 *     页码状态在页面自己手里（本文件只渲染页脚）；不给「全部」这一档
 *     （那会把几十万行塞进 DOM）。
 *
 * ── 每页条数是**每张表各自记住**的 ───────────────────────────
 * 键 `workbuddy-desktop-table-size:<表名>`：各表数据量差别大（账号十来条、
 * 请求日志几十万），共用一个值只会互相添乱。
 */

/** 每页条数：数字，或 'all'（不分页，一次全渲染） */
export type PageSizeChoice = number | 'all'

/** 不提供「全部」的档位（服务端真分页的表用，见文件头） */
export const SERVER_PAGE_SIZES: PageSizeChoice[] = [20, 50, 100, 200]

/** 客户端分页的默认档位（多一档「全部」） */
export const CLIENT_PAGE_SIZES: PageSizeChoice[] = [20, 50, 100, 200, 'all']

/** 默认档位（数字字面量：服务端真分页的调用方要拿它做算术） */
export const DEFAULT_PAGE_SIZE = 50

const SIZE_KEY_PREFIX = 'workbuddy-desktop-table-size:'

/** 档位的显示文案：「全部」是唯一的非数字档 */
export function pageSizeText(size: PageSizeChoice): string {
  return size === 'all' ? t('全部') : String(size)
}

/**
 * 读某张表上次选的每页条数。
 *
 * 传进来的 `sizes` 是这张表**当前允许**的档位：存盘里的值已经不在档位里
 * （比如某张表后来去掉了「全部」）就回落默认值 —— 否则会出现「下拉里没有
 * 这一项、表格却按它分页」这种没法解释的状态。`fallback` 是「没存过」时用哪一档
 * （账号页给出「全部」：那是队列，行序本身就是数据）。
 */
export function readPageSize(
  table: string,
  sizes: PageSizeChoice[] = CLIENT_PAGE_SIZES,
  fallback: PageSizeChoice = DEFAULT_PAGE_SIZE,
): PageSizeChoice {
  if (!sizes.includes(fallback)) fallback = sizes.includes(DEFAULT_PAGE_SIZE) ? DEFAULT_PAGE_SIZE : sizes[0]
  let raw: string | null = null
  try {
    raw = localStorage.getItem(SIZE_KEY_PREFIX + table)
  } catch {
    return fallback
  }
  if (raw === null || raw.trim() === '') return fallback
  if (raw === 'all') return sizes.includes('all') ? 'all' : fallback
  const value = Number(raw)
  return Number.isFinite(value) && sizes.includes(value) ? value : fallback
}

export function writePageSize(table: string, size: PageSizeChoice): void {
  try {
    localStorage.setItem(SIZE_KEY_PREFIX + table, String(size))
  } catch { /* 隐私模式等存不了就算了：本次会话内仍然生效 */ }
}

/** useClientPaging 的返回值：切片区间用**1 起的闭区间**（读数与用户口径一致） */
export type ClientPaging = {
  size: PageSizeChoice
  /** 当前页（1 起，已按总页数夹过） */
  page: number
  pageCount: number
  /** 本页第一条在整份数据里的序号（1 起）；空数据为 0 */
  rangeStart: number
  /** 本页最后一条的序号；空数据为 0 */
  rangeEnd: number
  /** 是不是「全部」档（不分页） */
  paged: boolean
  setSize: (size: PageSizeChoice) => void
  goto: (page: number) => void
  /** 按当前页切片；「全部」档原样返回 */
  slice: <T>(rows: readonly T[]) => T[]
}

/**
 * 客户端分页状态。数据一次全取回的表都用它 —— 页码、每页条数（含持久化）、
 * 越界夹取三件事都在这里，页面只需要 `slice(rows)` 与把返回值交给 TableFooter。
 *
 * `total` 变化（换筛选、搜索）导致当前页越界时**自动夹回**最后一页：否则用户会
 * 停在一片空白上，还以为「没有数据」。
 */
export function useClientPaging(
  total: number,
  table: string,
  options: { sizes?: PageSizeChoice[]; defaultSize?: PageSizeChoice } = {},
): ClientPaging {
  const sizes = options.sizes ?? CLIENT_PAGE_SIZES
  const fallback = options.defaultSize ?? DEFAULT_PAGE_SIZE
  const [size, setSizeState] = React.useState<PageSizeChoice>(() => readPageSize(table, sizes, fallback))
  const [page, setPage] = React.useState(1)

  const pageCount = Math.max(1, size === 'all' ? 1 : Math.ceil(total / size))
  const current = Math.min(Math.max(1, page), pageCount)

  React.useEffect(() => {
    if (current !== page) setPage(current)
  }, [current, page])

  const setSize = React.useCallback((next: PageSizeChoice) => {
    setSizeState(next)
    writePageSize(table, next)
    // 换档位回到第一页：停在第 5 页改成每页 200 条，看到的会是数据尾部，没人想要
    setPage(1)
  }, [table])

  const goto = React.useCallback((next: number) => {
    setPage(Math.min(Math.max(1, Math.round(next) || 1), pageCount))
  }, [pageCount])

  const start = size === 'all' ? 0 : (current - 1) * size
  const end = size === 'all' ? total : Math.min(total, start + size)

  const slice = React.useCallback(
    <T,>(rows: readonly T[]): T[] => (size === 'all' ? rows.slice() : rows.slice(start, end)),
    [size, start, end],
  )

  return {
    size,
    page: current,
    pageCount,
    rangeStart: total ? start + 1 : 0,
    rangeEnd: total ? end : 0,
    paged: size !== 'all',
    setSize,
    goto,
    slice,
  }
}

type TableFooterProps = {
  /** 数据总条数（**筛选后**的条数：用户看到的列表就是它） */
  total: number
  page: number
  pageCount: number
  size: PageSizeChoice
  sizes?: PageSizeChoice[]
  onSizeChange: (size: PageSizeChoice) => void
  onPageChange: (page: number) => void
  /** 本页显示的行区间（1 起闭区间）；不分页（「全部」档）传 null */
  range?: { start: number; end: number } | null
  /** 请求在途时把控件禁掉（客户端分页的表通常不需要） */
  disabled?: boolean
  /** 页脚左侧的说明文字：沿用各页原有的提示，不因为加了控件就删掉 */
  leading?: React.ReactNode
  className?: string
}

/**
 * 统一的表格页脚。用法见各表（请求日志是服务端口径，其余是 useClientPaging）。
 *
 * 布局：说明文字在左、控件组在右（`.spacer` 顶开）；说明长到一行放不下时，
 * 控件组整组换到第二行（`.panel-foot` 本就 flex-wrap，见 layout.css）。
 */
function TableFooter({
  total,
  page,
  pageCount,
  size,
  sizes = CLIENT_PAGE_SIZES,
  onSizeChange,
  onPageChange,
  range = null,
  disabled = false,
  leading,
  className,
}: TableFooterProps) {
  // 跳页框是**草稿态**：输入过程中不翻页，回车 / 失焦才提交（边打字边翻页会把
  // 中间态（1 → 12 → 123）也发出去，请求日志那种真分页的表还会打三次接口）
  const [draft, setDraft] = React.useState(String(page))
  React.useEffect(() => setDraft(String(page)), [page])

  const commit = (): void => {
    const text = draft.trim()
    if (!text) { setDraft(String(page)); return }
    const next = Number(text)
    if (!Number.isFinite(next)) { setDraft(String(page)); return }
    const clamped = Math.min(Math.max(1, Math.round(next)), pageCount)
    setDraft(String(clamped))
    if (clamped !== page) onPageChange(clamped)
  }

  const single = pageCount <= 1
  const countText = total === 0
    ? t('共 0 条')
    : range
      ? t('共 {total} 条 · 当前第 {start}–{end} 条', { total, start: range.start, end: range.end })
      : t('共 {total} 条（全部显示）', { total })

  return (
    <div className={className ? `panel-foot table-foot-bar ${className}` : 'panel-foot table-foot-bar'}>
      {leading}
      <div className='spacer' />
      <div className='table-foot-ctl'>
        <span className='table-foot-count'>{countText}</span>
        <span className='table-foot-size'>
          {t('每页')}
          <Select
            value={String(size)}
            onValueChange={next => onSizeChange(next === 'all' ? 'all' : Number(next))}
          >
            <SelectTrigger className='h-[26px] w-[86px]' disabled={disabled} aria-label={t('每页条数')}>
              <SelectValue>{pageSizeText(size)}</SelectValue>
            </SelectTrigger>
            <SelectContent>
              {sizes.map(choice => (
                <SelectItem key={String(choice)} value={String(choice)}>{pageSizeText(choice)}</SelectItem>
              ))}
            </SelectContent>
          </Select>
          {t('条')}
        </span>
        <span className='table-foot-jump'>
          {t('跳至')}
          <Input
            className='h-[26px] w-[52px] px-1 text-center'
            value={draft}
            disabled={disabled || single}
            aria-label={t('跳转到指定页')}
            title={single ? t('当前只有一页') : t('跳转到第 1–{pageCount} 页', { pageCount })}
            inputMode='numeric'
            onChange={event => setDraft(event.target.value)}
            onKeyDown={event => {
              if (event.key === 'Enter') { event.preventDefault(); commit() }
              // 退出编辑：把草稿还原成真实页码（取消输入）
              else if (event.key === 'Escape') setDraft(String(page))
            }}
            onBlur={commit}
          />
          {t('页')}
        </span>
        <Pager page={page} pageCount={pageCount} onPageChange={onPageChange} disabled={disabled} />
      </div>
    </div>
  )
}

export { TableFooter }
