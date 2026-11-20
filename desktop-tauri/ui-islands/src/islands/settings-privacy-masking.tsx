import * as React from 'react'
import { Alert, Button, Card, CardContent, CardHeader, Label, Switch, Textarea } from '@ui'
import { errorMessage, shared } from './settings-model'
import { t } from '../i18n'

type Policy = { secrets: boolean; personal: boolean; terms: string[] }
const initial: Policy = { secrets: false, personal: false, terms: [] }
function parse(value: unknown): Policy {
  if (!value || typeof value !== 'object') throw new Error(t('服务端未返回本地隐私脱敏设置'))
  const x = value as Record<string, unknown>
  if (typeof x.secrets !== 'boolean' || typeof x.personal !== 'boolean'
    || !Array.isArray(x.terms) || !x.terms.every(term => typeof term === 'string')) {
    throw new Error(t('本地隐私脱敏设置格式无效'))
  }
  return { secrets: x.secrets, personal: x.personal, terms: x.terms as string[] }
}
const equal = (a: Policy, b: Policy) => a.secrets === b.secrets && a.personal === b.personal
  && a.terms.length === b.terms.length && a.terms.every((term, i) => term === b.terms[i])

export function PrivacyMaskingPanel() {
  const [saved, setSaved] = React.useState<Policy | null>(null)
  const [draft, setDraft] = React.useState<Policy>(initial)
  const [loading, setLoading] = React.useState(true)
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')
  const [notice, setNotice] = React.useState('')
  const pending = React.useRef(false)

  const load = React.useCallback(async () => {
    if (pending.current) return
    pending.current = true
    setLoading(true)
    setBusy(true)
    setError('')
    setNotice('')
    try {
      const api = shared().workbuddyDesktop
      if (!api?.getPrivacyMasking) throw new Error(t('当前后端暂不支持本地隐私脱敏'))
      const value = parse(await api.getPrivacyMasking())
      setSaved(value)
      setDraft(value)
    } catch (failure) { setSaved(null); setError(errorMessage(failure)) }
    finally { pending.current = false; setLoading(false); setBusy(false) }
  }, [])
  React.useEffect(() => { void load() }, [load])

  async function save() {
    if (pending.current || !saved) return
    if (draft.terms.length > 100 || draft.terms.some(term => new TextEncoder().encode(term).length > 256)) {
      setError(t('自定义脱敏词最多 100 个，每个不超过 256 字节'))
      return
    }
    pending.current = true
    setBusy(true)
    setError('')
    setNotice('')
    try {
      const api = shared().workbuddyDesktop
      if (!api?.savePrivacyMasking) throw new Error(t('当前后端暂不支持本地隐私脱敏'))
      const value = parse(await api.savePrivacyMasking(draft))
      setSaved(value)
      setDraft(value)
      setNotice(t('设置已保存：从下一个推理请求生效'))
    } catch (failure) { setError(errorMessage(failure)) }
    finally { pending.current = false; setBusy(false) }
  }

  const active = !!saved && (saved.secrets || saved.personal || saved.terms.length > 0)
  const dirty = !!saved && !equal(saved, draft)
  const locked = loading || busy || !saved
  const labelId = React.useId()
  return <Card render={<section />} className='panel' id='settings-privacy-masking'>
    <CardHeader className='flex-row items-center justify-between gap-3'>
      <div className='flex flex-col gap-1'>
        <h2>{t('本地隐私脱敏')}</h2>
        <span className='text-xs text-subtle'>{loading ? t('正在读取设置…') : saved ? t(active ? '已开启' : '已关闭') : t('不可用')}</span>
      </div>
      <Button variant='ghost' size='sm' disabled={busy} onClick={() => void load()}>{t('刷新')}</Button>
    </CardHeader>
    <CardContent className='panel-body flex flex-col gap-3'>
      <p className='m-0 text-xs leading-relaxed text-subtle'>
        {t('默认关闭。仅保护经此网关的 Chat Completions、Responses 和 Messages，返回给客户端的文本和工具参数会还原。')}
      </p>
      <Label htmlFor={labelId + '-secrets'} className='flex items-center justify-between gap-3'>
        <span>{t('遮盖 API Key、Token、密码和已识别的私钥')}</span>
        <Switch id={labelId + '-secrets'} checked={draft.secrets} disabled={locked}
          onCheckedChange={secrets => setDraft(value => ({ ...value, secrets }))} />
      </Label>
      <Label htmlFor={labelId + '-personal'} className='flex items-center justify-between gap-3'>
        <span>{t('遮盖邮箱和中国大陆手机号')}</span>
        <Switch id={labelId + '-personal'} checked={draft.personal} disabled={locked}
          onCheckedChange={personal => setDraft(value => ({ ...value, personal }))} />
      </Label>
      <div className='flex flex-col gap-2'>
        <Label htmlFor={labelId + '-terms'}>{t('自定义脱敏词（每行一个，精确区分大小写）')}</Label>
        <Textarea id={labelId + '-terms'} rows={4} spellCheck={false} disabled={locked}
          value={draft.terms.join('\n')}
          onChange={event => setDraft(value => ({ ...value, terms: event.target.value.split(/\r?\n/) }))} />
      </div>
      <p className='m-0 text-xs leading-relaxed text-subtle'>
        {t('原值与占位符的映射只在本次请求内存中保留。附件、签名内容和面板内置助手不受此设置保护；启用时网关独立生图请求会报错。规则可能漏判，请用测试数据验证。')}
      </p>
      {error ? <Alert variant='destructive' role='alert'>{error}</Alert> : null}
      {notice ? <p className='m-0 text-xs text-subtle' role='status'>{notice}</p> : null}
      <div className='flex justify-end'>
        <Button id='privacy-masking-save' variant='outline' disabled={locked || !dirty} onClick={() => void save()}>
          {busy ? t('保存中…') : t('保存设置')}
        </Button>
      </div>
    </CardContent>
  </Card>
}
