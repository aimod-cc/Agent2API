import * as React from 'react'
import { Checkbox as CheckboxPrimitive } from '@base-ui/react/checkbox'
import { cn } from './lib/cn'

/**
 * 复选框（标准形态，与 shadcn/ui 的 Checkbox 一致）。
 *
 * 自绘（Base UI 的 Checkbox + 令牌配色），不再是原生 `<input type=checkbox>`：
 * 原生框的观感由浏览器决定，accent-color 只能改填充色，尺寸/圆角/勾形都不可控。
 * 自绘后尺寸固定 14px、圆角取 --r-xs 档的一半、选中填主色。
 *
 * 需要「部分选中」时传 `indeterminate` —— 原生 input 表达不了这个状态。
 * 勾与横杠用内联 SVG（shadcn 那份走图标库，我们没引图标库）。
 */

function Checkbox({ className, ...props }: CheckboxPrimitive.Root.Props) {
  return (
    <CheckboxPrimitive.Root
      data-slot='checkbox'
      className={cn(
        'group/checkbox relative inline-flex size-3.5 shrink-0 cursor-pointer items-center justify-center rounded-[3px] border border-control-border bg-control',
        'outline-none transition-colors duration-150 ease-out',
        'hover:border-control-border-hover',
        // Base UI 在全选时挂 data-checked，部分选中时挂 data-indeterminate（aria-checked="mixed"）
        'data-checked:border-primary data-checked:bg-primary',
        'data-[indeterminate]:border-primary data-[indeterminate]:bg-primary',
        'focus-visible:shadow-focus',
        'data-disabled:cursor-not-allowed data-disabled:opacity-45',
        className
      )}
      {...props}
    >
      <CheckboxPrimitive.Indicator
        data-slot='checkbox-indicator'
        className='grid place-content-center text-primary-foreground data-unchecked:hidden'
      >
        <svg viewBox='0 0 12 12' className='size-2.5' aria-hidden='true'>
          {/* 部分选中画横杠，全选画对勾 —— 与浏览器原生口径一致。
              状态属性在 Root 上、图标在 Indicator 里，所以要经 group 变体取。 */}
          <path
            d='M2 6h8'
            stroke='currentColor'
            strokeWidth='2'
            strokeLinecap='round'
            fill='none'
            className='hidden group-data-[indeterminate]/checkbox:block'
          />
          <path
            d='M2.5 6.2 4.8 8.5 9.5 3.8'
            stroke='currentColor'
            strokeWidth='1.8'
            strokeLinecap='round'
            strokeLinejoin='round'
            fill='none'
            className='group-data-[indeterminate]/checkbox:hidden'
          />
        </svg>
      </CheckboxPrimitive.Indicator>
    </CheckboxPrimitive.Root>
  )
}

export { Checkbox }
