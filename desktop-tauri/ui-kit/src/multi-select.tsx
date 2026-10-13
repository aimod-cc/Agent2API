import * as React from 'react'
import { Combobox as ComboboxPrimitive } from '@base-ui/react/combobox'
import { cn } from './lib/cn'
import { t } from './i18n'

/**
 * 多选下拉。
 *
 * 形态与 shadcn/ui 的 Combobox 一致（shadcn 的多选就是 Combobox 加 `multiple`，
 * 没有另一个叫 MultiSelect 的组件）—— 部件名沿用 Combobox 的叫法，
 * 这里只是把「触发器当表单控件、搜索框放进浮层」那套样板封成一个组件。
 *
 * 为什么是 Combobox 而不是 Select 加 multiple：Base UI 的 Select 是单选原语，
 * 多选只能自己拿 Checkbox 拼，键盘与 aria 都得手写。Combobox 原生支持
 * `multiple`，并且**内建按标签筛选** —— 提供商一多，能在浮层里打字过滤是刚需。
 *
 * 视觉对齐既有的多选下拉（ui/css/select.css 的 `.select-shell-multi`）：
 * 触发器与 SelectTrigger 同一张脸，一项都没选时显示 `placeholder`
 * （空选的语义通常是「不限制」，必须说清 —— 留一个空框会让人以为「不选＝全都不能用」）；
 * 选中多项时按 `、` 连接（与 select.js 的 selectedText 同一口径）。
 *
 * 用法：
 *   <MultiSelect
 *     value={ids}
 *     onValueChange={setIds}
 *     options={[{ value: 'wb', label: 'WorkBuddy' }]}
 *     placeholder='留空 = 不限制'
 *   />
 */

type MultiSelectOption<Value extends string = string> = {
  value: Value
  label: string
}

type MultiSelectProps<Value extends string = string> = {
  /** 受控选中值；本组件不持有状态 */
  value: readonly Value[]
  onValueChange: (value: Value[]) => void
  options: readonly MultiSelectOption<Value>[]
  /** 一项都没选时触发器上的文案 */
  placeholder?: string
  /** 浮层里搜不到匹配项时的文案 */
  emptyHint?: string
  /** 浮层里搜索框的占位文案 */
  searchPlaceholder?: string
  disabled?: boolean
  className?: string
  /** 触发器上的原生 id：给可见 `<label htmlFor>` 关联用（没有可见 label 时用 aria-label 也行） */
  id?: string
  /** 触发器的悬停提示 */
  title?: string
  /** 无障碍名（触发器上没有可见 label 时必填） */
  'aria-label'?: string
}

/** 选中项的连接符（与 select.js 的 selectedText 逐字一致） */
const JOINER = '、'

