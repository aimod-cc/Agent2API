import * as React from 'react'
import { createRoot } from 'react-dom/client'
import {
  AlertDialog,
  AlertDialogBanner,
  AlertDialogBody,
  AlertDialogContent,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  Button,
} from '@ui'

/**
 * Agent2API · 通用确认弹窗（替代原生 confirm）。
 *
 * 替换 ui/confirm-dialog.js —— 那份用 innerHTML 拼 .modal-mask / .modal 那套老类名，
 * 结构还写死在 index.html 的 #confirm-modal 里。对外接口与原实现**完全一致**：
 * `window.wbConfirm.ask(options?)`，13 处调用点一行都不用改。
 *
 * 为什么这个弹窗不能少：Tauri 的 WebView 里原生 window.confirm() 不弹窗、直接放行
 * （返回 true），所有依赖它的危险操作（清空 / 删除）等于没有确认。
 *
 * ── 与组件库 AlertDialog 的语义冲突（本文件的关键取舍）─────────────
 * Base UI 的 AlertDialog 是「强制选择」：遮罩按压被内部写死的 disablePointerDismissal
 * 拦掉，标准语义里也不给 Esc 这条出口。但既有契约要求「点遮罩 / Esc 都等于取消」
 * （调用点全靠拿到 false 来中止危险操作），所以这里显式补上这两条 —— 见组件里
 * 那两段 useEffect 的注释。视觉部分（z-38、min(440px,…)、danger-zone 观感）
 * 由 ui-kit 的 AlertDialogContent / AlertDialogBanner 直接提供，不必自己写。
 */

type AskOptions = {
  /** 标题（纯文本） */
  title?: string
  /** 正文原始 HTML，调用方负责转义 —— 内部原样注入 */
  html?: string
  /** 纯文本正文（与 html 二选一，内部转义，\n 转成 <br>） */
  text?: string
  /** 确认键文案（如「清空」「删除」） */
  okText?: string
  /** 确认键样式：primary（默认）或 danger（不可恢复的危险操作） */
  okClass?: string
  /** 正文容器：danger-zone（默认，红底警示框）；普通提示传空串 */
  bodyClass?: string
  /** 取消键文案 */
  cancelText?: string
}

/** 与旧实现逐字一致的转义（注意：不含单引号，改了就可能和调用方拼的 HTML 不一致） */
function escapeHtml(value: unknown): string {
  return String(value ?? '')
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
}

type ConfirmDialogProps = {
  options: AskOptions
  /** 收口回调：true = 确认，false = 取消 / ✕ / 遮罩 / Esc */
  onSettle: (ok: boolean) => void
}

