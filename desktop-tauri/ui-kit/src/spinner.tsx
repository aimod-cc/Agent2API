import * as React from 'react'
import { cn } from './lib/cn'
import { t } from './i18n'

/**
 * 转圈指示器（标准形态，与 shadcn/ui 的 Spinner 一致：一个旋转的 loader 图标）。
 *
 * shadcn 那份用的是图标库（lucide 的 Loader2Icon 等），我们没引入图标库，
 * 这里用等价的内联 SVG：24 视窗、stroke 2、缺口在右上 —— 结构与标准一致
 * （`role="status"` + `aria-label` + `data-slot="spinner"`），只是把图标换成了本地路径。
 *
 * 颜色走 currentColor：放进按钮、徽章里不用额外指定，跟随所在容器的文字色。
 * 默认 size-4（与标准一致），要项目里那枚 12px 的小圈传 `className="size-3"`。
 * 转速按项目口径收快一档（0.7s，Tailwind 默认 1s）。
 */

function Spinner({ className, ...props }: React.ComponentProps<'svg'>) {
  return (
    <svg
      data-slot='spinner'
      role='status'
      aria-label={t('加载中')}
      viewBox='0 0 24 24'
      fill='none'
      stroke='currentColor'
      strokeWidth='2'
      strokeLinecap='round'
      className={cn('size-4 animate-spin [animation-duration:0.7s]', className)}
      {...props}
    >
      {/* loader 圈：留一个缺口，转起来才有方向感 */}
      <path d='M21 12a9 9 0 1 1-6.219-8.56' />
    </svg>
  )
}

export { Spinner }
