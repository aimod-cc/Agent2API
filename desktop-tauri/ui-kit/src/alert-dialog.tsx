import * as React from 'react'
import { AlertDialog as AlertDialogPrimitive } from '@base-ui/react/alert-dialog'
import { cn } from './lib/cn'
import { Button } from './button'
import { t } from './i18n'

/**
 * 确认对话框（标准形态，与 shadcn/ui 的 AlertDialog 一致）。
 *
 * 部件：AlertDialog / AlertDialogTrigger / AlertDialogPortal / AlertDialogClose /
 * AlertDialogOverlay / AlertDialogContent / AlertDialogHeader / AlertDialogTitle /
 * AlertDialogDescription / AlertDialogFooter / AlertDialogAction /
 * AlertDialogCancel。
 *
 * ── 与 Dialog 的区别（标准语义，别混用）──────────────────────
 * AlertDialog 是**强制选择**：点遮罩、按 Esc 都不关闭，必须点其中一个动作键。
 * Dialog 是普通弹窗：点遮罩 / Esc 就关。
 * 因此「不可恢复操作」的二次确认用 AlertDialog 最贴切 —— 用户不能靠点空白处
 * 糊弄过去，必须明确表态。
 *
 * 但本项目既有的 wbConfirm.ask 契约允许点遮罩 / Esc 取消（既有 13 处调用点都
 * 依赖「返回 false」这一行为），所以 ui/islands 里的确认弹窗岛走的是标准
 * AlertDialog 结构 + 显式放开两种关闭途径（见 wbConfirm 岛的实现注释）。
 * 新写确认框时直接用本组件、不加那两个放开项，拿到的就是标准行为。
 *
 * 层级：z-38（比普通弹窗 z-30 高、比 toast z-40 低）。确认框常从别的弹窗里发起
 * （「终止请求」「删除账号」…），同档时会被后声明的弹窗盖住，所以必须抬起来 ——
 * 这条与 ui/css/components.css 里 `#confirm-modal { z-index: 38 }` 的历史取舍一致。
 */

const AlertDialog = AlertDialogPrimitive.Root
const AlertDialogTrigger = AlertDialogPrimitive.Trigger
const AlertDialogPortal = AlertDialogPrimitive.Portal

const AlertDialogTitle = (props: AlertDialogPrimitive.Title.Props) => (
  <AlertDialogPrimitive.Title data-slot='alert-dialog-title' {...props} />
)

const AlertDialogDescription = (props: AlertDialogPrimitive.Description.Props) => (
  <AlertDialogPrimitive.Description data-slot='alert-dialog-description' {...props} />
)

function AlertDialogOverlay({ className, ...props }: AlertDialogPrimitive.Backdrop.Props) {
  return (
    <AlertDialogPrimitive.Backdrop
      data-slot='alert-dialog-overlay'
      className={cn(
        'fixed inset-0 isolate z-[38] bg-mask backdrop-blur-[3px]',
        'transition-opacity duration-150',
        'data-open:animate-in data-open:fade-in-0',
        'data-closed:animate-out data-closed:fade-out-0 data-closed:fill-mode-forwards',
        className
      )}
      {...props}
    />
  )
}

type AlertDialogContentProps = AlertDialogPrimitive.Popup.Props & {
  /** 是否自带右上角关闭按钮，默认 false —— 强制选择类弹窗不留「✕」这条模糊出口 */
  showCloseButton?: boolean
}

function AlertDialogContent({
  className,
  children,
  showCloseButton = false,
  ...props
}: AlertDialogContentProps) {
  return (
    <AlertDialogPortal>
      <AlertDialogOverlay />
      <AlertDialogPrimitive.Popup
        data-slot='alert-dialog-content'
        className={cn(
          'fixed top-1/2 left-1/2 z-[38] flex max-h-[calc(100vh-48px)] w-[min(440px,calc(100vw-48px))] -translate-x-1/2 -translate-y-1/2 flex-col',
          'overflow-hidden rounded-lg border border-border bg-surface shadow-3 outline-none',
          'transition-[opacity,transform] duration-200 [transition-timing-function:cubic-bezier(.2,.9,.3,1)]',
          'data-open:animate-in data-open:fade-in-0 data-open:zoom-in-[.985] data-open:slide-in-from-bottom-2.5',
          'data-closed:animate-out data-closed:fade-out-0 data-closed:fill-mode-forwards',
          className
        )}
        {...props}
      >
        {children}
        {showCloseButton && (
          <AlertDialogPrimitive.Close
            data-slot='alert-dialog-close'
            render={<Button variant='ghost' size='icon-sm' className='absolute top-3 right-3' />}
          >
            <svg viewBox='0 0 12 12' className='size-3' aria-hidden='true'>
              <path
                d='M2.5 2.5l7 7M9.5 2.5l-7 7'
                stroke='currentColor'
                strokeWidth='1.5'
                strokeLinecap='round'
                fill='none'
              />
            </svg>
            <span className='sr-only'>{t('关闭')}</span>
          </AlertDialogPrimitive.Close>
        )}
      </AlertDialogPrimitive.Popup>
    </AlertDialogPortal>
  )
}

/** 标题栏：与 DialogHeader 同款（左侧标题、底部发丝线、右侧留白给关闭钮） */
function AlertDialogHeader({ className, ...props }: React.ComponentProps<'div'>) {
  return (
    <div
      data-slot='alert-dialog-header'
      className={cn(
        'flex items-center gap-2.5 border-b border-hairline px-5 py-[15px]',
        '[&_h2]:flex-1 [&_h2]:text-[14.5px] [&_h2]:font-semibold',
        className
      )}
      {...props}
    />
  )
}

/** 正文（项目扩展，对应 `.modal-body`）：纵向排布、超高滚动 */
function AlertDialogBody({ className, ...props }: React.ComponentProps<'div'>) {
  return (
    <div
      data-slot='alert-dialog-body'
      className={cn('flex flex-col gap-4 overflow-y-auto px-5 py-[18px]', className)}
      {...props}
    />
  )
}

/** 底栏：按钮靠右，左侧留白用 mr-auto 顶开 */
function AlertDialogFooter({ className, ...props }: React.ComponentProps<'div'>) {
  return (
    <div
      data-slot='alert-dialog-footer'
      className={cn(
        'flex items-center gap-2 border-t border-hairline bg-surface-2 px-5 py-[13px]',
        className
      )}
      {...props}
    />
  )
}

/** 危险操作的警示正文框（对应 ui/css 的 `.danger-zone`：语义柔底 + 语义描边 + 小字）
 *  `tone` 取 neutral 时是普通提示框（背景凹槽底、普通描边）。 */
function AlertDialogBanner({
  className,
  tone = 'danger',
  ...props
}: React.ComponentProps<'div'> & { tone?: 'danger' | 'neutral' }) {
  return (
    <div
      data-slot='alert-dialog-banner'
      data-tone={tone}
      className={cn(
        'rounded-md border px-3.5 py-3 text-xs leading-[1.65] text-subtle',
        '[&_strong]:font-semibold',
        tone === 'danger'
          ? 'border-destructive-bd bg-destructive-soft [&_strong]:text-destructive'
          : 'border-border bg-surface-inset [&_strong]:text-foreground',
        className
      )}
      {...props}
    />
  )
}

export {
  AlertDialog,
  AlertDialogTrigger,
  AlertDialogPortal,
  AlertDialogOverlay,
  AlertDialogContent,
  AlertDialogHeader,
  AlertDialogBody,
  AlertDialogFooter,
  AlertDialogBanner,
  AlertDialogTitle,
  AlertDialogDescription,
}
