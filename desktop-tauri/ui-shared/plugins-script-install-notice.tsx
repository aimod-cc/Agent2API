import { useRef } from 'react'
import {
  Alert,
  AlertDialog,
  AlertDialogBody,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  Button,
} from '@ui'
import type { PluginTranslate } from './plugins-store'

/** 安装或更新脚本插件前的安全与兼容性确认弹窗：从「安装／更新脚本插件」弹窗叠起来的一层。
 *
 * 用 AlertDialog + overlayForceRender：信任确认统一收敛到这一层，不再要求在表单里先勾选
 * 复选框；点空白处 / Esc 不会误关，用户必须明确选择取消或继续。
 */
export function ScriptPluginInstallNotice({ open, updating = false, source, version, disabled, busy, error, onCancel, onConfirm, t }: {
  open: boolean
  updating?: boolean
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
  return <AlertDialog open={open} onOpenChange={(next, details) => {
    // 正在安装或更新时不许关闭，避免用户以为没执行。
    if (!next) { if (busy) details.cancel(); else onCancel() }
  }}>
    <AlertDialogContent overlayForceRender className='w-[min(520px,calc(100vw-48px))] text-foreground' initialFocus={cancelRef}>
      <AlertDialogHeader>
        <AlertDialogTitle>{updating ? t('确认更新脚本插件') : t('确认安装脚本插件')}</AlertDialogTitle>
      </AlertDialogHeader>
      <AlertDialogBody className='gap-3 text-xs leading-relaxed text-subtle'>
        <div className='rounded-md border border-hairline bg-surface-inset px-3 py-2'>
          <p className='m-0 break-all font-medium text-foreground'>{source}</p>
          {version ? <p className='m-0 mt-1 break-all text-muted-foreground'>{version}</p> : null}
        </div>
        <Alert variant='warning'>{t('脚本不是安全沙箱，可以访问网关所在电脑的文件和网络。不要安装来历不明的插件。')}</Alert>
        <div className='space-y-1.5 rounded-md border border-hairline bg-surface-inset px-3.5 py-3'>
          <p className='m-0 font-medium text-foreground'>{t('Magpie 官方和社区插件均通过兼容方式接入 Agent2API，不保证 100% 可用。')}</p>
          <p className='m-0'>{t('登录、模型调用、模型列表、余额查询和签到等功能，可能因插件实现或上游变化而不可用。')}</p>
          <p className='m-0'>{t('签到及附带任务默认允许，实际执行遵循签到中心的开关和计划。')}</p>
          <p className='m-0 text-muted-foreground'>{t('部分插件的附带任务可能消耗额度、发送对话或创建及解散群聊。')}</p>
        </div>
        <AlertDialogDescription className='m-0 text-[13px] leading-relaxed text-foreground'>
          {t('我信任此插件来源，并允许它在网关电脑上运行。')}
        </AlertDialogDescription>
        {error ? <p role='alert' className='m-0 break-words text-destructive'>{error}</p> : null}
      </AlertDialogBody>
      <AlertDialogFooter>
        <Button ref={cancelRef} variant='outline' disabled={busy} onClick={onCancel}>{t('取消')}</Button>
        <Button disabled={disabled} onClick={onConfirm}>
          {busy ? (updating ? t('正在更新…') : t('正在安装…')) : (updating ? t('了解并更新') : t('了解并安装'))}
        </Button>
      </AlertDialogFooter>
    </AlertDialogContent>
  </AlertDialog>
}
