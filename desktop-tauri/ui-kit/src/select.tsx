import * as React from 'react'
import { Select as BaseSelect } from '@base-ui/react/select'
import { cn } from './lib/cn'

/**
 * 下拉选择。
 *
 * 视觉对齐 ui/css/select.css：触发器 30px 高、控件底、1px 控件描边、--r-md 圆角、
 * 12.5px 字号、无投影（与同行的 input 齐平）；浮层 z-35、--r-lg 圆角、
 * 浮层底（--ui-raised）+ 强描边 + --shadow-3，选项 28px 高、选中走品牌浅底。
 *
 * 与既有 select.js 的关系：那个是「增强原生 `<select>`」——保留原生控件当唯一
 * 数据源与宽度锚，只为未迁移的页面服务。组件库这份是完整自绘（Base UI 的 Select），
 * 键盘输入筛选、方向键、typeahead、`aria-activedescendant` 全内建，
 * 新界面直接用它；既有页面等迁移时再换，两者可以并存。
 *
 * 用法：
 *   <Select value={v} onValueChange={setV}>
 *     <SelectTrigger><SelectValue /></SelectTrigger>
 *     <SelectContent>
 *       <SelectItem value="a">选项 A</SelectItem>
 *     </SelectContent>
 *   </Select>
 */

const Select = BaseSelect.Root
const SelectValue = BaseSelect.Value
const SelectGroup = BaseSelect.Group
/** 分组标题：shadcn 里叫 SelectLabel（Base UI 原语叫 GroupLabel） */
const SelectLabel = BaseSelect.GroupLabel
const SelectSeparator = BaseSelect.Separator

type SelectTriggerProps = Omit<React.ComponentProps<typeof BaseSelect.Trigger>, 'className'> & {
  className?: string
}

function SelectTrigger({ className, children, ...props }: SelectTriggerProps) {
  return (
    <BaseSelect.Trigger
      data-slot='select-trigger'
      className={cn(
        'inline-flex h-[30px] min-w-0 cursor-pointer items-center justify-between gap-1.5 rounded-md border border-control-border bg-control px-2.5',
        // 选中值（SelectValue 渲染出的 span）是触发器的 flex 子项，默认
        // min-width:auto 不收缩：标签比触发器宽时会把边框撑破、文字溢出去
        // （账号表代理列实测）。truncate 把 overflow 变 hidden，自动最小尺寸
        // 随之归零，过长标签省略号收口 —— 完整值在展开的浮层与触发器的 title 里。
        // 这条选择器会连箭头图标的 span 一起命中，truncate 不改 display，对它无影响。
        '[&>span]:min-w-0 [&>span]:truncate',
        'text-[12.5px] font-normal text-foreground',
        'transition-colors duration-150 ease-out',
        'hover:border-control-border-hover hover:bg-control-hover',
        'focus-visible:border-primary focus-visible:shadow-focus focus-visible:outline-none',
        'data-disabled:cursor-not-allowed data-disabled:bg-control data-disabled:text-muted-foreground data-disabled:opacity-60',
        className
      )}
      {...props}
    >
      {children}
      <BaseSelect.Icon className='flex flex-none text-muted-foreground transition-transform duration-[180ms] data-popup-open:rotate-180'>
        <svg viewBox='0 0 10 6' className='size-2.5' aria-hidden='true'>
          <path d='M1 1l4 4 4-4' stroke='currentColor' strokeWidth='1.4' fill='none' strokeLinecap='round' strokeLinejoin='round' />
        </svg>
      </BaseSelect.Icon>
    </BaseSelect.Trigger>
  )
}

type SelectContentProps = Omit<React.ComponentProps<typeof BaseSelect.Popup>, 'className'> & {
  className?: string
  /** 与触发器的间距 */
  sideOffset?: number
}

function SelectContent({ className, sideOffset = 4, children, ...props }: SelectContentProps) {
  return (
    <BaseSelect.Portal>
      {/* alignItemWithTrigger={false}：关闭 Base UI 默认的「选中项对齐触发器」定位。
          那个模式由组件自己接管弹层的高度与 top/bottom（fixed + 从视口顶边起算），
          与我们在 Popup 上写的 max-h 固定上限互相打架：当选中项落在长列表深处、
          触发器又靠近视口底部时（账号页代理列实测 —— 轮询账号的补位项排在
          40+ 项的末尾），弹层会被钉到视口顶部，离触发器十万八千里。
          关掉后回到标准锚定定位：贴着触发器下方弹出、空间不够向上翻转，
          弹层内部自行滚动到选中项。官方对默认模式的说明与关闭方式见
          base-ui.com/react/components/select 的 Positioning 一节。 */}
      <BaseSelect.Positioner sideOffset={sideOffset} alignItemWithTrigger={false} className='z-[35]'>
        <BaseSelect.Popup
          data-slot='select-content'
          className={cn(
            'max-h-[260px] min-w-[var(--anchor-width)] overflow-y-auto overscroll-contain rounded-lg border border-border-strong bg-raised p-[5px] shadow-3',
            'outline-none',
            // 只淡入不做位移/缩放：打开后要立刻量尺寸摆位置，transform 会污染坐标
            'data-open:animate-in data-open:fade-in-0',
            'data-closed:animate-out data-closed:fade-out-0',
            className
          )}
          {...props}
        >
          {children}
        </BaseSelect.Popup>
      </BaseSelect.Positioner>
    </BaseSelect.Portal>
  )
}

type SelectItemProps = Omit<React.ComponentProps<typeof BaseSelect.Item>, 'className'> & {
  className?: string
}

function SelectItem({ className, children, ...props }: SelectItemProps) {
  return (
    <BaseSelect.Item
      data-slot='select-item'
      className={cn(
        'flex min-h-7 cursor-pointer items-center gap-2 rounded-sm px-[9px] text-[12.5px] leading-[1.4] text-foreground select-none',
        'transition-colors duration-[120ms] ease-out',
        'data-[highlighted]:bg-nav-hover',
        // 用 data-[selected] 而不是 shadcn 的 data-selected 变体：后者只认
        // [data-selected="true"]，而 Base UI 输出的是 data-selected=""（属性存在
        // 即选中），空串匹配不上，选中项会静默失去底色与字重。
        'data-[selected]:bg-primary-soft data-[selected]:font-semibold data-[selected]:text-primary-fg',
        'data-[highlighted]:data-[selected]:bg-primary-tint',
        'data-disabled:pointer-events-none data-disabled:font-normal data-disabled:text-muted-foreground',
        className
      )}
      {...props}
    >
      <BaseSelect.ItemText className='min-w-0 flex-1 truncate'>{children}</BaseSelect.ItemText>
      <BaseSelect.ItemIndicator className='flex flex-none text-primary'>
        <svg viewBox='0 0 12 12' className='size-3' aria-hidden='true'>
          <path d='M2.5 6.2 4.8 8.5 9.5 3.8' stroke='currentColor' strokeWidth='1.8' fill='none' strokeLinecap='round' strokeLinejoin='round' />
        </svg>
      </BaseSelect.ItemIndicator>
    </BaseSelect.Item>
  )
}

export {
  Select,
  SelectTrigger,
  SelectValue,
  SelectContent,
  SelectItem,
  SelectGroup,
  SelectLabel,
  SelectSeparator,
}
