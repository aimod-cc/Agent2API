import * as React from 'react'
import { Button } from './button'
import { cn } from './lib/cn'

/**
 * 表格（标准形态，与 shadcn/ui 的 Table 一族一致：Table / TableHeader / TableBody /
 * TableFooter / TableHead / TableRow / TableCell / TableCaption）。
 *
 * 样式对齐 ui/css/page-gateway.css 的 `.models-table`：12.5px 正文字、表头粘顶
 * （sticky）、表头 10.5px 加字距、单元格 8px 14px 内边距、行底发丝线、悬停行底色。
 * 换到组件库后这些成为默认值，业务侧不必再挂 .models-table。
 *
 * 外层 Table 自带一个横向滚动容器（shadcn 同款做法）：窄窗口下宽表不会被压扁或
 * 溢出，而是横滚。表格本身 `w-full`，`table-layout` 默认 auto —— 需要定宽列时
 * 由业务侧用 `<colgroup>` + `table-fixed` 控制（既有页面正是这么做的）。
 *
 * 列宽拖动（table-columns.js）作用于 `<col>` 的 inline width，与这里的默认值
 * 不冲突：组件不写 col 宽度，谁也不覆盖谁。
 */

function Table({ className, containerClassName, container = true, ...props }: React.ComponentProps<'table'> & {
  containerClassName?: string
  /** 已有滚动宿主或表头拖拽依赖直接子节点时，不再增加滚动层。 */
  container?: boolean
}) {
  const table = <table data-slot='table' className={cn('w-full caption-bottom border-collapse text-[12.5px]', className)} {...props} />
  return container
    ? <div data-slot='table-container' className={cn('relative w-full overflow-auto', containerClassName)}>{table}</div>
    : table
}

function TableHeader({ className, ...props }: React.ComponentProps<'thead'>) {
  return <thead data-slot='table-header' className={cn(className)} {...props} />
}

function TableBody({ className, ...props }: React.ComponentProps<'tbody'>) {
  return <tbody data-slot='table-body' className={cn(className)} {...props} />
}

function TableFooter({ className, ...props }: React.ComponentProps<'tfoot'>) {
  return (
    <tfoot
      data-slot='table-footer'
      className={cn('border-t border-border bg-surface-2 font-medium', className)}
      {...props}
    />
  )
}

function TableRow({ className, ...props }: React.ComponentProps<'tr'>) {
  return (
    <tr
      data-slot='table-row'
      className={cn(
        // 悬停行底色：整行 td 一起变，所以打在格子上的类要跟着行状态走
        'transition-colors duration-100 hover:[&>td]:bg-control-hover',
        className
      )}
      {...props}
    />
  )
}

type TableSortDirection = 'asc' | 'desc'
type TableHeadProps = React.ComponentProps<'th'> & {
  sortDirection?: TableSortDirection | false
  onSortChange?: (direction: TableSortDirection) => void
  sortLabel?: string
}

function TableHead({ className, children, sortDirection = false, onSortChange, sortLabel, ...props }: TableHeadProps) {
  return (
    <th
      data-slot='table-head'
      className={cn(
        'sticky top-0 z-[2] whitespace-nowrap border-b border-border bg-[var(--ui-table-head-bg)] px-2 py-2',
        'text-left text-[10.5px] font-semibold tracking-[.05em] text-muted-foreground',
        className
      )}
      {...props}
      aria-sort={onSortChange
        ? (sortDirection === 'asc' ? 'ascending' : sortDirection === 'desc' ? 'descending' : 'none')
        : props['aria-sort']}
    >
      {onSortChange ? (
        <Button type='button' variant='ghost' size='xs' data-table-sort=''
          className='h-auto max-w-full gap-1 border-0 px-0 py-0 align-middle [font-size:inherit] font-semibold tracking-[inherit] text-inherit hover:bg-transparent active:translate-y-0'
          aria-label={sortLabel} title={[sortLabel, props.title].filter(Boolean).join('\n') || undefined}
          onClick={() => onSortChange(sortDirection === 'asc' ? 'desc' : 'asc')}>
          <span className='min-w-0 truncate'>{children}</span>
          <svg viewBox='0 0 24 24' className='size-3' aria-hidden='true'
            fill='none' stroke='currentColor' strokeWidth='2' strokeLinecap='round' strokeLinejoin='round'>
            <path d='m8 9 4-4 4 4' className={sortDirection === 'asc' ? 'text-primary-fg' : 'opacity-35'} />
            <path d='m8 15 4 4 4-4' className={sortDirection === 'desc' ? 'text-primary-fg' : 'opacity-35'} />
          </svg>
        </Button>
      ) : children}
    </th>
  )
}

function TableCell({ className, ...props }: React.ComponentProps<'td'>) {
  return (
    <td
      data-slot='table-cell'
      className={cn('border-b border-hairline px-2 py-2 align-middle', className)}
      {...props}
    />
  )
}

function TableCaption({ className, ...props }: React.ComponentProps<'caption'>) {
  return (
    <caption
      data-slot='table-caption'
      className={cn('mt-4 text-xs text-muted-foreground', className)}
      {...props}
    />
  )
}

export {
  Table, TableHeader, TableBody, TableFooter, TableHead, TableRow, TableCell, TableCaption,
  type TableHeadProps, type TableSortDirection,
}