function ConfirmDialog({ options, onSettle }: ConfirmDialogProps) {
  const {
    title = '确认操作',
    html = '',
    text = '',
    okText = '确定',
    okClass = 'primary',
    bodyClass = 'danger-zone',
    cancelText = '取消',
  } = options

  // 焦点要落在「取消」上：不可恢复的操作，敲回车不该等于同意。
  // 用 AlertDialogContent 的 initialFocus 而不是按钮上的 autoFocus —— 焦点陷阱
  // 打开时会自己决定首个焦点，autoFocus 会被它覆盖掉。
  const cancelRef = React.useRef<HTMLButtonElement | null>(null)

  /**
   * Esc = 取消。AlertDialog 的标准语义是不给这条出口，但既有契约要（旧实现里
   * 13 处调用点都靠它拿到 false），所以自己兜一条 document 监听。
   * 不 stopPropagation：旧实现同样没掐冒泡，app.js / settings-panel 那些只管
   * 自己弹窗的全局 Esc 处理照旧运行，行为不变。
   */
  React.useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === 'Escape') onSettle(false)
    }
    document.addEventListener('keydown', onKeyDown)
    return () => document.removeEventListener('keydown', onKeyDown)
  }, [onSettle])

  /**
   * 点遮罩 = 取消。AlertDialog 的遮罩按压在 Base UI 内部被 disablePointerDismissal
   * 写死拦掉，而 ui-kit 的 AlertDialogContent 把遮罩渲染封在内部、不透传 props，
   * 没法在遮罩元素上直接挂 onClick —— 只能退到 document 上代理：事件目标正好是
   * 遮罩本身（data-slot 是组件库固定的标记）才算「点遮罩」。
   * 在遮罩上按下、拖到弹窗上松开时，click 的目标是两者的共同祖先（portal 容器），
   * 不会误判成取消；点弹窗本体更不会。
   */
  React.useEffect(() => {
    function onClick(event: MouseEvent) {
      const target = event.target as HTMLElement | null
      if (target?.dataset.slot === 'alert-dialog-overlay') onSettle(false)
    }
    document.addEventListener('click', onClick)
    return () => document.removeEventListener('click', onClick)
  }, [onSettle])

  return (
    // 受控 open（恒为 true）：本岛是「打开时建、关闭即卸」，关窗一律由 onSettle
    // 收口。onOpenChange 只用来接 ✕ 这类由 Base UI 自己发起的关闭请求。
    <AlertDialog open onOpenChange={next => { if (!next) onSettle(false) }}>
      {/* ✕ 显式打开：既有契约里它是「取消」的一种（标准 AlertDialog 默认不留这条
          模糊出口，所以默认是关的）。它走 Base UI 的 Close → onOpenChange(false)。 */}
      <AlertDialogContent initialFocus={cancelRef} showCloseButton>
        {/* pr-10 给右上角的 ✕ 让位，长标题不会钻到它底下 */}
        <AlertDialogHeader className='pr-10'>
          <AlertDialogTitle>{title}</AlertDialogTitle>
        </AlertDialogHeader>
        <AlertDialogBody>
          {/* html 与 text 二选一：html 非空就用它（调用方负责转义，内部原样注入，
              既有调用点会传 <strong>/<code>/<b> 这类标记），否则用转义后的 text
              并把换行转成 <br>。 */}
          <AlertDialogBanner
            tone={bodyClass === 'danger-zone' ? 'danger' : 'neutral'}
            dangerouslySetInnerHTML={{ __html: html || escapeHtml(text).replace(/\n/g, '<br>') }}
          />
        </AlertDialogBody>
        <AlertDialogFooter>
          <div className='mr-auto' />
          <Button ref={cancelRef} variant='outline' onClick={() => onSettle(false)}>
            {cancelText}
          </Button>
          {/* danger 走 destructive（语义柔底 + 语义描边），其余一律主色 default */}
          <Button
            variant={okClass === 'danger' ? 'destructive' : 'default'}
            onClick={() => onSettle(true)}
          >
            {okText}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}

/* ─── 命令式外壳：与旧实现的 window.wbConfirm 接口一致 ─── */

let root: ReturnType<typeof createRoot> | null = null
let host: HTMLElement | null = null
/** 当前等待者；非空即表示弹窗开着（与旧实现的 resolver 同义） */
let pending: ((ok: boolean) => void) | null = null

function unmountDialog() {
  if (root) {
    root.unmount()
    root = null
  }
  if (host) {
    host.remove()
    host = null
  }
}

/** 收口：关窗并把结果交给等待者（重复调用无副作用） */
function settle(ok: boolean) {
  const resolve = pending
  pending = null
  unmountDialog()
  resolve?.(ok)
}

function ask(options: AskOptions = {}): Promise<boolean> {
  return new Promise<boolean>(resolve => {
    // 同时只开一个：重入时把前一个按「取消」收尾，否则前一个调用方的 Promise
    // 永远挂着（旧实现同一取向）。先 settle 再登记新的，避免自己收掉自己。
    if (pending) settle(false)
    pending = resolve
    host = document.createElement('div')
    document.body.append(host)
    root = createRoot(host)
    root.render(<ConfirmDialog options={options} onSettle={settle} />)
  })
}

declare global {
  interface Window {
    wbConfirm?: {
      ask(options?: AskOptions): Promise<boolean>
    }
  }
}

window.wbConfirm = { ask }
