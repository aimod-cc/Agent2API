import { useEffect, useState } from 'react'
import { Alert, Button, Dialog, DialogBody, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, Field, FieldDescription, FieldLabel, Input, Switch } from '@ui'
import { CODING_BASE, codingConnection, type CodingConfig, type CodingMachine, type CodingTranslate, type CodingTransport } from './assistant-coding-types'

export function CodingSettings({ open, onOpenChange, config, transport, t, onSaved }: {
  open: boolean; onOpenChange: (open: boolean) => void; config: CodingConfig | null
  transport: CodingTransport; t: CodingTranslate; onSaved: (config: CodingConfig) => void
}) {
  const [url, setUrl] = useState('http://127.0.0.1:3006')
  const [token, setToken] = useState('')
  const [enabled, setEnabled] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')
  useEffect(() => {
    if (!open) return
    setUrl(config?.hubUrl || 'http://127.0.0.1:3006'); setEnabled(config?.enabled ?? false)
    setToken(''); setError(''); setNotice('')
  }, [open])
  const save = async (check: boolean) => {
    if (busy) return
    setBusy(true); setError(''); setNotice('')
    try {
      const result = await transport.json<{ config: CodingConfig }>('PUT', `${CODING_BASE}/config`, { enabled, hubUrl: url, accessToken: token })
      setToken(''); onSaved(result.config)
      if (check && enabled) {
        const status = await codingConnection(transport, result.config.revision).json<{ connected: boolean; machines: CodingMachine[] }>('GET', `${CODING_BASE}/status`)
        setNotice(status.machines.length ? t('连接成功，发现 {count} 台在线电脑', { count: status.machines.length }) : t('后台已连接，尚无在线电脑。请在工作电脑上启动 HAPI Runner。'))
      } else onOpenChange(false)
    } catch (failure) { setError(failure instanceof Error ? failure.message : String(failure)) }
    finally { setBusy(false) }
  }
  return <Dialog open={open} onOpenChange={value => { if (!busy) onOpenChange(value) }}>
    <DialogContent className='max-w-xl'>
      <DialogHeader><DialogTitle>{t('编程会话连接')}</DialogTitle><DialogDescription>{t('连接自己的 HAPI 后台，在这里使用电脑上的 Codex 和 Claude Code。')}</DialogDescription></DialogHeader>
      <DialogBody className='gap-4'>
        <Field orientation='horizontal'><FieldLabel>{t('启用编程会话')}</FieldLabel><Switch checked={enabled} onCheckedChange={setEnabled} disabled={busy} /></Field>
        <Field><FieldLabel htmlFor='coding-hub-url'>{t('HAPI 后台地址')}</FieldLabel><Input id='coding-hub-url' value={url} onChange={e => setUrl(e.target.value)} disabled={busy} placeholder='http://127.0.0.1:3006' />
          <FieldDescription>{t('地址由 Agent2API 后台访问，不是手机本机地址。远程连接需要 HTTPS。')}</FieldDescription></Field>
        <Field><FieldLabel htmlFor='coding-hub-token'>{t('HAPI 连接密钥')}</FieldLabel><Input id='coding-hub-token' type='password' autoComplete='new-password' value={token} onChange={e => setToken(e.target.value)} disabled={busy}
          placeholder={config?.hasToken && config.hubUrl === url.trim().replace(/\/$/, '') ? t('已保存，留空保持不变') : t('填写 HAPI 启动时显示的访问密钥')} />
          <FieldDescription>{t('密钥加密保存在后台，不会回传给手机；更换后台地址时需要重新填写。')}</FieldDescription></Field>
        <div className='space-y-2 rounded-md border border-border bg-muted/30 p-3 text-xs leading-relaxed text-muted-foreground'>
          <p>{t('首次使用需在电脑安装 HAPI，并安装、配置 Codex 或 Claude Code。')}</p>
          <pre className='overflow-x-auto whitespace-pre text-foreground'><code>{'npm install -g @twsxtd/hapi\nhapi hub\nhapi runner start --workspace-root "/path/to/projects"'}</code></pre>
          <p>{t('后台与 Runner 需要保持运行。用允许访问的项目目录替换示例路径，避免开放整台电脑。')}</p>
          <p>{t('已有终端会话需要由 HAPI 管理；这里不会强行接管任意已打开的窗口。')}</p>
        </div>
        <Alert variant='warning'>{t('编程会话可以读写文件、运行命令，仅向受信任的管理员开放。关闭连接不会终止已经运行的任务。')}</Alert>
        {error ? <Alert variant='destructive'>{error}</Alert> : null}
        {notice ? <Alert>{notice}</Alert> : null}
      </DialogBody>
      <DialogFooter><Button variant='outline' disabled={busy} onClick={() => onOpenChange(false)}>{t('取消')}</Button>
        <Button variant='outline' disabled={busy || !enabled} onClick={() => void save(true)}>{t('保存并检查连接')}</Button>
        <Button disabled={busy} onClick={() => void save(false)}>{busy ? t('正在保存…') : t('保存')}</Button></DialogFooter>
    </DialogContent>
  </Dialog>
}
