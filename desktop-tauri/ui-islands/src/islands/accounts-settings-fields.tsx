import * as React from 'react'
import {
  Alert, Badge, Button, Collapsible, CollapsibleContent, CollapsibleTrigger,
  Field, FieldDescription, FieldLabel, Input,
  InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput, Label,
  RadioGroup, RadioGroupItem, Select, SelectContent, SelectItem, SelectTrigger,
  SelectValue, Switch,
} from '@ui'
import { t } from '../i18n'
import { shared } from './accounts-shared'

export type ProviderSettings = {
  id: string
  name?: string
  protocol?: string
  baseUrl?: string
  clientEmulation?: string
}

export function AccountSettingsIcon({ name, size = 16 }: { name: string; size?: number }) {
  return <span className='acct-dlg-icon' aria-hidden='true'
    dangerouslySetInnerHTML={{ __html: shared().wbIcons?.icon?.(name, size) || '' }} />
}

/**
 * 一组设置：分组名在方框外，框内的行以 1px 细线分隔（macOS 系统设置的分组列表）。
 *
 * 标题不塞进框里，是为了让「标题 → 框」的层级留在 DOM 上 —— 样式
 * （`acct-dlg-group-label` + `acct-dlg-group`）与读屏顺序都跟着这个结构走。
 * `action` 是分组名右侧的附加件（计数徽章、清除按钮），没有就不渲染。
 */
export function SettingGroup({ title, action, children }: {
  title: React.ReactNode
  action?: React.ReactNode
  children: React.ReactNode
}) {
  return <section className='acct-dlg-group-wrap'>
    <div className='acct-dlg-group-label'>
      <span>{title}</span>
      {action ? <span className='acct-dlg-label-action'>{action}</span> : null}
    </div>
    <div className='acct-dlg-group'>{children}</div>
  </section>
}

export function BalanceTokenField({ configured, value, onChange, onClear, clearBusy }: {
  configured: boolean
  value: string
  onChange: (next: string) => void
  onClear: () => void
  clearBusy: boolean
}) {
  return <SettingGroup title={t('余额查询凭证')}
    action={<Badge shape='tag' variant={configured ? 'success' : 'secondary'}>{configured ? t('已配置') : t('未配置')}</Badge>}>
    <Field className='acct-dlg-field'>
      <div className='acct-dlg-field-head'><FieldLabel htmlFor='account-balance-token-input'>{t('余额查询凭证')}</FieldLabel>
        {configured ? <Button variant='ghost' size='xs' disabled={clearBusy} onClick={onClear}
          className='acct-dlg-clear'>{t('清除')}</Button> : null}
      </div>
      <Input id='account-balance-token-input' type='password' autoComplete='new-password'
        placeholder={configured ? t('已配置，留空则不修改') : t('一般不用填')}
        value={value} onChange={event => onChange(event.currentTarget.value)} />
      <FieldDescription>{t('积分查询已复用转发用的登录凭证，这里通常留空即可。只有旧版本填过、或从旧代理导入过凭证时才有值')}</FieldDescription>
    </Field>
  </SettingGroup>
}

