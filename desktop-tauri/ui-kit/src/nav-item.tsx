import * as React from 'react'
import { Button } from './button'
import { cva, type VariantProps } from 'class-variance-authority'
import { useRender } from '@base-ui/react/use-render'
import { useSelectionIndicator } from './lib/selection-indicator'
import { cn } from './lib/cn'

const NavListContext = React.createContext(false)

function NavList({ className, render, children, ...props }: useRender.ComponentProps<'div'>) {
  const { containerRef, indicator } = useSelectionIndicator(
    '[data-slot="nav-item"]', '[data-slot="nav-item"][data-active]',
  )
  const element = useRender({
    defaultTagName: 'div', render, ref: containerRef,
    props: {
      ...props,
      'data-slot': 'nav-list',
      className: cn('relative isolate flex min-h-0 flex-col gap-0.5', className),
      children: <>
        <span data-slot='nav-list-indicator' aria-hidden='true' {...indicator} />
        {children}
      </>,
    },
  })
  return <NavListContext.Provider value>{element}</NavListContext.Provider>
}

/**
 * 导航 / 列表项（项目扩展，shadcn 标准里没有对应件）。
 *
 * 用 Button 承接交互，集中维护导航选中态、图标与计数的布局。
 *
 * 形态取自侧边栏的 `.nav-item` 与模型管理页左栏的 `.pv`（两者本来就是同一形态）：
 * 行高由 padding 撑、圆角 --r-md —— 条目是 30px 高，与同样 30px 高的普通按钮
 * 取同一档，用户说的「侧栏和按钮圆角不一样」就是这两处（阶梯见 styles/theme.css）。
 *
 * 三种变体：
 *   · default  普通条目（可选中）
 *   · add      虚线描边的「＋ 新建…」入口：它不是一家、点了也不会有选中态，
 *              虚线正是为了与上面那些真条目分开
 *
 * 注意：条目本身是 `<button>`，**里面不能再嵌按钮**。要在右侧挂一枚独立的
 * 操作按钮（如删除 ×），请由调用方在它外面套一层定位容器、把那枚按钮放成兄弟节点
 * —— HTML 不允许 button 嵌套，硬塞进去浏览器会把内层拆出来。
 */

const navItemVariants = cva(
  [
    'relative flex h-auto w-full items-center justify-start gap-2.5 whitespace-normal text-left active:translate-y-0',
    'border border-transparent rounded-md',
    'shadow-none',
    'transition-[background-color,border-color,color] duration-150 ease-out',
    'outline-none focus-visible:shadow-focus',
    'disabled:pointer-events-none disabled:opacity-45',
    '[&_svg]:pointer-events-none [&_svg]:shrink-0',
  ],
  {
    variants: {
      variant: {
        default: [
          'cursor-pointer px-2.5 py-2',
          'text-[12.5px] font-medium text-sidebar-fg',
          'hover:bg-nav-hover hover:text-foreground',
          // 选中态：主色实心块（底色 / 描边 / 文字色三个取值统一在
          // styles/nav.css 与 theme.css 的 --ui-nav-selected-* 里，见那里的说明）。
          // 字重只从 400 到 500：再加粗会让整行宽度跳一档。
          'data-[active]:border-nav-selected-border data-[active]:bg-nav-selected-bg data-[active]:font-medium data-[active]:text-nav-selected-fg',
        ],
        add: [
          'cursor-pointer justify-center px-2 py-1.5',
          'border-dashed border-border-strong bg-transparent',
          'text-[11.5px] text-muted-foreground',
          'hover:border-primary hover:text-primary-fg',
        ],
      },
    },
    defaultVariants: { variant: 'default' },
  }
)

type NavItemProps = Omit<React.ComponentProps<'button'>, 'className'> &
  VariantProps<typeof navItemVariants> & {
    /** 选中态。写成 data-active 而不是类名切换，是为了让 Tailwind 变体能命中它 */
    active?: boolean
    /** 右侧计数（家数 / 条数）；传 undefined 不渲染 */
    count?: number | string
    /** 左侧图标 */
    icon?: React.ReactNode
    /** 右侧附加内容（计数之外的东西，如状态点） */
    trailing?: React.ReactNode
    className?: string
  }

function NavItem({
  className,
  variant,
  active = false,
  count,
  icon,
  trailing,
  children,
  ...props
}: NavItemProps) {
  const sliding = React.useContext(NavListContext)
  return (
    <Button
      type='button'
      variant='ghost'
      data-slot='nav-item'
      data-active={active ? '' : undefined}
      data-sliding={sliding ? '' : undefined}
      aria-current={active ? 'true' : undefined}
      className={cn(navItemVariants({ variant }), sliding && 'data-[active]:border-transparent data-[active]:bg-transparent data-[active]:hover:bg-transparent', className)}
      {...props}
    >
      {icon}
      <span data-slot='nav-item-label' className='min-w-0 flex-1 truncate'>
        {children}
      </span>
      {trailing}
      {count !== undefined && count !== null && (
        <span
          data-slot='nav-item-count'
          className={cn(
            'flex-none text-[10.5px] text-sidebar-muted [font-variant-numeric:tabular-nums]',
            // 选中态下计数跟随文字色，否则它在深色底上会糊成一团
            active && 'text-inherit'
          )}
        >
          {count}
        </span>
      )}
    </Button>
  )
}

export { NavList, NavItem, navItemVariants, type NavItemProps }
