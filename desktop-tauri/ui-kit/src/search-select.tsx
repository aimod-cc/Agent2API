import * as React from 'react'
import { Combobox as ComboboxPrimitive } from '@base-ui/react/combobox'
import { cn } from './lib/cn'
import { t } from './i18n'

/**
 * 单选下拉（带浮层内搜索）。
 *
 * 与 MultiSelect 同族：部件名沿用 Combobox 的叫法，形态对齐 shadcn/ui 的
 * Combobox（单选）。选它而不是 Select 的理由与 MultiSelect 相同 —— 选项一多
 * （提供商全名 / 模型名这类长标签），能在浮层里打字过滤是刚需，Select 的
 * typeahead 只支持从头匹配，覆盖不住「按模型名搜」的用法。
 *
 * 视觉与 MultiSelect 共用同一张脸：触发器同款控件样式，一项都没选时显示
 * placeholder；搜索框在浮层里（input-inside-popup），触发器保持「选择器」
 * 的样子，而不是变成一个能打字的输入框。
 *
 * 用法：
 *   <SearchSelect
 *     value={id}
 *     onValueChange={setId}
 *     options={[{ value: 'wb', label: 'WorkBuddy' }]}
 *     placeholder='请选择'
 *   />
 */

type SearchSelectOption<Value extends string = string> = {
  value: Value
  label: string
  /**
   * 置灰不可选（仍显示在清单里）。与 SelectItem / MultiSelectOption 的
   * `disabled` 同一语义。
   */
  disabled?: boolean
}

type SearchSelectProps<Value extends string = string> = {
  /** 受控选中值；空串 = 未选择（触发器显示 placeholder） */
  value: Value | ''
  onValueChange: (value: Value) => void
  options: readonly SearchSelectOption<Value>[]
  /** 一项都没选时触发器上的文案 */
  placeholder?: string
  /** 浮层里搜不到匹配项时的文案 */
  emptyHint?: string
  /** 浮层里搜索框的占位文案 */
  searchPlaceholder?: string
  disabled?: boolean
  className?: string
  /** 触发器的内联样式（调用方用它注入按内容测得的宽度） */
  style?: React.CSSProperties
  /** 触发器上的原生 id：给可见 `<label htmlFor>` 关联用 */
  id?: string
  /** 触发器的悬停提示 */
  title?: string
  /** 无障碍名（触发器上没有可见 label 时必填） */
  'aria-label'?: string
}

function SearchSelect<Value extends string = string>({
  value,
  onValueChange,
  options,
  placeholder = '',
  emptyHint = t('没有匹配的选项'),
  searchPlaceholder = t('搜索…'),
  disabled = false,
  className,
  style,
  id,
  title,
  ...props
}: SearchSelectProps<Value>) {
  /**
   * 组件对外收的是 value 字符串，Base UI 内部收的是选项对象。翻译与
   * `isItemEqualToValue` 的取舍见 MultiSelect 同名注释：按 value 比，
   * 调用方每次 render 重建 options 才不会把选中态弄丢。
   */
  const selected = React.useMemo(
    () => options.find(option => option.value === value) ?? null,
    [options, value],
  )

  return (
    <ComboboxPrimitive.Root
      items={options}
      value={selected}
      onValueChange={next => onValueChange((next as SearchSelectOption<Value> | null)?.value ?? ('' as Value))}
      isItemEqualToValue={(a, b) => a?.value === b?.value}
      disabled={disabled}
    >
      <ComboboxPrimitive.Trigger
        data-slot='search-select-trigger'
        id={id}
        title={title}
        style={style}
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
        <span
          data-slot='search-select-value'
          className='min-w-0 flex-1 truncate text-left data-[placeholder]:text-muted-foreground'
          data-placeholder={selected ? undefined : ''}
          title={selected?.label ?? placeholder}
        >
          {selected?.label ?? placeholder}
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
            data-slot='search-select-content'
            className={cn(
              // 宽度按选项内容的最大长度自适应（w-max），触发器宽度只作下限，
              // 可用宽度封顶 —— 长模型名不再被截断（理由与 MultiSelect 相同）
              'flex max-h-[280px] w-max min-w-[max(var(--anchor-width),220px)] max-w-[var(--available-width)] flex-col overflow-hidden',
              'rounded-lg border border-border-strong bg-raised shadow-3 outline-none',
              'data-open:animate-in data-open:fade-in-0',
              'data-closed:animate-out data-closed:fade-out-0'
            )}
          >
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
              {(option: SearchSelectOption<Value>) => (
                <ComboboxPrimitive.Item
                  key={option.value}
                  value={option}
                  disabled={option.disabled === true}
                  className={cn(
                    'grid cursor-pointer grid-cols-[14px_minmax(0,1fr)] items-center gap-2 rounded-sm px-[9px] py-[5px]',
                    'text-[12.5px] leading-[1.4] text-foreground select-none outline-none',
                    'transition-colors duration-[120ms] ease-out',
                    'data-[highlighted]:bg-nav-hover',
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

export { SearchSelect, type SearchSelectOption, type SearchSelectProps }
