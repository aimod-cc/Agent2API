import { useEffect, useRef, useState } from 'react'
import { Alert, Button, Dialog, DialogBody, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, Field, FieldDescription, FieldLabel, Input, Switch } from '@ui'
import { CODING_BASE, codingConnection, type CodingConfig, type CodingStatus, type CodingTranslate, type CodingTransport } from './assistant-coding-types'

export function CodingSettings({ open, onOpenChange, config, transport, t, onSaved }: {
  open: boolean; onOpenChange: (open: boolean) => void; config: CodingConfig | null
  transport: CodingTransport; t: CodingTranslate; onSaved: (config: CodingConfig) => void
}) {
  const [executable, setExecutable] = useState('')
  const [home, setHome] = useState('')
  const [enabled, setEnabled] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [status, setStatus] = useState<CodingStatus | null>(null)
  const saving = useRef(false)
  useEffect(() => {
    if (!open) return
    setExecutable(config?.executable || ''); setHome(config?.codexHome || '')
    setEnabled(config?.enabled ?? false)
    setError(''); setStatus(null)
  }, [open])
  const save = async (check: boolean) => {
    if (saving.current) return
    saving.current = true; setBusy(true); setError(''); setStatus(null)
    try {
      const result = await transport.json<{ config: CodingConfig }>('PUT', `${CODING_BASE}/config`, {
        enabled, executable: executable.trim(), codexHome: home.trim(),
      })
      onSaved(result.config)
      if (check && enabled) {
        setStatus(await codingConnection(transport, result.config.revision).json<CodingStatus>('GET', `${CODING_BASE}/status`))
      } else onOpenChange(false)
    } catch (failure) { setError(failure instanceof Error ? failure.message : String(failure)) }
    finally { saving.current = false; setBusy(false) }
  }
  return <Dialog open={open} onOpenChange={value => { if (!busy) onOpenChange(value) }}>
    <DialogContent className='max-w-xl'>
      <DialogHeader className='flex-col items-start'><DialogTitle>{t('本机 Codex 设置')}</DialogTitle><DialogDescription>{t('由 Agent2API 直接启动本机官方 Codex，无需 HAPI 或其他守护程序。')}</DialogDescription></DialogHeader>
      <DialogBody className='gap-4'>
        <Field orientation='horizontal'><FieldLabel htmlFor='coding-enabled'>{t('启用本机 Codex')}</FieldLabel><Switch id='coding-enabled' checked={enabled} onCheckedChange={setEnabled} disabled={busy} /></Field>
        <Field><FieldLabel htmlFor='coding-executable'>{t('Codex 程序路径（可选）')}</FieldLabel>
          <Input id='coding-executable' value={executable} onChange={e => setExecutable(e.target.value)} disabled={busy} placeholder={t('留空自动查找已安装的 Codex')} />
          <FieldDescription>{t('Windows 可填写 codex.exe 或官方 npm 安装的 codex.cmd；不要填写启动参数。')}</FieldDescription></Field>
        <Field><FieldLabel htmlFor='coding-home'>{t('Codex 配置目录（可选）')}</FieldLabel>
          <Input id='coding-home' value={home} onChange={e => setHome(e.target.value)} disabled={busy} placeholder={t('留空沿用 CODEX_HOME 或用户目录下的 .codex')} />
          <FieldDescription>{t('沿用这里已有的登录、模型和历史记录，不会复制或改写你的登录凭证。')}</FieldDescription></Field>
        <div className='space-y-2 rounded-md border border-border bg-muted/30 p-3 text-xs leading-relaxed text-muted-foreground'>
          <p>{t('先在运行 Agent2API 的电脑上安装官方 Codex，并完成登录或 API 配置。手机无需安装 Codex。')}</p>
          <p>{t('以上路径均属于 Agent2API 后台所在电脑；若后台在服务器或容器中，也必须在那里安装和配置 Codex。')}</p>
          <p>{t('在会话中点击“打开官方终端”，电脑终端与手机即可共同查看、发消息和审批；独立启动的旧 Codex 窗口不会被强行接管。')}</p>
        </div>
        <Alert variant='warning'>{t('Codex 可以读写文件和运行命令，仅向受信任的管理员开放。关闭页面不停止任务；退出 Agent2API 会停止它启动的 Codex。')}</Alert>
        {error ? <Alert variant='destructive'>{error}</Alert> : null}
        {status ? <Alert variant={status.authenticated ? 'default' : 'warning'}>
          <p>{status.authenticated ? t('本机 Codex 已就绪：{version}', { version: status.version }) : t('已找到 Codex，但尚未完成登录或 API 配置，请先在电脑端完成。')}</p>
          <p className='mt-1 break-all text-xs'>{status.executable}</p>
        </Alert> : null}
      </DialogBody>
      <DialogFooter><Button variant='outline' disabled={busy} onClick={() => onOpenChange(false)}>{t('取消')}</Button>
        <Button variant='outline' disabled={busy || !enabled} onClick={() => void save(true)}>{t('保存并检查')}</Button>
        <Button disabled={busy} onClick={() => void save(false)}>{busy ? t('正在保存…') : t('保存')}</Button></DialogFooter>
    </DialogContent>
  </Dialog>
}
