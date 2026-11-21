import * as React from 'react'
import { Alert, Button, Card, CardContent, CardHeader, Label, Switch, Textarea } from '@ui'
import { errorMessage, shared } from './settings-model'
import { t } from '../i18n'

type ExtraKind = 'ssn' | 'passport' | 'iban' | 'birthday' | 'mac' | 'serial' | 'home' | 'userhost' | 'ip' | 'bucket'
type Rule = { kind?: string; prefix?: string; regex?: string }
type Policy = { revision: string; secrets: boolean; personal: boolean; terms: string[]; kinds?: Partial<Record<ExtraKind, boolean>>; terms_any_case?: boolean; terms_whole?: boolean; allow?: string[]; rules?: Rule[] }
const kinds: readonly [ExtraKind, string, boolean][] = [
  ['ssn', '美国社会安全号码（SSN）', true], ['passport', '护照号', true], ['iban', 'IBAN', true],
  ['birthday', '生日', true], ['mac', 'MAC 地址', true], ['serial', '序列号', true],
  ['home', '主目录用户名', false], ['userhost', '登录用户名和主机名', false], ['ip', '公网 IP', false], ['bucket', '云存储桶', false],
]
function normalizeLines(value: string) {
  return [...new Set(value.split(/\r?\n/).map(line => line.trim()).filter(Boolean))]
}
const initial: Policy = { revision: '', secrets: false, personal: false, terms: [] }
function parse(value: unknown): Policy {
  if (!value || typeof value !== 'object') throw new Error(t('服务端未返回本地隐私脱敏设置'))
  const x = value as Record<string, unknown>
  if (typeof x.secrets !== 'boolean' || typeof x.personal !== 'boolean'
    || !Array.isArray(x.terms) || !x.terms.every(term => typeof term === 'string')
    || typeof x.revision !== 'string' || !/^[0-9a-f]{64}$/.test(x.revision)) {
    throw new Error(t('本地隐私脱敏设置格式无效'))
  }
  return x as Policy
}
const ruleText = (policy: Policy) => JSON.stringify(policy.rules ?? [], null, 2)

