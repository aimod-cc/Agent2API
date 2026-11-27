import * as React from 'react'
import {
  Alert,
  Badge,
  Button,
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Empty,
  Field,
  FieldLabel,
  Input,
  SegmentedControl,
  Spinner,
  Switch,
  cn,
} from '@ui'
import { t } from '../i18n'
import { SearchableSelect } from '../../../ui-shared/searchable-select'
import { AgentIcon } from './agent-actions'
import { catalogModel, refreshCatalogModels, useAgentCatalog, type CatalogModel } from './agent-catalog'
import { AgentSelect } from './agent-fields'
import { cloneProfile, profileSignature, protocolLabel, validateDraft } from './agent-model'
import { WritePreview } from './agent-preview'
import {
  ClaudeMappingWorkspace,
  detectClaudeDefaultTier,
  type ClaudeTierKey,
} from './agent-profile-claude'
import {
  SingleModelWorkspace,
  SplitModelsWorkspace,
} from './agent-profile-models'
import { fillQuickProfile } from './agent-quick-model'
import { applyProfile, confirm, errorMessage, getAgentStore, requestPreview, saveConfiguration, toast } from './agent-state'
import type { AgentClient, AgentPreview, AgentProfile } from './agent-types'

function gatewayUrlFor(gateway: string, protocol: string): string {
  return protocol === 'anthropic' ? gateway : `${gateway}/v1`
}

function urlPlaceholder(clientId: string, protocol: string): string {
  if (clientId === 'workbuddy') return t('https://api.example.com/v1（保存时自动补全完整路径）')
  if (protocol === 'anthropic') return t('https://api.anthropic.com（不要附加 /v1/messages）')
  if (protocol === 'gemini') return t('https://generativelanguage.googleapis.com（不要附加 /v1beta）')
  return t('https://api.example.com/v1（通常以 /v1 结尾）')
}

function DialogClientAvatar({ client }: { client: AgentClient }) {
  const [broken, setBroken] = React.useState(false)
  const hasIcon = Boolean(client.icon) && !broken
  return (
    <span
      className={cn('agent-avatar', hasIcon && 'has-icon', hasIcon && client.iconFull && 'is-full')}
      style={{ '--av-h': String(client.hue) } as React.CSSProperties}
      aria-hidden='true'
    >
      {hasIcon ? <img src={client.icon} alt='' onError={() => setBroken(true)} /> : client.letter}
    </span>
  )
}

