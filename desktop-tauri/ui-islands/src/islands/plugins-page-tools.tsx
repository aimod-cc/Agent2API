import * as React from 'react'
import { Button, Dialog, DialogBody, DialogContent, DialogHeader, DialogTitle, Popover, PopoverContent,
  PopoverTitle, PopoverTrigger } from '@ui'
import type { PluginsPayload } from './plugins-types'
import { GithubFailureText } from '../../../ui-shared/github-failure'
import { t } from '../i18n'

export function PluginPageTools({ disabled, fileDisabled, scriptDisabled, scriptsAvailable, checking,
  onRefresh, onCheck, onSettings, onFile, onScript, onDiagnostics }: {
  disabled: boolean
  fileDisabled: boolean
  scriptDisabled: boolean
  scriptsAvailable: boolean
  checking: boolean
  onRefresh: () => void
  onCheck: () => void
  onSettings: () => void
  onFile: () => void
  onScript: () => void
  onDiagnostics: () => void
}) {
  const [open, setOpen] = React.useState(false)
  const actions = [
    { label: checking ? t('检查中…') : t('检查更新'), disabled: disabled || checking, action: onCheck },
    { label: t('刷新'), disabled, action: onRefresh },
    { label: t('插件设置'), disabled, action: onSettings },
    { label: t('从文件安装…'), disabled: fileDisabled, action: onFile },
    ...(scriptsAvailable ? [{ label: t('从 npm / GitHub 安装'), disabled: scriptDisabled, action: onScript }] : []),
    { label: t('诊断信息'), disabled: false, action: onDiagnostics },
  ]
  return <Popover open={open} onOpenChange={setOpen}>
    <PopoverTrigger render={<Button variant='ghost' size='icon' aria-label={t('更多操作')} title={t('更多操作')} />}>
      <svg viewBox='0 0 24 24' className='size-4' fill='currentColor' aria-hidden='true'>
        <circle cx='5' cy='12' r='1.7' /><circle cx='12' cy='12' r='1.7' /><circle cx='19' cy='12' r='1.7' />
      </svg>
    </PopoverTrigger>
    <PopoverContent align='end' className='w-56 p-1.5'>
      <PopoverTitle className='sr-only'>{t('更多操作')}</PopoverTitle>
      {actions.map(item => <Button key={item.label} variant='ghost' className='justify-start' disabled={item.disabled}
        onClick={() => { setOpen(false); item.action() }}>{item.label}</Button>)}
    </PopoverContent>
  </Popover>
}

export function PluginDiagnostics({ open, payload, onClose }: {
  open: boolean
  payload: PluginsPayload | null
  onClose: () => void
}) {
  return <Dialog open={open} onOpenChange={next => { if (!next) onClose() }}>
    <DialogContent className='text-foreground'>
      <DialogHeader><DialogTitle>{t('诊断信息')}</DialogTitle></DialogHeader>
      <DialogBody className='gap-3 text-[12px] text-subtle'>
        <div>{t('插件目录')} <code className='break-all'>{payload?.pluginsDir || '—'}</code></div>
        <div>{t('当前平台')} <code>{payload?.target || '—'}</code></div>
        <div>{t('协议版本')} {payload?.protocol || '—'}</div>
        {payload?.catalog.reason ? <div className='text-destructive'><GithubFailureText value={payload.catalog} t={t} /></div> : null}
        {payload?.host?.lastError ? <div className='break-words text-destructive'>{payload.host.lastError}</div> : null}
        {payload?.scripts ? <div className='flex flex-col gap-2 border-t border-hairline pt-3'>
          <span>Bun {payload.scripts.runtime.version} · {payload.scripts.runtime.installed ? t('已下载') : t('按需下载')}</span>
          {payload.scripts.runtime.path ? <code className='break-all'>{payload.scripts.runtime.path}</code> : null}
          {payload.scripts.runtime.error ? <p className='m-0 whitespace-pre-wrap break-words text-destructive'>{payload.scripts.runtime.error}</p> : null}
          <p className='m-0'>{t('直接加载兼容的 Magpie / OpenCode v1 提供商包，多个插件共用一个 Bun 进程。')}</p>
          <p className='m-0'>{t('当前兼容无运行时 npm 依赖的提供商插件；不加载 Magpie 界面、Matrix 工具或 Pi 扩展。')}</p>
          <p className='m-0'>{t('登录支持自动完成的 OAuth／设备码和 API Key；要求手动回调的插件暂不支持。')}</p>
        </div> : null}
      </DialogBody>
    </DialogContent>
  </Dialog>
}
