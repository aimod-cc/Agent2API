/**
 * 图片兼容设置用到的排版基础件（分组容器、设置行、问号提示、脚注）。
 *
 * 为什么单独一份：面板（settings-vision-panel）与两块设置（settings-vision-way）
 * 都要用同一套行与分组，写在任一边都会形成循环依赖。
 *
 * ── 说明走问号，不铺在行里 ────────────────────────────────────
 * 设置页其它面板（排队等待、图片压缩那一批）的行都只有「标签 + 控件」，
 * 需要解释的地方挂一枚问号、悬停才展开。这里跟它们同一口径：行内 `tip` 而不是
 * `description`，是为了不让每行都撑着两行字把面板拉长。
 */

import * as React from 'react'
import { cn, Label, Switch, Tooltip, TooltipContent, TooltipTrigger } from '@ui'

/** 分组容器：浅色底 + 行间 hairline */
export function Group({ className, children }: { className?: string; children: React.ReactNode }) {
  return <div className={cn('overflow-hidden rounded-md bg-surface-2', className)}>{children}</div>
}

/**
 * 一枚问号：悬停 / 聚焦展开说明，与设置页其它面板的 `.tip-q` 同一套外观。
 *
 * 走组件库的 Tooltip 而不是老的 `data-tip`（desktop-tooltip.tsx）：本面板桌面端与
 * 手机端共用，`data-tip` 只有桌面壳在服务，手机上会只剩一个点不动的圆圈。
 */
export function Tip({ text, label }: { text: string; label: string }) {
  return <Tooltip>
    <TooltipTrigger render={<span className='tip-q' tabIndex={0} aria-label={`${label}：说明`} />}>?</TooltipTrigger>
    <TooltipContent className='max-w-[300px]'>{text}</TooltipContent>
  </Tooltip>
}

/** 分区脚注：容器下方的小灰字说明 */
export function FooterNote({ className, children }: { className?: string; children: React.ReactNode }) {
  return <p className={cn('m-0 max-w-[620px] px-1 text-[12px] leading-relaxed text-muted-foreground', className)}>{children}</p>
}

/** 一行设置：标题在左（需要解释时标题后挂问号），控件在右。控件自己带 aria-label。 */
export function Row({ label, tip, control, first = false, className }: {
  label: React.ReactNode
  /** 行内问号的说明文案；没有就不渲染问号 */
  tip?: string
  control?: React.ReactNode
  first?: boolean
  className?: string
}) {
  return <div className={cn('flex items-start gap-4 px-4 py-3', !first && 'border-t border-hairline', className)}>
    <div className='flex min-w-0 flex-1 flex-wrap items-center gap-2'>
      <Label className='text-[13px] text-foreground'>{label}</Label>
      {tip ? <Tip text={tip} label={typeof label === 'string' ? label : ''} /> : null}
    </div>
    {control ? <div className='flex flex-none items-center gap-2'>{control}</div> : null}
  </div>
}

/** 开关行的语法糖 */
export function SwitchRow({ label, tip, checked, onCheckedChange, disabled, first }: {
  label: string
  tip?: string
  checked: boolean
  onCheckedChange: (next: boolean) => void
  disabled?: boolean
  first?: boolean
}) {
  return <Row first={first} label={label} tip={tip} control={
    <Switch checked={checked} disabled={disabled} aria-label={label}
      onCheckedChange={next => onCheckedChange(next === true)} />
  } />
}

/** 折叠区里的一行「值 + 行尾操作」：备选顺序用它 */
export function ListRow({ index, title, note, actions, first }: {
  index?: number
  title: React.ReactNode
  note?: React.ReactNode
  actions: React.ReactNode
  first?: boolean
}) {
  return <div className={cn('flex items-center gap-3 px-4 py-2.5', !first && 'border-t border-hairline')}>
    {index === undefined ? null : <span className='tabular w-4 flex-none text-[12px] text-muted-foreground'>{index + 1}</span>}
    <span className='min-w-0 flex-1 break-all text-[13px] text-foreground'>{title}</span>
    {note ? <span className='flex-none text-[12px] text-muted-foreground'>{note}</span> : null}
    <span className='flex flex-none items-center gap-1'>{actions}</span>
  </div>
}
