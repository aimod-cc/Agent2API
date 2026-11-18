import { useEffect, useRef, useState } from 'react'
import { Button, Collapsible, CollapsibleContent, CollapsibleTrigger } from '@ui'
import { t } from '../ui-kit/src/i18n'
import type { LoginDiagnostic } from './plugin-login-diagnostic'
import './plugin-login-form.css'

export function PluginLoginDiagnostic({ diagnostic }: { diagnostic?: LoginDiagnostic }) {
  const [open, setOpen] = useState(false)
  const [copied, setCopied] = useState(false)
  const [copyFailed, setCopyFailed] = useState(false)
  const copySource = useRef<HTMLTextAreaElement>(null)
  useEffect(() => { setOpen(false); setCopied(false); setCopyFailed(false) }, [diagnostic])
  useEffect(() => {
    if (!copied) return
    const timer = setTimeout(() => setCopied(false), 2000)
    return () => clearTimeout(timer)
  }, [copied])
  if (!diagnostic) return null
  const stage = diagnostic.stage === 'identity_keys' ? t('验证登录身份') : diagnostic.stage === 'token_refresh'
    ? t('更新登录凭证') : t('换取登录凭证')
  const rows = [
    { label: t('失败步骤'), value: stage },
    { label: t('请求地址'), value: `${diagnostic.method} ${diagnostic.url}` },
    ...(diagnostic.status ? [{ label: t('状态码'), value: `HTTP ${diagnostic.status}` }] : []),
    ...(diagnostic.code ? [{ label: t('错误码'), value: diagnostic.code }] : []),
    ...(diagnostic.requestId ? [{ label: t('请求编号'), value: diagnostic.requestId }] : []),
    ...(diagnostic.route ? [{ label: t('出网方式'), value: diagnostic.route === 'direct' ? t('无代理（直连）') : t('所选代理') }] : []),
  ]
  const notes = [
    ...(diagnostic.redacted ? [t('敏感内容已隐藏；网页响应仅显示可读文字。')] : []),
    ...(diagnostic.truncated ? [t('响应过长，仅显示前 16 KB。')] : []),
    ...(diagnostic.bodyNote === 'response_read_timeout' ? [t('读取响应超时，以下内容可能不完整。')]
      : diagnostic.bodyNote ? [t('读取响应中断，以下内容可能不完整。')] : []),
  ]
  const headers = Object.entries(diagnostic.headers).map(([name, value]) => `${name}: ${value}`).join('\n')
  const body = diagnostic.body || t('OpenAI 未返回响应正文。')
  const text = [rows.map(row => `${row.label}: ${row.value}`).join('\n'),
    headers ? `${t('响应头')}\n${headers}` : '', `${t('响应正文')}\n${body}`, notes.join('\n')].filter(Boolean).join('\n\n')
  async function copy() {
    setCopyFailed(false)
    try {
      await navigator.clipboard.writeText(text)
      setCopied(true)
    } catch {
      try {
        const area = copySource.current
        if (!area) throw new Error('copy_unavailable')
        area.select()
        if (!document.execCommand('copy')) throw new Error('copy_failed')
        setCopied(true)
      } catch { setCopyFailed(true) }
    }
  }
  return <Collapsible open={open} onOpenChange={setOpen} className='plugin-login-diagnostic'>
    <CollapsibleTrigger className='plugin-login-diagnostic-trigger'>
      {t('原始响应')}
      <svg width='16' height='16' viewBox='0 0 24 24' fill='none' stroke='currentColor' strokeWidth='1.8' aria-hidden='true'><path d='m6 9 6 6 6-6' /></svg>
    </CollapsibleTrigger>
    <CollapsibleContent className='plugin-login-diagnostic-content'>
      <dl className='plugin-login-diagnostic-meta'>{rows.map(row => <div key={row.label}><dt>{row.label}</dt><dd>{row.value}</dd></div>)}</dl>
      <pre className='plugin-login-diagnostic-body'>{headers ? `${headers}\n\n${body}` : body}</pre>
      {notes.length ? <p className='plugin-login-note'>{notes.join(' ')}</p> : null}
      <textarea ref={copySource} value={text} readOnly tabIndex={-1} aria-hidden className='plugin-login-diagnostic-copy-source' />
      <Button type='button' variant='outline' size='sm' className='self-start' onClick={() => void copy()}>{copied ? t('已复制') : t('复制原始响应')}</Button>
      {copyFailed ? <p role='status' className='plugin-login-note'>{t('复制失败，请手动选择复制')}</p> : null}
    </CollapsibleContent>
  </Collapsible>
}
