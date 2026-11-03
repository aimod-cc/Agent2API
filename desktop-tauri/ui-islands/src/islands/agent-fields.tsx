import * as React from 'react'
import { Button, Field, FieldLabel, FieldDescription, Input, Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from '@ui'
import { t } from '../i18n'
import type { AgentClient, AgentProfile } from './agent-types'
import type { CatalogModel } from './agent-catalog'
import { protocolLabel, syncClaudeTiers } from './agent-model'
import { getAgentStore } from './agent-state'

export function AgentSelect({ value, options, onChange, disabled, label }: {
  value: string; options: Array<{ value: string; label: string }>; onChange: (value: string) => void; disabled?: boolean; label: string
}) {
  const selected = options.find(option => option.value === value)?.label || value || t('请选择')
  return <Select value={value} disabled={disabled} onValueChange={next => { if (typeof next === 'string') onChange(next) }}>
    <SelectTrigger aria-label={label} title={selected} className='w-full'><SelectValue>{selected}</SelectValue></SelectTrigger>
    <SelectContent>{options.map(option => <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>)}</SelectContent>
  </Select>
}

export function TextField({ label, fieldKey, value, onChange, hint, type = 'text', disabled, placeholder }: {
  label: string; fieldKey: string; value: string; onChange: (value: string) => void; hint?: string; type?: string; disabled?: boolean; placeholder?: string
}) {
  const id = React.useId()
  return <Field><FieldLabel htmlFor={id}>{label}<code className='ml-auto text-[10px] text-subtle'>{fieldKey}</code></FieldLabel>
    <Input id={id} type={type} autoComplete={type === 'password' ? 'new-password' : 'off'} spellCheck={false} value={value}
      onChange={event => onChange(event.currentTarget.value)} disabled={disabled} placeholder={placeholder} />
    {hint && <FieldDescription>{hint}</FieldDescription>}
  </Field>
}

/**
 * 网关模型下拉：列目录里的模型；当前值不在目录里时补一项，避免导入或手填的值被悄悄清掉。
 *
 * 「模型 ID 让用户敲」这件事只在读不到目录时才成立 —— 能选就不该让人手输。
 */
export function GatewayModelSelect({ label, value, catalog, onChange, disabled, allowEmpty, emptyLabel }: {
  label: string; value: string; catalog: CatalogModel[]; onChange: (value: string) => void
  disabled?: boolean; allowEmpty?: boolean; emptyLabel?: string
}) {
  const options = catalog.map(model => ({ value: model.id, label: model.label }))
  if (value && !options.some(option => option.value === value)) options.unshift({ value, label: t('{id}（不在网关目录）', { id: value }) })
  if (allowEmpty) options.unshift({ value: '', label: emptyLabel || t('不写入') })
  return <AgentSelect label={label} value={value} options={options} disabled={disabled} onChange={onChange} />
}

/**
 * Claude Code 的四档模型映射（Opus / Sonnet / Haiku / 子 Agent）。
 *
 * Claude Code 自己按档位挑模型（主模型、后台小任务、子代理各走一档），所以接网关时
 * 不是「选一个模型」而是「每档指一个网关模型」—— 四档挤成一列全宽输入框既占地方又
 * 要求用户背模型 ID，这里按两列排、每档给下拉，另配「一键设置」把主模型铺到四档。
 */
export function ClaudeModelFields({ draft, update, catalog, disabled, note }: {
  draft: AgentProfile; update: (patch: Partial<AgentProfile>) => void; catalog: CatalogModel[]; disabled: boolean; note?: string
}) {
  const rows: Array<{ key: 'opus' | 'sonnet' | 'haiku' | 'subagent'; label: React.ReactNode; labelText: string; fieldKey: string }> = [
    { key: 'opus', label: 'Opus', labelText: 'Opus', fieldKey: 'ANTHROPIC_DEFAULT_OPUS_MODEL' },
    { key: 'sonnet', label: 'Sonnet', labelText: 'Sonnet', fieldKey: 'ANTHROPIC_DEFAULT_SONNET_MODEL' },
    { key: 'haiku', label: 'Haiku', labelText: 'Haiku', fieldKey: 'ANTHROPIC_DEFAULT_HAIKU_MODEL' },
    { key: 'subagent', label: t('子 Agent'), labelText: t('子 Agent'), fieldKey: 'CLAUDE_CODE_SUBAGENT_MODEL' },
  ]
  const pick = draft.kind === 'gateway' && catalog.length > 0
  const fill = () => update({ claudeModels: { opus: draft.model, sonnet: draft.model, haiku: draft.model, subagent: draft.model } })
  const setTier = (key: string, value: string) => update({ claudeModels: { ...draft.claudeModels, [key]: value } })
  return <section className='agent-claude-models flex min-w-0 flex-col gap-3 rounded-md border border-border bg-surface-inset p-3'>
    <div className='flex flex-wrap items-center gap-2'>
      <h4 className='text-xs font-semibold'>{t('模型映射')}</h4><span className='mr-auto' />
      <Button variant='outline' size='xs' disabled={disabled || !draft.model} onClick={fill}>{t('一键设置')}</Button>
    </div>
    <div className='grid min-w-0 grid-cols-1 gap-3 sm:grid-cols-2'>
      {rows.map(row => <Field key={row.key}>
        <FieldLabel>{row.label}<code className='ml-auto text-[10px] text-subtle'>{row.fieldKey}</code></FieldLabel>
        {pick ? <GatewayModelSelect label={row.labelText} value={draft.claudeModels[row.key]} catalog={catalog} disabled={disabled} allowEmpty
          emptyLabel={t('不写入')} onChange={value => setTier(row.key, value)} />
          : <Input value={draft.claudeModels[row.key]} autoComplete='off' spellCheck={false} disabled={disabled} placeholder={t('留空表示不写入')} onChange={event => setTier(row.key, event.currentTarget.value)} />}
      </Field>)}
    </div>
    <p className='text-xs leading-relaxed text-subtle'>{note || t('Claude Code 按 Opus / Sonnet / Haiku / 子 Agent 分档请求模型；这里把每档指向网关目录里的模型，留空表示不改写该档。')}</p>
    {pick && rows.some(row => !draft.claudeModels[row.key]) && <p className='text-xs leading-relaxed text-warning'>{t('留空的分档会以 Claude Code 内置模型名请求网关；网关没有同名模型时该档会失败，建议点「一键设置」。')}</p>}
  </section>
}

export function ConnectionFields({ client, draft, update, disabled }: {
  client: AgentClient; draft: AgentProfile; update: (patch: Partial<AgentProfile>) => void; disabled: boolean
}) {
  const store = getAgentStore()
  const authOptions = client.id === 'zed' ? [{ value: 'preserve', label: t('在客户端管理认证') }]
    : [ ...(client.supportsGateway && draft.kind === 'gateway' ? [{ value: 'gateway', label: t('使用网关 Key') }] : []),
      { value: 'key', label: t('API Key') }, ...(client.supportsEnvKey ? [{ value: 'env', label: t('环境变量') }] : []),
      { value: 'none', label: t('无需认证') }, ...(draft.sourceId ? [{ value: 'preserve', label: t('保留导入的认证') }] : []) ]
  const switchSource = (kind: string) => {
    const baseUrl = kind === 'gateway' ? draft.protocol === 'anthropic' ? store.gateway : `${store.gateway}/v1` : draft.baseUrl
    update({ kind, baseUrl, authMode: client.id === 'zed' ? 'preserve' : kind === 'gateway' ? 'gateway' : draft.authMode === 'gateway' ? 'key' : draft.authMode })
  }
  return <div className='agent-connection-fields grid min-w-0 grid-cols-1 gap-4 sm:grid-cols-2'>
    <Field><FieldLabel>{t('配置来源')}</FieldLabel><AgentSelect label={t('配置来源')} value={draft.kind} disabled={disabled} onChange={switchSource}
      options={[...(client.supportsGateway ? [{ value: 'gateway', label: t('本网关') }] : []), { value: 'official', label: t('官方直连') }, { value: 'relay', label: t('第三方中转') }, { value: 'local', label: t('本机服务') }]} /></Field>
    <Field><FieldLabel>{t('客户端协议')}</FieldLabel><AgentSelect label={t('客户端协议')} value={draft.protocol} disabled={disabled || client.protocols.length === 1 && client.protocols.includes(draft.protocol)}
      onChange={protocol => update({ protocol, ...(draft.kind === 'gateway' ? { baseUrl: protocol === 'anthropic' ? store.gateway : `${store.gateway}/v1` } : {}) })}
      options={client.protocols.map(value => ({ value, label: protocolLabel(value) }))} />
      <FieldDescription>{t('这里只选择客户端实际支持的协议，不改变上游提供商协议。')}</FieldDescription></Field>
    <div className='sm:col-span-2'><TextField label={t('接口基址')} fieldKey={urlField(client.id)} value={draft.baseUrl} onChange={baseUrl => update({ baseUrl })} disabled={disabled}
      hint={client.id === 'workbuddy' ? t('保存时自动写入完整的 /chat/completions 路径。') : draft.protocol === 'gemini' ? t('填写服务根地址，不要附加 /v1beta 或 generateContent 路径。') : draft.protocol === 'anthropic' ? t('不要添加 /v1/messages；客户端会自行追加请求路径。') : t('通常以 /v1 结尾，不要附加具体请求路径。')} /></div>
    <Field><FieldLabel>{t('认证方式')}</FieldLabel><AgentSelect label={t('认证方式')} value={draft.authMode} options={authOptions} onChange={authMode => update({ authMode })} disabled={disabled} /></Field>
    {draft.authMode === 'gateway' && store.requiresKey && <Field><FieldLabel>{t('网关 Key')}</FieldLabel>
      <AgentSelect label={t('网关 Key')} value={draft.gatewayKeyId} disabled={disabled} onChange={gatewayKeyId => update({ gatewayKeyId })}
        options={[{ value: '', label: t('第一把启用的 Key') }, ...store.gatewayKeys.map(key => ({ value: key.id, label: key.name }))]} />
      <FieldDescription>{t('应用时使用当前启用的真实 Key；预览会隐藏密钥。')}</FieldDescription></Field>}
    {draft.authMode === 'gateway' && !store.requiresKey && <p className='text-xs text-subtle'>{t('当前网关未启用 API Key 鉴权；占位值只用于客户端要求非空密钥的场景。')}</p>}
    {draft.authMode === 'key' && <TextField label={t('API Key')} fieldKey={client.id === 'codex' ? 'experimental_bearer_token' : client.id === 'claude-code' ? 'env.ANTHROPIC_API_KEY' : 'apiKey'} type='password' value={draft.apiKey} onChange={apiKey => update({ apiKey })} disabled={disabled}
      placeholder={draft.hasApiKey ? t('留空保留已保存密钥') : t('请输入密钥')} hint={t('方案中的密钥加密保存；客户端原生配置可能要求明文。')} />}
    {draft.authMode === 'env' && <TextField label={t('环境变量名')} fieldKey='env_key' value={draft.envKey} onChange={envKey => update({ envKey })} disabled={disabled}
      hint={t('只保存变量引用，不修改系统环境；请在启动客户端的环境里设置变量值。')} />}
    {client.id === 'zed' && <p className='text-xs leading-relaxed text-subtle'>{t('请在 Zed 提供商设置中保存 API Key；本页不修改系统凭证存储。')}</p>}
  </div>
}

function urlField(id: string): string {
  return { 'claude-code': 'env.ANTHROPIC_BASE_URL', codex: 'model_providers.<id>.base_url', 'gemini-cli': 'GOOGLE_GEMINI_BASE_URL', opencode: 'provider.<id>.options.baseURL', zcode: 'config.api.baseUrl', workbuddy: 'models[].url', cline: 'providers.<id>.settings.baseUrl', 'kimi-code': 'providers.<id>.base_url', crush: 'providers.<id>.base_url', dsh: 'llm-pi-ai.providers.<id>.baseURL', zed: 'language_models.openai_compatible.<id>.api_url' }[id] || 'baseUrl'
}

export function SingleModelFields({ client, draft, update, catalog, disabled }: {
  client: AgentClient; draft: AgentProfile; update: (patch: Partial<AgentProfile>) => void; catalog: CatalogModel[]; disabled: boolean
}) {
  const selected = catalog.find(model => model.id === draft.model)
  const allowed = client.id === 'claude-code' ? ['low','medium','high','xhigh','max','auto'] : ['none','minimal','low','medium','high','xhigh']
  const levels = selected?.reasoningLevels.filter(level => allowed.includes(level)) || []
  const pick = draft.kind === 'gateway' && catalog.length > 0
  const modelKey = client.id === 'claude-code' ? 'model / ANTHROPIC_MODEL' : client.id === 'gemini-cli' ? 'model.name' : 'model'
  // 改主模型时把仍跟随的档一起带过去；Claude Code 的四档通常就等于主模型。
  const changeModel = (model: string) => update(client.id === 'claude-code'
    ? { model, effort: '', claudeModels: syncClaudeTiers(draft.claudeModels, draft.model, model) }
    : { model, effort: '' })
  return <>
    <div className='grid min-w-0 grid-cols-1 gap-4 sm:grid-cols-2'>
      <Field><FieldLabel>{pick ? t('从网关目录选择模型') : t('模型 ID')}<code className='ml-auto text-[10px] text-subtle'>{modelKey}</code></FieldLabel>
        {pick ? <GatewayModelSelect label={t('从网关目录选择模型')} value={draft.model} catalog={catalog} disabled={disabled} onChange={changeModel} />
          : <Input value={draft.model} autoComplete='off' spellCheck={false} disabled={disabled} onChange={event => changeModel(event.currentTarget.value)} />}
        <FieldDescription>{t('使用真实模型 ID，不要填写模型展示名。')}</FieldDescription>
      </Field>
      {client.supportsEffort && (levels.length > 0 || draft.effort) && <Field><FieldLabel>{t('思考等级')}<code className='ml-auto text-[10px]'>{client.id === 'codex' ? 'model_reasoning_effort' : 'CLAUDE_CODE_EFFORT_LEVEL'}</code></FieldLabel>
        <AgentSelect label={t('思考等级')} value={draft.effort} disabled={disabled} onChange={effort => update({ effort })}
          options={[{ value: '', label: t('模型默认') }, ...[...new Set([...levels, ...(draft.effort ? [draft.effort] : [])])].map(value => ({ value, label: value }))]} /></Field>}
    </div>
    {client.id === 'claude-code' && <ClaudeModelFields draft={draft} update={update} catalog={catalog} disabled={disabled} />}
    {client.supportsEffort && levels.length === 0 && !draft.effort && <p className='text-xs text-subtle'>{t('模型未声明思考档位，保留客户端默认，不显示猜测选项。')}</p>}
  </>
}
