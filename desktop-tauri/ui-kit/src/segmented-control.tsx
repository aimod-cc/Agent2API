import * as React from 'react'
import { Radio } from '@base-ui/react/radio'
import { RadioGroup } from '@base-ui/react/radio-group'
import { cn } from './lib/cn'
import { useSelectionIndicator } from './lib/selection-indicator'

/**
 * 分段控件：一行互斥的档位选择。全站的分段筛选都走这一个组件。
 *
 * 行为层（焦点管理、方向键切换、roving tabindex、单选语义）全部交给 Base UI 的
 * RadioGroup；滑块视觉由组件库 selection.css 统一维护。
 *
 * 为什么用 RadioGroup 而不是 ToggleGroup：这些都是「多选一」的筛选，
 * 单选语义（`role="radiogroup"`，方向键移动即选中）比切换按钮组
 * （`aria-pressed`，方向键只移焦点、还要 Space/Enter 确认）更贴合用户对
 * 筛选器的预期，读屏播报的也是「单选按钮 已选中」而不是「按下」。
 *
 * Radio 用 Base UI 默认的 `<span role="radio">`，**不要改成 `render={<button />}`
 * 加 `nativeButton`**：那条路会坏掉键盘方向键。RadioRoot 在组内是经 `CompositeItem`
 * 渲染的，props 合并顺序为 `[compositeProps, ...props, elementProps]`，而 `useButton`
 * 生成的 props 排在 compositeProps 之后，会把 CompositeItem 注册的方向键处理覆盖掉
 * —— 实测 button 形态下按方向键焦点纹丝不动，span 形态下正常移动。
 *
 * 选中态用一枚滑动指示器承载，不再让选中项自己变色 —— 这样切换档位时能看到
 * 色块平移过去，而不是硬切。指示器跟踪实际选中项的二维位置与尺寸。
 * 选中项文字不加粗，避免字重变化让整个控件重新排版。
 */

type SegmentedControlOption<Value extends string> = {
  value: Value
  label: React.ReactNode
  /**
   * 计数徽标：不传就不渲染徽标。
   * 传 0 时整项按「空段」弱化，与账号页筛选的既有约定一致。
   */
  count?: number
  /**
   * 悬停提示（原生 title）。分段里没有第二行位置，说明性文字只能挂这儿 ——
   * 比如账号设置里「轮询（换 IP）」为什么存在（免费档按 IP 限流，单 IP 用完一律 429）。
   */
  title?: string
  disabled?: boolean
}

type SegmentedControlProps<Value extends string> = {
  options: readonly SegmentedControlOption<Value>[]
  /** 受控选中值：本控件不持有状态，值始终由调用方给 */
  value: Value
  onValueChange: (value: Value) => void
  /** 无障碍名：这一组档位在筛什么 */
  'aria-label': string
  disabled?: boolean
  /** 附加到容器上的类名（弹窗里的紧凑档位就是靠它带尺寸覆盖） */
  className?: string
}

function SegmentedControl<Value extends string>({
  options,
  value,
  onValueChange,
  disabled = false,
  className,
  ...props
}: SegmentedControlProps<Value>) {
  const { containerRef, indicator } = useSelectionIndicator(
    '[data-slot="segmented-item"]', '[data-slot="segmented-item"][data-checked]',
  )

  return (
    <RadioGroup<Value>
      ref={containerRef}
      data-slot='segmented-control'
      className={cn(
        'relative inline-flex gap-0.5 rounded-md border border-hairline bg-control p-0.5 shadow-none',
        className
      )}
      value={value}
      disabled={disabled}
      onValueChange={next => onValueChange(next)}
      {...props}
    >
      {/* 纯装饰：位置由 JS 喂，交互一律穿透给下面的选项 */}
      <span aria-hidden='true' data-slot='segmented-thumb' {...indicator} />
      {options.map(option => (
        <Radio.Root
          key={option.value}
          value={option.value}
          data-slot='segmented-item'
          title={option.title}
          disabled={option.disabled}
          data-zero={option.count === 0 ? 'true' : undefined}
          className={cn(
            'group/seg relative z-[1] inline-flex h-6 items-center gap-[5px] rounded-sm border-0 bg-transparent px-2.5',
            'text-xs font-normal whitespace-nowrap text-subtle select-none',
            'transition-[background-color,color] duration-150 ease-out',
            'hover:bg-nav-hover hover:text-foreground focus-visible:outline-none focus-visible:shadow-focus',
            // 选中项让出底色（底色归滑块画），只留文字色
            'data-checked:bg-transparent data-checked:text-foreground data-checked:hover:bg-transparent',
            'data-disabled:cursor-not-allowed data-disabled:opacity-40',
            // 计数为 0 的分段弱化；当前选中的那一档即使为 0 也保持正常
            'data-[zero=true]:data-unchecked:opacity-45'
          )}
        >
          {option.label}
          {option.count !== undefined && (
            <span
              className={cn(
                'min-w-4 rounded-pill bg-surface-3 px-[5px] text-center text-[10.5px] font-semibold tabular-nums text-muted-foreground',
                'group-data-checked/seg:bg-surface-2 group-data-checked/seg:text-foreground'
              )}
            >
              {option.count}
            </span>
          )}
        </Radio.Root>
      ))}
    </RadioGroup>
  )
}

export { SegmentedControl, type SegmentedControlOption, type SegmentedControlProps }