function MultiSelect<Value extends string = string>({
  value,
  onValueChange,
  options,
  placeholder = '',
  emptyHint = t('没有匹配的选项'),
  searchPlaceholder = t('搜索…'),
  disabled = false,
  className,
  id,
  title,
  ...props
}: MultiSelectProps<Value>) {
  /**
   * 组件对外收的是 value 字符串数组，Base UI 内部收的是选项对象数组。
   * 两个方向都在这里翻译，调用方不必关心选项对象的引用是否稳定 ——
   * 所以 `isItemEqualToValue` 必须按 value 比，不能用默认的 Object.is：
   * 调用方每次 render 重建 options 数组时，Object.is 会认不出「还是那一项」，
   * 选中态会闪没。
   */
  const selected = React.useMemo(
    () => options.filter(option => value.includes(option.value)),
    [options, value]
  )

  const summary = selected.length
    ? selected.map(option => option.label).join(JOINER)
    : placeholder

  return (
    <ComboboxPrimitive.Root
      items={options}
      multiple
      value={selected}
      onValueChange={next => onValueChange(next.map(option => option.value))}
      isItemEqualToValue={(a, b) => a?.value === b?.value}
      disabled={disabled}
    >
      <ComboboxPrimitive.Trigger
        data-slot='multi-select-trigger'
        id={id}
        title={title}
        aria-label={props['aria-label']}
        className={cn(
          'inline-flex h-[30px] min-w-0 cursor-pointer items-center justify-between gap-1.5 rounded-md border border-control-border bg-control px-2.5',
          'text-[12.5px] font-normal text-foreground',
          'transition-colors duration-150 ease-out',
          'hover:border-control-border-hover hover:bg-control-hover',
          'focus-visible:border-primary focus-visible:shadow-focus focus-visible:outline-none',
          'data-disabled:cursor-not-allowed data-disabled:bg-control data-disabled:text-muted-foreground data-disabled:opacity-60',
          className
        )}
      >
        {/* 一项都没选时走 data-placeholder 的弱化色，与 Select 的占位观感一致 */}
        <span
          data-slot='multi-select-value'
          className='min-w-0 flex-1 truncate text-left data-[placeholder]:text-muted-foreground'
          data-placeholder={selected.length ? undefined : ''}
          title={summary}
        >
          {summary}
        </span>
        <ComboboxPrimitive.Icon className='flex flex-none text-muted-foreground transition-transform duration-[180ms] data-popup-open:rotate-180'>
          <svg viewBox='0 0 10 6' className='size-2.5' aria-hidden='true'>
            <path
              d='M1 1l4 4 4-4'
              stroke='currentColor'
              strokeWidth='1.4'
              fill='none'
              strokeLinecap='round'
              strokeLinejoin='round'
            />
          </svg>
        </ComboboxPrimitive.Icon>
      </ComboboxPrimitive.Trigger>

      <ComboboxPrimitive.Portal>
        <ComboboxPrimitive.Positioner sideOffset={4} className='z-[35]'>
          <ComboboxPrimitive.Popup
            data-slot='multi-select-content'
            className={cn(
              // 宽度跟随触发器（--anchor-width），但给一个下限：触发器常常很窄，
              // 而选项文字（提供商全名 / 模型名）比触发器长得多
              'flex max-h-[280px] w-[max(var(--anchor-width),220px)] flex-col overflow-hidden',
              'rounded-lg border border-border-strong bg-raised shadow-3 outline-none',
              'data-open:animate-in data-open:fade-in-0',
              'data-closed:animate-out data-closed:fade-out-0'
            )}
          >
            {/* 搜索框在浮层里（input-inside-popup）：触发器保持「选择器」的样子，
                而不是变成一个能打字的输入框 —— 那会让人以为可以随便填。 */}
            <div className='flex flex-none items-center gap-1.5 border-b border-hairline px-2.5 py-1.5'>
              <svg viewBox='0 0 12 12' className='size-3 flex-none text-muted-foreground' aria-hidden='true'>
                <circle cx='5' cy='5' r='3.4' stroke='currentColor' strokeWidth='1.3' fill='none' />
                <path d='M7.6 7.6 10.5 10.5' stroke='currentColor' strokeWidth='1.3' strokeLinecap='round' />
              </svg>
              <ComboboxPrimitive.Input
                placeholder={searchPlaceholder}
                className='min-w-0 flex-1 border-0 bg-transparent p-0 text-[12.5px] text-foreground outline-none placeholder:text-muted-foreground'
              />
            </div>
            <ComboboxPrimitive.Empty>
              <div className='px-2.5 py-3 text-[11.5px] leading-[1.5] text-muted-foreground'>
                {emptyHint}
              </div>
            </ComboboxPrimitive.Empty>
            <ComboboxPrimitive.List className='min-h-0 flex-1 overflow-y-auto overscroll-contain p-[5px]'>
              {(option: MultiSelectOption<Value>) => (
                <ComboboxPrimitive.Item
                  key={option.value}
                  value={option}
                  className={cn(
                    'grid cursor-pointer grid-cols-[14px_minmax(0,1fr)] items-center gap-2 rounded-sm px-[9px] py-[5px]',
                    'text-[12.5px] leading-[1.4] text-foreground select-none outline-none',
                    'transition-colors duration-[120ms] ease-out',
                    'data-[highlighted]:bg-nav-hover',
                    // 选中项走品牌浅底 + 字重（与 SelectItem 同一观感）。
                    // 用 data-[selected] 而不是 shadcn 的 data-selected 变体：
                    // Base UI 输出的是 data-selected=""，空串匹配不上 true。
                    'data-[selected]:bg-primary-soft data-[selected]:font-semibold data-[selected]:text-primary-fg',
                    'data-[highlighted]:data-[selected]:bg-primary-tint',
                    'data-disabled:pointer-events-none data-disabled:text-muted-foreground'
                  )}
                >
                  <ComboboxPrimitive.ItemIndicator className='col-start-1 flex text-primary'>
                    <svg viewBox='0 0 12 12' className='size-3' aria-hidden='true'>
                      <path
                        d='M2.5 6.2 4.8 8.5 9.5 3.8'
                        stroke='currentColor'
                        strokeWidth='1.8'
                        fill='none'
                        strokeLinecap='round'
                        strokeLinejoin='round'
                      />
                    </svg>
                  </ComboboxPrimitive.ItemIndicator>
                  <span className='col-start-2 min-w-0 truncate'>{option.label}</span>
                </ComboboxPrimitive.Item>
              )}
            </ComboboxPrimitive.List>
          </ComboboxPrimitive.Popup>
        </ComboboxPrimitive.Positioner>
      </ComboboxPrimitive.Portal>
    </ComboboxPrimitive.Root>
  )
}

export { MultiSelect, type MultiSelectOption, type MultiSelectProps }
