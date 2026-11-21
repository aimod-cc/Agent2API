import * as React from 'react'
import { createPortal } from 'react-dom'
import {
  Button, Collapsible, CollapsibleContent, CollapsibleTrigger, Input, Label, SegmentedControl,
  type SegmentedControlOption,
} from '@ui'
import { SearchableSelect } from './searchable-select'
import { t } from '../ui-kit/src/i18n'
import {
  promptEnabled, promptValue, type Method, type Prompt,
} from '../ui-islands/src/islands/add-account-dynamic-fields'
import { PluginCredentialFile, type CredentialUpload } from './plugin-credential-file'
import { SecretTextarea } from './plugin-secret-textarea'
import './plugin-login-form.css'

export { SecretTextarea } from './plugin-secret-textarea'

export function loginMethodTitle(method: Method): string {
  return method.label
}

export function PluginLoginMethodPicker({ methods, value, onChange, disabled, label, extra = [] }: {
  methods: Method[]
  value: string
  onChange(value: string): void
  disabled?: boolean
  label?: string
  extra?: SegmentedControlOption<string>[]
}): React.ReactElement {
  const options = methods.map(method => ({
    value: method.id,
    label: loginMethodTitle(method),
    title: method.reason || method.label,
    disabled: Boolean(method.reason),
  }))
  return <div className='plugin-login-method-section'>
    <Label>{t('添加方式')}</Label>
    <SegmentedControl className='plugin-login-methods' value={value} onValueChange={onChange} disabled={disabled}
      aria-label={label || t('账号登录方式')} options={[...options, ...extra]} />
  </div>
}

export function multilineLoginPrompt(prompt: Pick<Prompt, 'key' | 'type' | 'label'>): boolean {
  return prompt.type === 'text' && (/json|cookie|credential|private.?key|pem|snapshot/i.test(prompt.key)
    || /JSON|Cookie|PEM/i.test(prompt.label))
}

export function PluginLoginFields({ method, values, onChange, prefix, disabled, prompts = method.prompts, upload, onUpload, onReading }: {
  method: Method
  values: Record<string, string>
  onChange(key: string, value: string): void
  prefix: string
  disabled?: boolean
  prompts?: Prompt[]
  upload?: CredentialUpload
  onUpload?(value?: CredentialUpload): void
  onReading?(value: boolean): void
}): React.ReactElement {
  return <div className='plugin-login-fields'>{prompts.map((prompt, index) => {
    if (!promptEnabled(prompt, method, values)) return null
    const id = `${prefix}-prompt-${index}-${prompt.key}`
    const value = promptValue(prompt, values)
    const label = <>{prompt.label}{prompt.optional ? <span className='plugin-login-optional'>{t('可选')}</span> : null}</>
    if (method.file?.field === prompt.key) return <PluginCredentialFile key={`${method.id}:${prompt.key}`}
      spec={method.file} value={value} upload={upload} disabled={disabled} onChange={value => onChange(prompt.key, value)}
      onUpload={value => onUpload?.(value)} onReading={value => onReading?.(value)} />
    if (multilineLoginPrompt(prompt)) return <SecretTextarea key={`${method.id}:${prompt.key}`} id={id} label={label}
      secret={prompt.secret} resetKey={method.id} rows={5} maxLength={262144} value={value} disabled={disabled}
      autoComplete='off' autoCapitalize='none' spellCheck={false} placeholder={prompt.placeholder}
      onChange={event => onChange(prompt.key, event.currentTarget.value)} />
    return <div key={`${method.id}:${prompt.key}`} className='plugin-login-field'>
      <Label htmlFor={id}>{label}</Label>
      {prompt.type === 'select' ? <SearchableSelect id={id} className='plugin-login-select' value={value}
        disabled={disabled || !prompt.options.length} onValueChange={value => onChange(prompt.key, value)}
        searchable={prompt.options.length > 8}
        displayValue={prompt.options.find(option => option.value === value)?.label || prompt.placeholder || t('请选择')}
        options={[...(prompt.optional ? [{ value: '', label: t('不填写（可选）') }] : []), ...prompt.options]} />
        : <Input id={id} value={value} type={prompt.secret ? 'password' : 'text'} disabled={disabled}
          autoComplete={prompt.secret ? 'new-password' : 'off'} autoCapitalize='none' spellCheck={false}
          placeholder={prompt.placeholder} onChange={event => onChange(prompt.key, event.currentTarget.value)} />}
    </div>
  })}</div>
}

export function PluginLoginBrowser({ method, provider, desktop, value, onChange, disabled }: {
  method: Method
  provider: string
  desktop: boolean
  value: 'external' | 'embedded'
  onChange(value: 'external' | 'embedded'): void
  disabled?: boolean
}): React.ReactElement | null {
  if (method.type !== 'oauth' || method.browserMode === 'none' || method.localOnly && !method.embeddedOnly) return null
  const embeddedOnly = method.embeddedOnly
  const choice = desktop && !embeddedOnly && (!method.browserMode || method.browserMode === 'choice')
  if (!choice) return null
  return <div className='plugin-login-browser'>
    <Label>{t('打开方式')}</Label>
    <SegmentedControl className='plugin-login-browser-choice' aria-label={t('网页登录的打开方式')}
      value={embeddedOnly ? 'embedded' : value} disabled={disabled} onValueChange={onChange}
      options={[{ value: 'embedded', label: t('内置窗口') },
        { value: 'external', label: t('系统浏览器') }]} />
  </div>
}

export function PluginLoginOptions({ children, summary, disabled, label }: {
  children: React.ReactNode
  summary?: React.ReactNode
  disabled?: boolean
  label?: string
}): React.ReactElement {
  const [open, setOpen] = React.useState(false)
  return <Collapsible open={open} onOpenChange={setOpen} className='plugin-login-options'>
    <CollapsibleTrigger disabled={disabled} className='plugin-login-options-trigger'>
      <span className='plugin-login-options-label'>
        <svg width='16' height='16' viewBox='0 0 24 24' fill='none' stroke='currentColor' strokeWidth='1.8' aria-hidden='true'>
          <path d='M4 7h16M4 17h16' /><circle cx='9' cy='7' r='2' /><circle cx='15' cy='17' r='2' />
        </svg>{label || t('备注与代理')}
      </span>
      <span className='plugin-login-options-summary'>{summary}
        <svg width='16' height='16' viewBox='0 0 24 24' fill='none' stroke='currentColor' strokeWidth='1.8' aria-hidden='true'><path d='m6 9 6 6 6-6' /></svg>
      </span>
    </CollapsibleTrigger>
    <CollapsibleContent keepMounted className='plugin-login-options-content'>{children}</CollapsibleContent>
  </Collapsible>
}

export function PluginLoginActions({ host, active, children }: {
  host?: HTMLElement | null
  active: boolean
  children: React.ReactNode
}): React.ReactElement {
  const content = <div className='plugin-login-actions acct-add-foot-slot' hidden={!active}>
    <div className='acct-add-foot-buttons'>{children}</div>
  </div>
  return host ? createPortal(content, host) : content
}

export function PluginLoginSecurity(): React.ReactElement {
  return <p className='plugin-login-security'>
    <svg width='16' height='16' viewBox='0 0 24 24' fill='none' stroke='currentColor' strokeWidth='1.8' aria-hidden='true'>
      <path d='m12 3 7 3v5c0 4-3 7-7 10-4-3-7-6-7-10V6l7-3Z' />
    </svg>{t('凭证仅用于当前账号的登录和保存')}
  </p>
}
