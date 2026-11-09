import * as React from 'react'
import {
  Avatar, AvatarFallback, AvatarImage, Button, Collapsible, CollapsibleContent,
  CollapsibleTrigger, Item, ItemActions, cn,
} from '@ui'

import { useProviderIcon } from './provider-icon'
import { t } from '../i18n'

export function ProviderLogo({ id, label }: { id: string; label: string }): React.ReactElement {
  const { src: icon, onError } = useProviderIcon(id)
  const initial = (label || id || '·').trim().slice(0, 1).toUpperCase() || '·'
  return (
    <Avatar className='size-6 rounded-[7px] bg-control' title={label} aria-hidden='true'>
      <AvatarFallback className='text-[10px] font-bold'>{initial}</AvatarFallback>
      <AvatarImage keepMounted src={icon || undefined} alt='' loading='lazy' className='object-contain'
        onLoadingStatusChange={status => { if (status === 'error') onError() }} />
    </Avatar>
  )
}

/** 展开箭头：展开时转 90°（与账号页同一观感） */
function ChevronIcon({ open }: { open: boolean }): React.ReactElement {
  return (
    <span
      className={cn('inline-flex flex-none text-muted-foreground transition-transform duration-150', open && 'rotate-90')}
      aria-hidden='true'
    >
      <svg viewBox='0 0 24 24' width='14' height='14' fill='none' stroke='currentColor'
        strokeWidth='1.8' strokeLinecap='round' strokeLinejoin='round'>
        <path d='M9.5 5.5 16 12l-6.5 6.5' />
      </svg>
    </span>
  )
}

type CheckinRowProps = {
  provider: string
  name: string
  description?: React.ReactNode
  summary?: React.ReactNode
  actions?: React.ReactNode
  note?: React.ReactNode
  expandable: boolean
  expanded: boolean
  onExpandedChange: (open: boolean) => void
  children: React.ReactNode
}

/**
 * 签到中心里可展开的一行（每日签到 / 新手福利 / 成长任务共用）。
 *
 * ── 一行只放这个账号自己的差异 ─────────────────────────────
 * 行上是「图标 + 名称 + 一行说明 + 右侧状态与操作」；这一家通用的规则、以及
 * 「点击行展开」这类操作提示，都不再挂在行里，而是走 `description`（这一家的
 * 读数）与卡片脚注。明细收进展开区的一块凹槽里，缩进到与行标题对齐。
 *
 * ── 交互仍然交给 Collapsible ───────────────────────────────
 * 行主体是 CollapsibleTrigger（渲染成 div，键盘可达性由 tabIndex 兜住），
 * 右侧的 summary / actions 是它的**兄弟节点** —— 行里带按钮，就不能把整行
 * 包成一个 <button>（HTML 不允许按钮嵌套）。
 */
export function CheckinRow({
  provider, name, description, summary, actions, note,
  expandable, expanded, onExpandedChange, children,
}: CheckinRowProps): React.ReactElement {
  const open = expandable && expanded
  return (
    <Collapsible open={open} onOpenChange={onExpandedChange} disabled={!expandable}>
      <Item className={cn('gap-0 p-0', expandable && 'cursor-pointer')}>
        <CollapsibleTrigger
          nativeButton={false}
          render={<Button variant='ghost' disabled={!expandable} nativeButton={false} render={<div />} />}
          tabIndex={expandable ? 0 : -1}
          className='h-auto min-w-0 flex-1 shrink items-center justify-start gap-3 rounded-none border-0 bg-transparent px-3.5 py-3 text-left whitespace-normal font-normal shadow-none hover:bg-transparent active:translate-y-0 data-disabled:cursor-default'
        >
          <ProviderLogo id={provider} label={name} />
          <div className='min-w-0 flex-1'>
            <div className='text-[13px] font-medium leading-snug'>{t(name)}</div>
            {description ? (
              <div className='mt-0.5 text-[12px] leading-relaxed text-muted-foreground'>
                {typeof description === 'string' ? t(description) : description}
              </div>
            ) : null}
          </div>
        </CollapsibleTrigger>
        {summary || actions || expandable ? (
          <ItemActions className='gap-2.5 pr-3.5'>
            {summary}
            {actions}
            {expandable ? <ChevronIcon open={open} /> : null}
          </ItemActions>
        ) : null}
      </Item>
      {note ? (
        <div className='border-t border-hairline bg-surface-2 px-3.5 py-2 text-[11.5px] leading-relaxed text-muted-foreground empty:hidden'>
          {note}
        </div>
      ) : null}
      <CollapsibleContent>
        {/* 凹槽缩进 50px = 行内边距 14 + 图标 24 + 间距 12，与行标题左对齐 */}
        <div className='mx-3.5 mb-2.5 ml-[50px] rounded-md bg-surface-2 px-3 py-1.5'>{children}</div>
      </CollapsibleContent>
    </Collapsible>
  )
}
