import * as React from 'react'
import { Button } from './button'
import { cn } from './lib/cn'
import { t } from './i18n'

/**
 * 分页器（项目扩展，shadcn 标准里没有对应件）。
 *
 * 为什么另起一个而不是照搬 shadcn 的 Pagination：那个是「链接式页码条」
 * （一组带 href 的页码链接，按页码跳转），适合静态站点；本项目两处用分页的地方
 * （请求日志、运行日志）都是 **offset 真分页 + 后端只给总数**，
 * 界面形态是「上一页 / 下一页 / 第 N / M 页」，页码条既没有信息量、
 * 也会在几千页时铺满一行。
 *
 * 所以这里只封这一种形态：两颗按钮 + 一行读数。按钮的禁用逻辑（首页不能退、
 * 末页不能进）也一并收进来，免得每个调用点各写一遍边界判断。
 *
 * 视觉对齐 ui/css 的 `.log-pager` / `.log-pager-info`：两颗 sm 档 outline 按钮，
 * 读数用等宽数字（tabular-nums），翻页时数字宽度不跳。
 *
 * 用法：
 *   <Pager page={page} pageCount={pageCount} onPageChange={setPage} disabled={loading} />
 */

type PagerProps = {
  /** 当前页，1 起数 */
  page: number
  /** 总页数（至少 1；调用方算不出来时传 1） */
  pageCount: number
  onPageChange: (page: number) => void
  /** 整体禁用（例如请求在途）：两颗按钮都不响应 */
  disabled?: boolean
  /** 读数前的文案，默认「第」；整句形如「第 1 / 3 页」 */
  className?: string
  /** 读数右对齐时用（面板底栏里读数在按钮右边） */
  infoFirst?: boolean
}

function Pager({
  page,
  pageCount,
  onPageChange,
  disabled = false,
  className,
  infoFirst = false,
}: PagerProps) {
  // 页数兜底：后端还没回话时 pageCount 可能是 0 或 NaN，别让它变成「第 1 / 0 页」
  const total = Math.max(1, Number(pageCount) || 1)
  const current = Math.min(Math.max(1, Number(page) || 1), total)

  const info = (
    <span
      data-slot='pager-info'
      // text-subtle（--text-2）而不是 muted-foreground（--text-3）：读数与旁边的
      // 说明文字是同一档，用更淡的那档在浅色主题下会糊
      className='text-[11.5px] text-subtle [font-variant-numeric:tabular-nums]'
    >
      {t('第 {current} / {total} 页', { current, total })}
    </span>
  )

  return (
    <div data-slot='pager' className={cn('flex items-center gap-2', className)}>
      {infoFirst && info}
      <Button
        type='button'
        variant='outline'
        size='sm'
        disabled={disabled || current <= 1}
        onClick={() => onPageChange(current - 1)}
      >
        {t('上一页')}
      </Button>
      <Button
        type='button'
        variant='outline'
        size='sm'
        disabled={disabled || current >= total}
        onClick={() => onPageChange(current + 1)}
      >
        {t('下一页')}
      </Button>
      {!infoFirst && info}
    </div>
  )
}

export { Pager, type PagerProps }
