/**
 * 图片兼容设置用到的排版基础件（分组容器、设置行、脚注）。
 *
 * 为什么单独一份：面板（settings-vision-panel）与模型选择/单独配置弹窗
 * （settings-vision-editors）都要用同一套行与分组，写在任一边都会形成循环依赖。
 *
 * 这里只有排版与令牌，没有任何控件与业务判断 —— 控件一律来自组件库
 * （Switch / Label / Button / Select…），组的形态见 CSS 说明：
 * 一块浅色底的圆角容器，行与行之间只留一条 hairline，说明走分区脚注。
 */

import * as React from 'react'
import { cn, Label, Switch } from '@ui'

/** 分组容器：浅色底 + 行间 hairline */
export function Group({ className, children }: { className?: string; children: React.ReactNode }) {
  return <div className={cn('overflow-hidden rounded-md bg-surface-2', className)}>{children}</div>
}

/** 分区标题：容器上方的小灰字 */
export function GroupTitle({ className, children }: { className?: string; children: React.ReactNode }) {
  return <div className={cn('px-1 text-[12px] text-muted-foreground', className)}>{children}</div>
}

/** 分区脚注：容器下方的小灰字说明 */
export function FooterNote({ className, children }: { className?: string; children: React.ReactNode }) {
  return <p className={cn('m-0 max-w-[620px] px-1 text-[12px] leading-relaxed text-muted-foreground', className)}>{children}</p>
}

/** 一行设置：标题在左（可带一行说明），控件在右。控件自己带 aria-label。 */
export function Row({ label, description, control, first = false, className }: {
  label: React.ReactNode
  description?: React.ReactNode
  control?: React.ReactNode
  first?: boolean
  className?: string
}) {
  return <div className={cn('flex items-start gap-4 px-4 py-3', !first && 'border-t border-hairline', className)}>
    <div className='min-w-0 flex-1'>
      <div className='flex flex-wrap items-center gap-2'>
        <Label className='text-[13px] text-foreground'>{label}</Label>
      </div>
      {description ? <p className='m-0 mt-1 max-w-[520px] text-[12px] leading-relaxed text-muted-foreground'>{description}</p> : null}
    </div>
    {control ? <div className='flex flex-none items-center gap-2'>{control}</div> : null}
  </div>
}

/** 开关行的语法糖 */
export function SwitchRow({ label, description, checked, onCheckedChange, disabled, first }: {
  label: string
  description?: string
  checked: boolean
  onCheckedChange: (next: boolean) => void
  disabled?: boolean
  first?: boolean
}) {
  return <Row first={first} label={label} description={description} control={
    <Switch checked={checked} disabled={disabled} aria-label={label}
      onCheckedChange={next => onCheckedChange(next === true)} />
  } />
}

/** 折叠区里的一行「值 + 行尾操作」：备选顺序、单独配置都用它 */
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

/**
 * 结果预览行：把「这套设置实际会发生什么」用一句人话写在配置下面。
 * 这是整块面板里最该被读懂的一行，所以给它一层比容器略深的底。
 */
export function OutcomeRow({ first = false, children }: { first?: boolean; children: React.ReactNode }) {
  return <div className={cn('flex items-start gap-3 bg-surface-3 px-4 py-3', !first && 'border-t border-hairline')}>
    <span className='min-w-0 flex-1 text-[12.5px] leading-relaxed text-muted-foreground'>{children}</span>
  </div>
}
