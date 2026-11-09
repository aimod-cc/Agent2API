import { useRef } from 'react'
import {
  Button, Dialog, DialogBody, DialogContent, DialogDescription, DialogFooter,
  DialogHeader, DialogTitle,
} from '@ui'
import type { PluginTranslate } from './plugins-store'

/** 安装前的兼容性须知：从「安装脚本插件」弹窗里叠起来的一层。
 *
 * 用普通 Dialog 而不是 AlertDialog：这一层只是把「不保证 100% 可用」讲清楚，
 * 不是危险操作的强制表态（信任勾选已经在上层做过了），所以照其他弹窗的规矩来 ——
 * 点空白处、按 Esc、右上角 ✕ 都能退出。
 *
 * overlayForceRender：Base UI 的嵌套约定是「子层不画遮罩」，只画的话下层（含上层
 * 弹窗）不会被压暗、也没有高斯模糊，点空白处更不会关闭（点空白处关闭认的就是遮罩
 * 本身）。所以这一层必须显式要回自己的遮罩，颜色与模糊与普通弹窗同一套。
 */
export function ScriptPluginInstallNotice({ open, source, version, disabled, busy, error, onCancel, onConfirm, t }: {
  open: boolean
  source: string
  version: string
  disabled: boolean
  busy: boolean
  error?: string
  onCancel: () => void
  onConfirm: () => void
  t: PluginTranslate
}) {
  const cancelRef = useRef<HTMLButtonElement>(null)
  return <Dialog open={open} onOpenChange={(next, details) => {
    // 正在安装时不许靠点空白处 / Esc 撤走，避免用户以为没装上。
    if (!next) { if (busy) details.cancel(); else onCancel() }
  }}>
    <DialogContent overlayForceRender className='w-[min(480px,calc(100vw-48px))] text-foreground' initialFocus={cancelRef}>
      <DialogHeader><DialogTitle>{t('Magpie 插件安装须知')}</DialogTitle></DialogHeader>
      <DialogBody className='gap-3 text-xs leading-relaxed text-subtle'>
        <div className='rounded-md border border-hairline bg-surface-inset px-3 py-2'>
          <p className='m-0 break-all font-medium text-foreground'>{source}</p>
          {version ? <p className='m-0 mt-1 break-all text-muted-foreground'>{version}</p> : null}
        </div>
        <DialogDescription className='m-0 text-sm font-medium leading-relaxed text-foreground'>
          {t('Magpie 官方和社区插件均通过兼容方式接入 Agent2API，不保证 100% 可用。')}
        </DialogDescription>
        <p className='m-0'>{t('登录、模型调用、模型列表、余额查询和签到等功能，可能因插件实现或上游变化而不可用。')}</p>
        <p className='m-0'>{t('签到及附带任务默认允许，实际执行遵循签到中心的开关和计划。')}</p>
        <p className='m-0 text-muted-foreground'>{t('部分插件的附带任务可能消耗额度、发送对话或创建及解散群聊。')}</p>
        {error ? <p role='alert' className='m-0 break-words text-destructive'>{error}</p> : null}
      </DialogBody>
      <DialogFooter>
        <Button ref={cancelRef} variant='outline' disabled={busy} onClick={onCancel}>{t('取消')}</Button>
        <Button disabled={disabled} onClick={onConfirm}>{busy ? t('正在安装…') : t('了解并安装')}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
}