export function ProfileDialog({ client, profile, creating, onClose }: {
  client: AgentClient
  profile: AgentProfile
  creating: boolean
  onClose: () => void
}) {
  const store = getAgentStore()
  const native = Boolean(profile.liveId)
  const isClaude = client.id === 'claude-code'
  const [draft, setDraft] = React.useState(() => cloneProfile(profile))
  const [claudeDefaultTier, setClaudeDefaultTier] = React.useState<ClaudeTierKey>(() => detectClaudeDefaultTier(profile))
  const [advancedOpen, setAdvancedOpen] = React.useState(false)
  const [error, setError] = React.useState('')
  const [saving, setSaving] = React.useState(false)
  const [preview, setPreview] = React.useState<AgentPreview | null>(null)
  const [previewing, setPreviewing] = React.useState(false)
  const [confirming, setConfirming] = React.useState(false)
  const [detailsOpen, setDetailsOpen] = React.useState(false)

  const { catalog, loading: catalogLoading, error: catalogError, retry: retryCatalog } = useAgentCatalog(
    draft.kind === 'gateway' && draft.authMode === 'gateway' ? draft.gatewayKeyId : undefined
  )

  const initial = React.useRef(profileSignature(profile))
  const nativeRevision = React.useRef(client.nativeRevision)
  const revision = React.useRef(client.revision)
  const configPath = React.useRef(client.configPath)
  const previewClient = { ...client, nativeRevision: nativeRevision.current, revision: revision.current, configPath: configPath.current }
  const previewGeneration = React.useRef(0)
  const previousCatalog = React.useRef<CatalogModel[]>([])
  const initialized = React.useRef(!creating)
  const dirty = profileSignature(draft) !== initial.current
  const operation = 'config-edit' as const
  const busy = saving || previewing

  const single = ['codex', 'claude-code'].includes(client.id) && !draft.baseUrl && draft.authMode === 'preserve' && !draft.models.length
  const validationClient = {
    ...client,
    supportsModels: client.supportsModels && !single,
    ...(native && !profile.enabled && !profile.models.length && !profile.model ? { supportsDefault: false } : {}),
  }

  React.useEffect(() => {
    if (catalogLoading || catalogError) return
    const previous = previousCatalog.current
    previousCatalog.current = catalog
    if (draft.kind !== 'gateway') return
    if (!initialized.current) {
      initialized.current = true
      if (isClaude) {
        setDraft(current => {
          if (current.model || Object.values(current.claudeModels).some(Boolean)) return current
          const preferred = catalog.find(item => item.id === store.defaultModel)
            || catalog.find(item => item.routingGroup)
            || catalog.find(item => item.supportsTools === true)
            || catalog[0]
          if (!preferred) return current
          return {
            ...current,
            model: preferred.id,
            claudeModels: { opus: preferred.id, sonnet: preferred.id, haiku: preferred.id, subagent: preferred.id },
          }
        })
        return
      }
      setDraft(current => current.models.length || current.model ? current : fillQuickProfile(client, current, catalog, store.defaultModel))
      return
    }
    if (!previous.length || !client.supportsModels) return
    previewGeneration.current += 1
    setPreview(null); setConfirming(false); setPreviewing(false)
    setDraft(current => ({ ...current, models: refreshCatalogModels(client, current.models, previous, catalog) }))
  }, [catalogLoading, catalogError, catalog, client, isClaude, draft.kind, store.defaultModel])

  React.useEffect(() => () => { previewGeneration.current += 1 }, [])

  const update = (values: Partial<AgentProfile>) => {
    previewGeneration.current += 1
    setDraft(current => ({ ...current, ...values }))
    setPreview(null); setConfirming(false); setPreviewing(false); setError('')
  }

  const switchMode = (mode: 'gateway' | 'custom') => {
    if (mode === 'gateway') {
      update({
        kind: 'gateway',
        baseUrl: gatewayUrlFor(store.gateway, draft.protocol),
        authMode: client.id === 'zed' ? 'preserve' : 'gateway',
      })
    } else {
      const preservedKind = profile.kind !== 'gateway' ? profile.kind : 'relay'
      update({
        kind: preservedKind,
        baseUrl: draft.kind === 'gateway' ? '' : draft.baseUrl,
        authMode: client.id === 'zed' ? 'preserve' : draft.authMode === 'gateway' ? 'key' : draft.authMode,
      })
    }
  }

  /** 提交前归一化：Claude Code 若填了四档或额外 /model 菜单，确保默认启动模型与后端校验一致 */
  const prepareForSubmit = (source: AgentProfile): AgentProfile => {
    if (!isClaude) return source
    const fallbackModel = source.claudeModels[claudeDefaultTier]
      || source.model
      || source.claudeModels.sonnet
      || source.claudeModels.opus
      || source.claudeModels.haiku
      || source.claudeModels.subagent
      || ''
    let nextModels = source.models
    if (nextModels.length > 0 && fallbackModel && !nextModels.some(m => m.id === fallbackModel)) {
      const fromCatalog = catalog.find(item => item.id === fallbackModel)
      const entry = fromCatalog
        ? catalogModel(client, fromCatalog)
        : { id: fallbackModel, name: fallbackModel, reasoningLevels: [], effort: '' }
      nextModels = [entry, ...nextModels]
    }
    return { ...source, model: fallbackModel, models: nextModels }
  }

  const close = async () => {
    if (saving) return
    if (dirty && !await confirm(t('放弃未保存的修改？'), t('关闭后丢弃本次修改，不会保存配置或改动客户端。'), t('放弃修改'))) return
    onClose()
  }

  const save = async () => {
    if (saving) return
    const prepared = prepareForSubmit(draft)
    const message = validateDraft(validationClient, prepared)
    if (message) { setError(message); return }
    if (native) {
      await showPreview(prepared)
      return
    }
    setSaving(true); setError('')
    try {
      await saveConfiguration(previewClient, prepared, creating)
      onClose(); toast(t('配置已保存，可在列表中点击使用'))
    } catch (err) { setError(errorMessage(err)); setSaving(false) }
  }

  const showPreview = async (prepared: AgentProfile) => {
    const generation = ++previewGeneration.current
    setPreviewing(true); setError('')
    try {
      const result = await requestPreview(previewClient, prepared, operation)
      if (generation === previewGeneration.current) { setDraft(prepared); setPreview(result); setConfirming(true) }
    } catch (err) {
      if (generation === previewGeneration.current) setError(errorMessage(err))
    } finally {
      if (generation === previewGeneration.current) setPreviewing(false)
    }
  }

  const apply = async () => {
    if (!preview || saving) return
    setSaving(true); setError('')
    try { await applyProfile(previewClient, draft, operation, preview); onClose() }
    catch (err) { setError(errorMessage(err)); setPreview(null); setConfirming(false) }
    finally { setSaving(false) }
  }

  const isGateway = draft.kind === 'gateway'
  const connectionMode: 'gateway' | 'custom' = isGateway ? 'gateway' : 'custom'
  const authOptions = client.id === 'zed'
    ? [{ value: 'preserve', label: t('在客户端管理认证') }]
    : [
      { value: 'key', label: t('API Key') },
      ...(client.supportsEnvKey ? [{ value: 'env', label: t('环境变量') }] : []),
      { value: 'none', label: t('无需认证') },
      ...(draft.sourceId ? [{ value: 'preserve', label: t('保留现有认证') }] : []),
    ]
  const gatewayKeyOptions = [
    { value: '', label: t('第一把启用的 Key') },
    ...store.gatewayKeys.map(key => ({ value: key.id, label: key.name })),
  ]
  const selectedKeyLabel = gatewayKeyOptions.find(opt => opt.value === draft.gatewayKeyId)?.label || t('第一把启用的 Key')

  const resolveLabel = (id: string) => catalog.find(item => item.id === id)?.name || id
  const summaryText = React.useMemo(() => {
    if (isClaude) {
      const filled = Object.values(draft.claudeModels).filter(Boolean).length
      const activeId = draft.claudeModels[claudeDefaultTier] || draft.model
      return activeId
        ? t('已配置 {n}/4 个档位映射 · 默认启动 {model}', { n: filled, model: resolveLabel(activeId) })
        : t('已配置 {n}/4 个档位映射', { n: filled })
    }
    if (validationClient.supportsModels) {
      const count = draft.models.filter(m => m.id).length
      return draft.model
        ? t('已选 {n} 项 · 默认 {model}', { n: count, model: resolveLabel(draft.model) })
        : t('已选 {n} 项', { n: count })
    }
    return draft.model ? t('当前模型：{model}', { model: resolveLabel(draft.model) }) : t('请选择或填写模型')
  }, [isClaude, draft, claudeDefaultTier, validationClient.supportsModels, catalog])

  return (
    <Dialog open onOpenChange={(open, details) => { if (!open) { details.cancel(); void close() } }}>
      <DialogContent className='agent-config-dialog agent-redesign-dialog' showCloseButton={!saving}>
        <DialogHeader className='agent-redesign-head'>
          <DialogClientAvatar client={client} />
          <DialogTitle className='truncate'>{creating ? t('新增配置') : t('编辑配置')} · {client.name}</DialogTitle>
          {native ? <Badge variant='info' shape='tag' className='shrink-0'>{t('使用中 · 保存前会预览改动')}</Badge> : null}
        </DialogHeader>

        <DialogBody className={cn('agent-redesign-body', confirming && 'is-confirming')}>
          {!confirming ? (
            <>
              {/* 顶部紧凑连接区 */}
              <section className='agent-conn-bar'>
                <div className='agent-conn-row'>
                  {client.supportsGateway ? (
                    <Field className='agent-conn-kind gap-1'>
                      <FieldLabel className='text-[11.5px] text-subtle'>{t('连接方式')}</FieldLabel>
                      <SegmentedControl<'gateway' | 'custom'>
                        value={connectionMode}
                        disabled={busy}
                        aria-label={t('连接方式')}
                        options={[
                          { value: 'gateway', label: t('本网关') },
                          { value: 'custom', label: t('自定义服务') },
                        ]}
                        onValueChange={switchMode}
                      />
                    </Field>
                  ) : null}

                  <Field className='min-w-[160px] flex-1 gap-1'>
                    <FieldLabel className='text-[11.5px] text-subtle'>{t('配置名称')}</FieldLabel>
                    <Input
                      value={draft.name}
                      disabled={busy}
                      autoComplete='off'
                      placeholder={client.supportsGateway ? 'Agent2API' : t('填写配置名称')}
                      onChange={e => update({ name: e.currentTarget.value })}
                    />
                  </Field>

                  {isGateway ? (
                    store.requiresKey && client.id !== 'zed' ? (
                      <Field className='w-[210px] shrink-0 gap-1'>
                        <FieldLabel className='text-[11.5px] text-subtle'>{t('网关 Key')}</FieldLabel>
                        <SearchableSelect
                          aria-label={t('网关 Key')}
                          value={draft.gatewayKeyId}
                          displayValue={selectedKeyLabel}
                          disabled={busy}
                          className='w-full'
                          onValueChange={gatewayKeyId => update({ gatewayKeyId })}
                          options={gatewayKeyOptions}
                        />
                      </Field>
                    ) : null
                  ) : (
                    <Field className='min-w-[240px] flex-[1.6] gap-1'>
                      <FieldLabel className='text-[11.5px] text-subtle'>{t('接口地址')}</FieldLabel>
                      <Input
                        value={draft.baseUrl}
                        disabled={busy}
                        autoComplete='off'
                        spellCheck={false}
                        className='font-mono text-xs'
                        placeholder={urlPlaceholder(client.id, draft.protocol)}
                        onChange={e => update({ baseUrl: e.currentTarget.value })}
                      />
                    </Field>
                  )}

                  <div className='flex shrink-0 items-end pb-[1px]'>
                    <Button
                      type='button'
                      variant={advancedOpen ? 'secondary' : 'ghost'}
                      size='sm'
                      className='text-subtle'
                      onClick={() => setAdvancedOpen(v => !v)}
                    >
                      <AgentIcon name='sliders' size={13} />
                      {t('更多')}
                    </Button>
                  </div>
                </div>

                {!isGateway ? (
                  <div className='agent-conn-row'>
                    <Field className='w-[175px] shrink-0 gap-1'>
                      <FieldLabel className='text-[11.5px] text-subtle'>{t('认证方式')}</FieldLabel>
                      <AgentSelect
                        label={t('认证方式')}
                        value={draft.authMode === 'gateway' ? 'key' : draft.authMode}
                        options={authOptions}
                        disabled={busy}
                        onChange={authMode => update({ authMode })}
                      />
                    </Field>

                    {draft.authMode === 'key' ? (
                      <Field className='min-w-[220px] flex-1 gap-1'>
                        <FieldLabel className='text-[11.5px] text-subtle'>{t('API Key')}</FieldLabel>
                        <Input
                          type='password'
                          autoComplete='new-password'
                          spellCheck={false}
                          value={draft.apiKey}
                          disabled={busy}
                          placeholder={draft.hasApiKey ? t('留空保留已保存密钥') : t('请输入密钥')}
                          onChange={e => update({ apiKey: e.currentTarget.value })}
                        />
                      </Field>
                    ) : draft.authMode === 'env' ? (
                      <Field className='min-w-[220px] flex-1 gap-1'>
                        <FieldLabel className='text-[11.5px] text-subtle'>{t('环境变量名')}</FieldLabel>
                        <Input
                          value={draft.envKey}
                          disabled={busy}
                          autoComplete='off'
                          spellCheck={false}
                          className='font-mono text-xs'
                          placeholder='OPENAI_API_KEY'
                          onChange={e => update({ envKey: e.currentTarget.value })}
                        />
                      </Field>
                    ) : client.id === 'zed' ? (
                      <div className='flex flex-1 items-end pb-2 text-xs text-muted-foreground'>
                        {t('请在 Zed 提供商设置中保存 API Key；本页不修改系统凭证存储。')}
                      </div>
                    ) : null}

                    {client.protocols.length > 1 ? (
                      <Field className='w-[205px] shrink-0 gap-1'>
                        <FieldLabel className='text-[11.5px] text-subtle'>{t('客户端协议')}</FieldLabel>
                        <AgentSelect
                          label={t('客户端协议')}
                          value={draft.protocol}
                          disabled={busy}
                          options={client.protocols.map(v => ({ value: v, label: protocolLabel(v) }))}
                          onChange={protocol => update({ protocol })}
                        />
                      </Field>
                    ) : null}
                  </div>
                ) : (
                  <div className='agent-conn-meta'>
                    <span>{t('网关地址')} <code>{draft.baseUrl}</code></span>
                    <span>· {protocolLabel(draft.protocol)}</span>
                    {!store.requiresKey ? <span>· {t('网关未启用 Key 鉴权')}</span> : null}
                    {client.id === 'zed' ? <span>· {t('API Key 在 Zed 中管理')}</span> : null}
                    {draft.note ? <span>· {draft.note}</span> : null}
                  </div>
                )}

                <Collapsible open={advancedOpen} onOpenChange={setAdvancedOpen}>
                  <CollapsibleTrigger render={<span className='sr-only' />} />
                  <CollapsibleContent className='mt-2.5 grid grid-cols-1 gap-3 border-t border-hairline pt-2.5 sm:grid-cols-3'>
                    <Field className='gap-1'>
                      <FieldLabel className='text-[11.5px] text-subtle'>{t('备注')}</FieldLabel>
                      <Input
                        value={draft.note}
                        disabled={busy}
                        autoComplete='off'
                        placeholder={t('例如：主力写代码、省钱备用')}
                        onChange={e => update({ note: e.currentTarget.value })}
                      />
                    </Field>
                    <Field className='gap-1'>
                      <FieldLabel className='text-[11.5px] text-subtle'>{t('客户端协议')}</FieldLabel>
                      <AgentSelect
                        label={t('客户端协议')}
                        value={draft.protocol}
                        disabled={busy || (client.protocols.length === 1 && client.protocols.includes(draft.protocol))}
                        options={client.protocols.map(v => ({ value: v, label: protocolLabel(v) }))}
                        onChange={protocol => update({
                          protocol,
                          ...(isGateway ? { baseUrl: gatewayUrlFor(store.gateway, protocol) } : {}),
                        })}
                      />
                    </Field>
                    {isGateway ? (
                      <Field className='gap-1'>
                        <FieldLabel className='text-[11.5px] text-subtle'>{t('自定义接口地址')}</FieldLabel>
                        <Input
                          value={draft.baseUrl}
                          disabled={busy}
                          autoComplete='off'
                          spellCheck={false}
                          className='font-mono text-xs'
                          onChange={e => update({ baseUrl: e.currentTarget.value })}
                        />
                      </Field>
                    ) : null}
                    {client.supportsToggle && native ? (
                      <div className='flex items-center justify-between gap-2 rounded-md border border-border bg-surface px-3 py-2 sm:col-span-3'>
                        <span className='text-xs text-foreground'>{t('启用此配置')}</span>
                        <Switch
                          checked={draft.enabled}
                          disabled={busy}
                          aria-label={t('启用此配置')}
                          onCheckedChange={enabled => update({ enabled })}
                        />
                      </div>
                    ) : null}
                  </CollapsibleContent>
                </Collapsible>
              </section>

              {isGateway && catalogError ? (
                <Alert variant='warning' className='flex shrink-0 items-center justify-between gap-3'>
                  <span>{t('模型目录读取失败，可手动填写真实模型 ID。')} {catalogError}</span>
                  <Button variant='outline' size='xs' onClick={retryCatalog} disabled={busy}>{t('重试')}</Button>
                </Alert>
              ) : null}

              {/* 下半核心区：固定高度模型工作台 */}
              <section className='agent-ws-stage'>
                {isGateway && catalogLoading ? (
                  <Empty className='my-auto py-12'>
                    <Spinner className='size-5 text-primary' />
                    <span className='mt-2 text-xs text-subtle'>{t('正在读取网关模型与路由组目录…')}</span>
                  </Empty>
                ) : isClaude ? (
                  <ClaudeMappingWorkspace
                    client={validationClient}
                    draft={draft}
                    catalog={catalog}
                    disabled={busy}
                    native={native}
                    defaultTier={claudeDefaultTier}
                    onDefaultTierChange={setClaudeDefaultTier}
                    update={update}
                  />
                ) : validationClient.supportsModels ? (
                  <SplitModelsWorkspace
                    client={validationClient}
                    draft={draft}
                    catalog={catalog}
                    disabled={busy}
                    native={native}
                    update={update}
                  />
                ) : (
                  <SingleModelWorkspace
                    client={validationClient}
                    draft={draft}
                    catalog={catalog}
                    disabled={busy}
                    update={update}
                  />
                )}
              </section>
            </>
          ) : preview ? (
            <div className='flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto'>
              <Alert>{t('确认后将更新 {name} 的配置文件，原配置会自动备份。', { name: client.name })}</Alert>
              {preview.warnings.map((warning, index) => (
                <p key={index} className='m-0 text-xs leading-relaxed text-subtle'>{warning}</p>
              ))}
              <Collapsible open={detailsOpen} onOpenChange={setDetailsOpen}>
                <CollapsibleTrigger render={<Button variant='ghost' size='sm' />}>
                  <span className={detailsOpen ? 'agent-chevron is-open' : 'agent-chevron'}><AgentIcon name='chevron' /></span>
                  {t('查看文件改动')}
                </CollapsibleTrigger>
                <CollapsibleContent><WritePreview preview={preview} showWarnings={false} /></CollapsibleContent>
              </Collapsible>
            </div>
          ) : null}

          {error ? <Alert variant='destructive' className='shrink-0'>{error}</Alert> : null}
        </DialogBody>

        <DialogFooter className='agent-config-footer'>
          {confirming ? (
            <Button variant='ghost' disabled={busy} onClick={() => { setConfirming(false); setPreview(null) }}>
              {t('返回编辑')}
            </Button>
          ) : null}
          <span className='mr-auto truncate text-xs text-muted-foreground'>{!confirming ? summaryText : ''}</span>
          <Button variant='outline' disabled={busy} onClick={() => void close()}>{t('取消')}</Button>
          <Button
            disabled={busy || (!creating && !dirty) || (confirming && !preview)}
            onClick={() => void (confirming ? apply() : save())}
          >
            {saving ? t('保存中…') : previewing ? t('处理中…') : confirming ? t('确认保存') : native ? t('预览并保存') : t('保存')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