export function CustomCredentialFields({ hasApiKey, noAuth, value, onValue, onToggleNoAuth, onClear, clearBusy }: {
  hasApiKey: boolean
  noAuth: boolean
  value: string
  onValue: (next: string) => void
  onToggleNoAuth: (next: boolean) => void
  onClear: () => void
  clearBusy: boolean
}) {
  const [revealed, setRevealed] = React.useState(false)
  const pendingKey = Boolean(value.trim())
  return <SettingGroup title={t('账号凭证')}
    action={<Badge shape='tag' variant={noAuth || pendingKey ? 'secondary' : hasApiKey ? 'success' : 'warning'}>
      {noAuth ? t('无需鉴权') : pendingKey ? t('新密钥待保存') : hasApiKey ? t('已配置') : t('未配置凭证')}
    </Badge>}>
    <Field className='acct-dlg-field'>
      <FieldLabel>{t('鉴权方式')}</FieldLabel>
      <RadioGroup value={noAuth ? 'none' : 'key'} disabled={clearBusy} aria-label={t('鉴权方式')}
        className='acct-dlg-choice-grid' onValueChange={next => onToggleNoAuth(next === 'none')}>
        <Label className='acct-dlg-choice' data-selected={!noAuth || undefined}>
          <RadioGroupItem value='key' /><span><strong>API Key</strong><small>{t('使用密钥访问上游')}</small></span>
        </Label>
        <Label className='acct-dlg-choice' data-selected={noAuth || undefined}>
          <RadioGroupItem value='none' /><span><strong>{t('无需鉴权')}</strong><small>{t('本地服务或无需凭证的上游')}</small></span>
        </Label>
      </RadioGroup>
    </Field>
    <Field className='acct-dlg-field'>
      <div className='acct-dlg-field-head'>
        <FieldLabel htmlFor='account-api-key-input'>API Key</FieldLabel>
        {hasApiKey && !noAuth ? <Button variant='ghost' size='xs' disabled={clearBusy} onClick={onClear}
          className='acct-dlg-clear'>{t('清除已存密钥')}</Button> : null}
      </div>
      <InputGroup data-disabled={noAuth || clearBusy}>
        <InputGroupInput id='account-api-key-input' type={revealed && !noAuth ? 'text' : 'password'}
          autoComplete='new-password' spellCheck={false} disabled={noAuth || clearBusy}
          placeholder={hasApiKey ? t('留空保留当前密钥，输入新密钥以替换') : 'sk-…'}
          value={value} onChange={event => onValue(event.currentTarget.value)} />
        <InputGroupAddon align='inline-end'>
          <InputGroupButton size='icon-xs' disabled={noAuth || clearBusy} aria-pressed={revealed}
            aria-label={revealed ? t('隐藏新输入的密钥') : t('显示新输入的密钥')}
            onClick={() => setRevealed(current => !current)}>
            <AccountSettingsIcon name={revealed ? 'eyeOff' : 'eye'} />
          </InputGroupButton>
        </InputGroupAddon>
      </InputGroup>
      <FieldDescription>{t('原密钥不会回显；更换密钥不会改变账号 ID。')}</FieldDescription>
    </Field>
    <div className={`acct-dlg-inline-note${noAuth || !hasApiKey && !pendingKey ? ' warning' : ''}`}>
      <AccountSettingsIcon name={noAuth || !hasApiKey ? 'info' : 'shield'} size={14} />
      <span>{noAuth
        ? t('保存后将移除密钥，转发与拉取模型都不发送鉴权头。请确认上游确实无需凭证。')
        : hasApiKey || pendingKey
          ? t('留空不修改，替换后点击「保存更改」生效。')
          : t('当前没有凭证，转发时会跳过此账号。请填写密钥或选择「无需鉴权」。')}</span>
    </div>
  </SettingGroup>
}

/** 与后端 custom_providers::MAX_NAME_CHARS 一致。 */
const MAX_PROVIDER_NAME_CHARS = 64