export function PrivacyMaskingPanel() {
  const [saved, setSaved] = React.useState<Policy | null>(null)
  const [draft, setDraft] = React.useState<Policy>(initial)
  const [allowText, setAllowText] = React.useState('')
  const [rulesText, setRulesText] = React.useState('[]')
  const [loading, setLoading] = React.useState(true)
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')
  const [notice, setNotice] = React.useState('')
  const pending = React.useRef(false)
  const editDraft = (update: (current: Policy) => Policy) => { setError(''); setDraft(update) }
  const editAllowText = (value: string) => { setError(''); setAllowText(value) }
  const editRulesText = (value: string) => { setError(''); setRulesText(value) }

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
      setAllowText((value.allow ?? []).join('\n'))
      setRulesText(ruleText(value))
    } catch (failure) { setSaved(null); setError(errorMessage(failure)) }
    finally { pending.current = false; setLoading(false); setBusy(false) }
  }, [])
  React.useEffect(() => { void load() }, [load])

  async function save() {
    if (pending.current || !saved) return
    const terms = [...new Set(draft.terms.map(term => term.trim()).filter(Boolean))]
    const encoder = new TextEncoder()
    if (terms.length > 100 || terms.some(term => encoder.encode(term).length > 256)) {
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
      let rules: unknown
      try { rules = JSON.parse(rulesText || '[]') }
      catch { throw new Error(t('自定义脱敏规则必须是 JSON 数组')) }
      if (!Array.isArray(rules)) throw new Error(t('自定义脱敏规则必须是 JSON 数组'))
      const value = parse(await api.savePrivacyMasking({ ...draft, terms, allow: normalizeLines(allowText), rules, revision: saved.revision }))
      setSaved(value)
      setDraft(value)
      setAllowText((value.allow ?? []).join('\n'))
      setRulesText(ruleText(value))
      setNotice(t('设置已保存：从下一个推理请求生效'))
    } catch (failure) { setError(t(errorMessage(failure))) }
    finally { pending.current = false; setBusy(false) }
  }

  async function refresh() {
    if (pending.current) return
    if (dirty) {
      pending.current = true
      setBusy(true)
      try {
        const ask = window.wbConfirm?.ask
        if (!ask) throw new Error(t('确认弹窗不可用，请重新打开页面'))
        if (!await ask({ title: t('放弃未保存的修改？'),
          text: t('刷新将丢弃尚未保存的本地隐私脱敏修改，是否继续？'),
          okText: t('放弃修改并刷新'), cancelText: t('取消'), bodyClass: '' })) return
      } catch (failure) { setError(t(errorMessage(failure))); return }
      finally { pending.current = false; setBusy(false) }
    }
    await load()
  }

  const active = !!saved && (saved.secrets || saved.personal || saved.terms.length > 0)
  const dirty = !!saved && (JSON.stringify(saved) !== JSON.stringify(draft)
    || allowText !== (saved.allow ?? []).join('\n') || rulesText !== ruleText(saved))
  const locked = loading || busy || !saved
  const labelId = React.useId()
  React.useEffect(() => { if (dirty) setNotice('') }, [dirty])
  return <Card render={<section />} className='panel' id='settings-privacy-masking'>
    <CardHeader className='flex-row items-center justify-between gap-3'>
      <div className='flex flex-col gap-1'>
        <h2>{t('本地隐私脱敏')}</h2>
        <span className='text-xs text-subtle'>{loading ? t('正在读取设置…') : saved ? t(active ? '已开启' : '已关闭') : t('不可用')}</span>
      </div>
      <Button variant='ghost' size='sm' disabled={busy} onClick={() => void refresh()}>{t('刷新')}</Button>
    </CardHeader>
    <CardContent className='panel-body flex flex-col gap-3'>
      <p className='m-0 text-xs leading-relaxed text-subtle'>
        {t('默认关闭。仅保护经此网关的 Chat Completions、Responses 和 Messages，返回给客户端的文本和工具参数会还原。')}
      </p>
      <Label htmlFor={labelId + '-secrets'} className='flex items-center justify-between gap-3'>
        <span>{t('遮盖 API Key、Token、密码和已识别的私钥')}</span>
        <Switch id={labelId + '-secrets'} checked={draft.secrets} disabled={locked}
          onCheckedChange={secrets => editDraft(value => ({ ...value, secrets }))} />
      </Label>
      <Label htmlFor={labelId + '-personal'} className='flex items-center justify-between gap-3'>
        <span>{t('遮盖邮箱、中国大陆手机号、身份证号和银行卡号')}</span>
        <Switch id={labelId + '-personal'} checked={draft.personal} disabled={locked}
          onCheckedChange={personal => editDraft(value => ({ ...value, personal }))} />
      </Label>
      {draft.personal ? <fieldset className='grid grid-cols-1 gap-2 sm:grid-cols-2'>
        <legend>{t('附加个人信息分类')}</legend>
        {kinds.map(([key, label, fallback]) => <Label key={key} htmlFor={labelId + '-' + key} className='flex items-center justify-between gap-2'>
          {t(label)}<Switch id={labelId + '-' + key} checked={draft.kinds?.[key] ?? fallback} disabled={locked}
            onCheckedChange={enabled => editDraft(value => ({ ...value, kinds: { ...value.kinds, [key]: enabled } }))} />
        </Label>)}
      </fieldset> : null}
      <div className='flex flex-col gap-2'>
        <Label htmlFor={labelId + '-terms'}>{t('自定义脱敏词（每行一个）')}</Label>
        <Textarea id={labelId + '-terms'} rows={4} spellCheck={false} disabled={locked}
          value={draft.terms.join('\n')}
          onChange={event => editDraft(value => ({ ...value, terms: event.target.value.split(/\r?\n/) }))} />
      </div>
      <details>
        <summary>{t('高级脱敏规则')}</summary>
        <div className='mt-2 flex flex-col gap-2'>
          <Label htmlFor={labelId + '-fold'} className='flex items-center justify-between gap-2'>
            {t('自定义词忽略大小写')}<Switch id={labelId + '-fold'} checked={draft.terms_any_case ?? false} disabled={locked}
              onCheckedChange={enabled => editDraft(value => ({ ...value, terms_any_case: enabled }))} />
          </Label>
          <Label htmlFor={labelId + '-whole'} className='flex items-center justify-between gap-2'>
            {t('自定义词只匹配整词')}<Switch id={labelId + '-whole'} checked={draft.terms_whole ?? false} disabled={locked}
              onCheckedChange={enabled => editDraft(value => ({ ...value, terms_whole: enabled }))} />
          </Label>
          <Label htmlFor={labelId + '-allow'}>{t('原样放行的值（每行一个）')}</Label>
          <Textarea id={labelId + '-allow'} disabled={locked} value={allowText} onChange={event => editAllowText(event.target.value)} rows={3} />
          <Label htmlFor={labelId + '-rules'}>{t('前缀或正则规则（开启凭据脱敏时生效）')}</Label>
          <Textarea id={labelId + '-rules'} disabled={locked} value={rulesText} onChange={event => editRulesText(event.target.value)} rows={5} spellCheck={false}
            placeholder={'[{"kind":"CUSTOM","prefix":"service_"}]'} />
          <div className='text-xs text-subtle'>{t('每条规则填 prefix 或 regex，正则有捕获组时只遮盖第一组。自定义词支持同一天的日期变体。')}</div>
        </div>
      </details>
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