export function ProviderSection({ provider, name, protocol, baseUrl, accountBaseUrl, count, emulation,
  onName, onProtocol, onBaseUrl, onEmulation, onRemove, onModels, onClearAccountBaseUrl, disabled }: {
  provider: ProviderSettings
  name: string
  protocol: string
  baseUrl: string
  /** 当前账号自己存的地址覆盖项（空串 = 跟随提供商）；转发时它优先于上面的 Base URL */
  accountBaseUrl: string
  count: number
  emulation: string
  onName: (next: string) => void
  onProtocol: (next: string) => void
  onBaseUrl: (next: string) => void
  onEmulation: (next: string) => void
  onRemove: () => void
  onModels: () => void
  onClearAccountBaseUrl: () => void
  disabled: boolean
}) {
  const [advanced, setAdvanced] = React.useState(emulation === 'opencode')
  const protocols = shared().wbProviders?.PROTOCOL_OPTIONS || [
    { value: 'chat_completions', label: 'OpenAI - Chat Completions' },
    { value: 'responses', label: 'OpenAI - Responses' },
    { value: 'anthropic', label: 'Anthropic - Messages' },
  ]
  const current = protocols.find(option => option.value === protocol)?.label || protocol
  const suffix = protocol === 'anthropic' ? '/v1/messages' : protocol === 'responses' ? '/responses' : '/chat/completions'
  // 请求端点按**这个账号实际会用的地址**算：账号上的覆盖项优先于提供商的 Base URL。
  // 早先版本新建提供商时会把地址抄进首个账号，改提供商地址不生效就是它挡的
  // （issue #455）—— 预览必须如实反映这一点，否则界面显示的和实际发出的又是两个值。
  const override = accountBaseUrl.trim()
  const effectiveBase = override || baseUrl.trim()
  const endpoint = effectiveBase ? `${effectiveBase.replace(/\/+$/, '')}${suffix}` : ''
  return <>
    <Alert variant='warning' className='acct-dlg-scope-notice' role='note'>
      <AccountSettingsIcon name='accounts' />
      <div><strong>{t('正在编辑提供商共用配置')}</strong>
        <p>{t('「{name}」名下 {count} 个账号共用；修改协议或地址可能中断正在进行的请求。', { name: provider.name || provider.id, count })}</p>
      </div>
    </Alert>
    <SettingGroup title={t('连接参数')}>
      <div className='acct-dlg-field-grid'>
        <Field className='acct-dlg-field'>
          <FieldLabel htmlFor='account-provider-name'>{t('提供商名称')}</FieldLabel>
          <Input id='account-provider-name' maxLength={MAX_PROVIDER_NAME_CHARS} disabled={disabled}
            value={name} onChange={event => onName(event.currentTarget.value)} />
        </Field>
        <Field className='acct-dlg-field'>
          <FieldLabel htmlFor='account-provider-protocol'>{t('协议')}</FieldLabel>
          <Select value={protocol} disabled={disabled} onValueChange={value => onProtocol(String(value))}>
            <SelectTrigger id='account-provider-protocol'><SelectValue>{current}</SelectValue></SelectTrigger>
            <SelectContent>{protocols.map(option => <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>)}</SelectContent>
          </Select>
        </Field>
        <Field className='acct-dlg-field acct-dlg-field-full'>
          <div className='acct-dlg-field-head'>
            <FieldLabel htmlFor='account-provider-baseurl'>Base URL</FieldLabel>
            {override ? <Button variant='ghost' size='xs' disabled={disabled}
              className='acct-dlg-clear' onClick={onClearAccountBaseUrl}>{t('改回跟随提供商')}</Button> : null}
          </div>
          <Input id='account-provider-baseurl' className='mono' disabled={disabled} spellCheck={false}
            placeholder={t('OpenAI 兼容填到 /v1；Anthropic 填根地址')}
            value={baseUrl} onChange={event => onBaseUrl(event.currentTarget.value)} />
          <FieldDescription>{t('填写接口基址，无需追加 {path}。', { path: suffix })}</FieldDescription>
          {override ? <div className='acct-dlg-inline-note warning'>
            <AccountSettingsIcon name='info' size={14} />
            <span>{t('这个账号单独存了一份地址：{url}，转发时优先用它。上面改的是提供商共用地址，不会覆盖它。', { url: override })}</span>
          </div> : null}
        </Field>
      </div>
      {endpoint ? <div className='acct-dlg-endpoint'><span>{t('请求端点')}</span><code>{endpoint}</code></div> : null}
      <div className='acct-dlg-model-link'><span>{t('模型清单与映射在「模型管理」中维护。')}</span>
        <Button variant='link' size='xs' disabled={disabled} onClick={onModels}>{t('模型管理')}<AccountSettingsIcon name='caretRight' size={12} /></Button>
      </div>
    </SettingGroup>
    <div className='acct-dlg-group-label'>{t('高级兼容')}</div>
    <Collapsible open={advanced} onOpenChange={setAdvanced} className='acct-dlg-advanced'>
      <CollapsibleTrigger disabled={disabled} className='acct-dlg-advanced-trigger'>
        <AccountSettingsIcon name='sliders' /><span>{t('高级兼容')}</span>
        <small>{emulation === 'opencode' ? t('已开启') : t('默认关闭')}</small>
        <AccountSettingsIcon name='caretRight' size={13} />
      </CollapsibleTrigger>
      <CollapsibleContent className='acct-dlg-advanced-content'>
        <Field className='acct-dlg-toggle-field'>
          <div><FieldLabel htmlFor='account-provider-emulation'>{t('模拟 OpenCode 官方客户端')}</FieldLabel>
            <FieldDescription>{t('仅用于 OpenCode Zen 这类上游，普通提供商请保持关闭。')}</FieldDescription>
          </div>
          <Switch id='account-provider-emulation' checked={emulation === 'opencode'} disabled={disabled}
            onCheckedChange={next => onEmulation(next ? 'opencode' : '')} />
        </Field>
        <p className='detail acct-dlg-advanced-note'>{t('开启后会改写请求体，补齐会话头与占位工具；未填密钥时使用匿名凭证 public。')}</p>
      </CollapsibleContent>
    </Collapsible>
    <div className='acct-dlg-danger'>
      <div><h3>{t('删除提供商')}</h3><p>{t('将同时删除名下 {count} 个账号，无法恢复。', { count })}</p></div>
      <Button variant='outline' size='sm' disabled={disabled} onClick={onRemove} className='acct-dlg-delete'>{t('删除提供商')}</Button>
    </div>
  </>
}
