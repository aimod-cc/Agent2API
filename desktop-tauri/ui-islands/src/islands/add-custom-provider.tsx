/**
 * Agent2API · 「添加账号」弹窗：自定义提供商（新建 / 加入已有）。
 *
 * 替换旧 ui/add-custom-provider.js。自定义提供商（custom- 前缀，运行期数据）的
 * 添加方式有两种：新建提供商 + 首个账号（POST /api/custom-providers）、往已有
 * 提供商再加账号（POST /api/accounts）。
 *
 * 与「账号设置」对齐的分栏分组设计：
 *   · 连接与凭证（本文件渲染在左侧首栏）：按「提供商连接」「账号凭证」分组呈现，
 *     并在 Base URL 下方实时预览完整请求地址、提示重复路径并支持一键清理；
 *   · 转发行为 / 查询设置（含余额查询方案凭证）/ 限制器 / 网络：由右侧分栏统管，
 *     提交时一并校验并落库；
 *   · 创建自定义提供商或其首个（且尚未配置模型的）账号成功后，自动弹出
 *     「获取模型」窗口拉取上游模型列表，省去再切到模型管理手动获取的步骤。
 */

import * as React from 'react'
import {
  Button,
  Field,
  FieldDescription,
  FieldLabel,
  Input,
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
  Label,
  RadioGroup,
  RadioGroupItem,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@ui'

import {
  accountCountOf,
  clearFields,
  describeError,
  draftProps,
  readField,
  setDraftValue,
  shared,
  toast,
  type CustomProviderRecord,
} from './add-account-bridge'
import { AccountSettingsIcon, SettingGroup } from './accounts-settings-fields'
import { customProtocolLabel, previewCustomEndpointUrl } from './models-custom-source'
import { DuplicateProviderDialog } from './add-provider-duplicate-dialog'
import { presetOfProvider, sameProviderEndpoint } from './add-provider-metadata'
import { PresetSponsorCard } from './add-provider-sponsor'
import { useAddAccountProxy, useAccountAddBusy } from './add-account-proxy'
import { useAddAccountSettings, type AddPaneId } from './add-account-settings'
import { customUserAgentEditable } from './add-account-user-agent'
import type { ProxyPayload } from './accounts-proxy-form'
import { t } from '../i18n'

/** 展示名长度上限（与后端 custom_providers::MAX_NAME_CHARS 一致，前端先挡一次） */
const MAX_NAME_CHARS = 64
/** 账号备注名长度上限（与 add-provider-forms 的 MAX_NAME_LENGTH 同一口径） */
const MAX_ACCOUNT_NAME_CHARS = 100

/** Base URL 的两种填法：Anthropic 走根地址，OpenAI 兼容要带 /v1（后端按协议拼路径） */
const BASE_HINT_OPENAI = t('OpenAI 兼容填到 /v1；无需追加 /chat/completions 或 /responses')
const BASE_HINT_ANTHROPIC = t('Anthropic 协议填根地址（无需追加 /v1/messages）')
const BASE_PLACEHOLDER_OPENAI = 'https://open.bigmodel.cn/api/paas/v4'
const BASE_PLACEHOLDER_ANTHROPIC = 'https://api.anthropic.com'

/** 协议下拉的兜底选项：值必须与后端 PROTOCOLS 逐字一致（选项定义收在 providers.js） */
const FALLBACK_PROTOCOLS = [
  { value: 'chat_completions', label: 'OpenAI - Chat Completions' },
  { value: 'responses', label: 'OpenAI - Responses' },
  { value: 'anthropic', label: 'Anthropic - Messages' },
]

const PROTOCOL_ID = 'custom-protocol-select'
const NAME_ID = 'custom-name-input'
const BASEURL_ID = 'custom-baseurl-input'
const APIKEY_ID = 'custom-apikey-input'
const NOAUTH_ID = 'custom-noauth-check'
const EXISTING_APIKEY_ID = 'custom-existing-apikey-input'
const EXISTING_NOAUTH_ID = 'custom-existing-noauth-check'
const EXISTING_NAME_ID = 'custom-existing-name-input'

export const CREATE_BUTTON_ID = 'custom-create-button'
export const EXISTING_BUTTON_ID = 'custom-existing-button'
export const REMOVE_BUTTON_ID = 'custom-existing-remove'

/** 提交互斥锁：弹窗内的提交不占用账号列表的 busy 锁（与内置家的 addBusy 同一取向） */
let submitBusy = false

const protocolOptions = (): Array<{ value: string; label: string }> =>
  shared().wbProviders?.PROTOCOL_OPTIONS || FALLBACK_PROTOCOLS

const customList = (): CustomProviderRecord[] => shared().wbProviders?.customList?.() || []

/** 预置卡声明的方案（空串 = 这个预置没有余额方案） */
function presetSchemeOf(presetKey: string): string {
  if (!presetKey) return ''
  const preset = shared().wbPresetProviders?.presetOf?.(presetKey)
  return String(preset?.balance?.scheme || '')
}

/** 识别用户是否把完整的接口路径误贴进了 Base URL 末尾，并返回可一键清理后的前缀 */
function detectTrailingEndpoint(baseUrl: string, protocol: string): string | null {
  const trimmed = baseUrl.trim().replace(/\/+$/, '')
  if (!trimmed) return null
  if (protocol === 'anthropic') {
    if (/\/v1\/messages$/i.test(trimmed)) return trimmed.replace(/\/v1\/messages$/i, '')
    if (/\/messages$/i.test(trimmed)) return trimmed.replace(/\/messages$/i, '')
    return null
  }
  if (/\/(?:chat\/completions|responses|messages)$/i.test(trimmed)) {
    return trimmed.replace(/\/(?:chat\/completions|responses|messages)$/i, '')
  }
  return null
}

/** 提交按钮的忙态包装（与内置家的 runAdd 同一形制；锁是模块级的，弹窗内互斥） */
async function runSubmit(setBusy: (value: boolean) => void, task: () => Promise<void>): Promise<void> {
  if (submitBusy) return
  submitBusy = true
  setBusy(true)
  try {
    await task()
  } finally {
    submitBusy = false
    setBusy(false)
  }
}

/** 添加成功后的统一收尾：关弹窗、刷新目录与账号列表、提示，并在首次添加时自动弹出模型获取窗口 */
async function afterCustomAdd(
  message: string,
  autoFetchTarget?: { providerId: string; name: string },
): Promise<void> {
  shared().wbAddAccountModal?.close?.()
  await shared().wbProviders?.refreshCustom?.()
  await shared().wbApp?.refresh?.()
  toast(message)
  if (autoFetchTarget?.providerId) {
    shared().wbModelsFetchModal?.open?.({
      providerId: autoFetchTarget.providerId,
      custom: true,
      name: autoFetchTarget.name || autoFetchTarget.providerId,
      autoFetch: true,
      onDone: () => { shared().wbModelsPanel?.render?.() },
    })
  }
}

/* ─── 「无需鉴权」状态草稿与凭证分组 ─────────────── */

function writeNoAuthDraft(id: string, value: boolean): void {
  setDraftValue(id, value ? '1' : '')
}

function readNoAuthDraft(id: string): boolean {
  return readField(id) === '1'
}

/** 账号凭证分组：与「账号设置 → 凭证设置」保持一致的卡片排版（API Key / 无需鉴权 + 密钥显隐切换） */
function CredentialGroup({
  apiKeyId,
  apiKeyValue,
  onApiKeyChange,
  noAuth,
  onToggleNoAuth,
  noAuthNote,
  extraBottom,
}: {
  apiKeyId: string
  apiKeyValue: string
  onApiKeyChange: (next: string) => void
  noAuth: boolean
  onToggleNoAuth: (next: boolean) => void
  noAuthNote: string
  extraBottom?: React.ReactNode
}): React.ReactElement {
  const [revealed, setRevealed] = React.useState(false)
  return (
    <SettingGroup title={t('账号凭证')}>
      <Field className='acct-dlg-field'>
        <FieldLabel>{t('鉴权方式')}</FieldLabel>
        <RadioGroup
          value={noAuth ? 'none' : 'key'}
          aria-label={t('鉴权方式')}
          className='acct-dlg-choice-grid'
          onValueChange={next => onToggleNoAuth(next === 'none')}
        >
          <Label className='acct-dlg-choice' data-selected={!noAuth || undefined}>
            <RadioGroupItem value='key' />
            <span><strong>API Key</strong><small>{t('使用密钥访问上游')}</small></span>
          </Label>
          <Label className='acct-dlg-choice' data-selected={noAuth || undefined}>
            <RadioGroupItem value='none' />
            <span><strong>{t('无需鉴权')}</strong><small>{t('本地服务或无需凭证的上游')}</small></span>
          </Label>
        </RadioGroup>
      </Field>
      <Field className='acct-dlg-field'>
        <FieldLabel htmlFor={apiKeyId}>API Key</FieldLabel>
        <InputGroup data-disabled={noAuth}>
          <InputGroupInput
            id={apiKeyId}
            type={revealed && !noAuth ? 'text' : 'password'}
            autoComplete='new-password'
            spellCheck={false}
            disabled={noAuth}
            placeholder='sk-…'
            value={apiKeyValue}
            onChange={event => onApiKeyChange(event.currentTarget.value)}
          />
          <InputGroupAddon align='inline-end'>
            <InputGroupButton
              size='icon-xs'
              disabled={noAuth}
              aria-pressed={revealed}
              aria-label={revealed ? t('隐藏密钥') : t('显示密钥')}
              onClick={() => setRevealed(current => !current)}
            >
              <AccountSettingsIcon name={revealed ? 'eyeOff' : 'eye'} />
            </InputGroupButton>
          </InputGroupAddon>
        </InputGroup>
        <FieldDescription>
          {noAuth
            ? t('无需 Key：转发与拉取模型都不发送鉴权头')
            : t('输入上游颁发的 API Key；若上游免鉴权，请切换为上方「无需鉴权」。')}
        </FieldDescription>
      </Field>
      {extraBottom}
      <div className={`acct-dlg-inline-note${noAuth ? '' : ' muted'}`}>
        <AccountSettingsIcon name={noAuth ? 'info' : 'shield'} size={14} />
        <span>{noAuthNote}</span>
      </div>
    </SettingGroup>
  )
}

/* ─── 表单段 ─────────────────────────────── */

export type CustomMode = 'create' | 'existing'

export function CustomProviderBlock({
  mode,
  providerHint,
  presetKey,
  showToken,
  version,
}: {
  mode: CustomMode
  /** 第 1 步点的是某一家已有提供商时的 id（空串 = 没有指定） */
  providerHint: string
  /** 第 1 步点的是预置家卡片时的 key（空串 = 不是从预置卡进来的） */
  presetKey: string
  /** 第 1 步每点一次卡就 +1：让预填在「再次点同一张预置卡」时也重新执行 */
  showToken: number
  /** 目录刷新后的重画信号 */
  version: number
}): React.ReactElement {
  const options = protocolOptions()
  const [protocol, setProtocol] = React.useState(() => readField(PROTOCOL_ID) || options[0].value)
  const [baseUrlDraft, setBaseUrlDraft] = React.useState(() => readField(BASEURL_ID))
  const [createApiKey, setCreateApiKey] = React.useState(() => readField(APIKEY_ID))
  const [existingApiKey, setExistingApiKey] = React.useState(() => readField(EXISTING_APIKEY_ID))
  const [baseHintOverride, setBaseHintOverride] = React.useState('')
  const [basePlaceholderOverride, setBasePlaceholderOverride] = React.useState('')
  const [noAuth, setNoAuth] = React.useState(() => readNoAuthDraft(NOAUTH_ID))
  const [existingNoAuth, setExistingNoAuth] = React.useState(() => readNoAuthDraft(EXISTING_NOAUTH_ID))

  const list = React.useMemo(() => customList(), [version])
  const pickedProvider = list.find(item => item.id === providerHint)
  const pickedName = pickedProvider?.name || t('该提供商')
  const createPreset = mode === 'create' && presetKey ? shared().wbPresetProviders?.presetOf?.(presetKey) : null
  const selfHosted = mode === 'create' && Boolean(presetKey) && !createPreset?.baseUrl

  // 与模块级草稿保持同步（再次进入同一表单或预置卡重填时刷新）
  React.useEffect(() => {
    setBaseUrlDraft(readField(BASEURL_ID))
    setCreateApiKey(readField(APIKEY_ID))
    setExistingApiKey(readField(EXISTING_APIKEY_ID))
  }, [showToken])

  React.useEffect(() => {
    if (mode !== 'create' || !presetKey) return
    const preset = shared().wbPresetProviders?.presetOf?.(presetKey)
    if (!preset) return
    const presetNoAuth = preset.account?.noAuth === true
    setDraftValue(NAME_ID, preset.baseUrl ? preset.name : '')
    setDraftValue(BASEURL_ID, preset.baseUrl || '')
    setBaseUrlDraft(preset.baseUrl || '')
    if (preset.protocol) {
      setProtocol(preset.protocol)
      setDraftValue(PROTOCOL_ID, preset.protocol)
    }
    setNoAuth(presetNoAuth)
    writeNoAuthDraft(NOAUTH_ID, presetNoAuth)
    setBaseHintOverride(preset.hint || '')
    setBasePlaceholderOverride(preset.placeholder || '')
  }, [mode, presetKey, providerHint, showToken])

  const existingPresetKey = pickedProvider?.presetKey || ''
  React.useEffect(() => {
    if (mode !== 'existing') return
    const presetNoAuth = presetOfProvider(pickedProvider)?.account?.noAuth === true
    setExistingNoAuth(presetNoAuth)
    writeNoAuthDraft(EXISTING_NOAUTH_ID, presetNoAuth)
  }, [mode, providerHint, existingPresetKey, showToken])

  const anthropic = protocol === 'anthropic'
  const baseHint = baseHintOverride || (anthropic ? BASE_HINT_ANTHROPIC : BASE_HINT_OPENAI)
  const basePlaceholder = basePlaceholderOverride || (anthropic ? BASE_PLACEHOLDER_ANTHROPIC : BASE_PLACEHOLDER_OPENAI)
  const createPreview = previewCustomEndpointUrl(baseUrlDraft, protocol, createPreset?.quirks?.urlSuffix)
  const strippedBase = detectTrailingEndpoint(baseUrlDraft, protocol)
  const existingPreview = previewCustomEndpointUrl(
    pickedProvider?.baseUrl,
    pickedProvider?.protocol,
    pickedProvider?.quirks?.urlSuffix || pickedProvider?.urlSuffix,
  )

  const toggleNoAuth = (id: string, setter: (next: boolean) => void) => (next: boolean) => {
    setter(next)
    writeNoAuthDraft(id, next)
  }

  return (
    <>
      <div hidden={mode !== 'create'}>
        {createPreset ? <PresetSponsorCard preset={createPreset} /> : null}
        <SettingGroup title={t('提供商连接')}>
          <div className='acct-dlg-field-grid'>
            <Field className='acct-dlg-field'>
              <FieldLabel htmlFor={NAME_ID}>
                {selfHosted ? t('站点名称') : t('名称')}
                <i className='req' aria-hidden='true'>*</i>
              </FieldLabel>
              <Input
                id={NAME_ID}
                type='text'
                maxLength={MAX_NAME_CHARS}
                aria-required='true'
                placeholder={selfHosted ? t('如：公司中转站、个人备用站') : t('如：智谱 GLM')}
                {...draftProps(NAME_ID)}
              />
              <FieldDescription>
                {selfHosted
                  ? t('填写具体站点名称；模板类型不会作为站点身份')
                  : t('1~64 个字符，账号列表与模型页按它分组显示')}
              </FieldDescription>
            </Field>
            <Field className='acct-dlg-field'>
              <FieldLabel htmlFor={PROTOCOL_ID}>{t('默认协议')}</FieldLabel>
              <Select
                value={protocol}
                onValueChange={next => {
                  const value = String(next)
                  setProtocol(value)
                  setDraftValue(PROTOCOL_ID, value)
                  setBaseHintOverride('')
                }}
              >
                <SelectTrigger id={PROTOCOL_ID}>
                  <SelectValue>{options.find(item => item.value === protocol)?.label || protocol}</SelectValue>
                </SelectTrigger>
                <SelectContent>
                  {options.map(item => (
                    <SelectItem key={item.value} value={item.value}>{item.label}</SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <FieldDescription>{t('各模型的协议后续可在「模型管理」中单独覆写。')}</FieldDescription>
            </Field>
            <Field className='acct-dlg-field acct-dlg-field-full'>
              <FieldLabel htmlFor={BASEURL_ID}>
                Base URL
                <i className='req' aria-hidden='true'>*</i>
              </FieldLabel>
              <Input
                id={BASEURL_ID}
                type='text'
                className='mono'
                spellCheck={false}
                aria-required='true'
                placeholder={basePlaceholder}
                value={baseUrlDraft}
                onChange={event => {
                  const value = event.currentTarget.value
                  setBaseUrlDraft(value)
                  setDraftValue(BASEURL_ID, value)
                }}
              />
              <FieldDescription id='custom-baseurl-hint'>{baseHint}</FieldDescription>
            </Field>
          </div>
          {strippedBase !== null ? (
            <div className='acct-dlg-inline-note warning'>
              <AccountSettingsIcon name='info' size={14} />
              <span className='min-w-0 flex-1'>
                {t('检测到 Base URL 末尾已包含具体请求路径，下面预览会重复拼接。')}
              </span>
              <Button
                type='button'
                variant='ghost'
                size='xs'
                className='acct-dlg-clear shrink-0'
                onClick={() => {
                  setBaseUrlDraft(strippedBase)
                  setDraftValue(BASEURL_ID, strippedBase)
                }}
              >
                {t('去除末尾路径')}
              </Button>
            </div>
          ) : null}
          <div className='acct-dlg-endpoint'>
            <span>{t('完整请求地址预览')}</span>
            {createPreview
              ? <code>{createPreview}</code>
              : <span className='text-[11.5px] text-subtle'>{t('填写 Base URL 后将在这里实时预览最终请求端点')}</span>}
          </div>
        </SettingGroup>

        <CredentialGroup
          apiKeyId={APIKEY_ID}
          apiKeyValue={createApiKey}
          onApiKeyChange={value => {
            setCreateApiKey(value)
            setDraftValue(APIKEY_ID, value)
            if (value.trim() && noAuth) {
              setNoAuth(false)
              writeNoAuthDraft(NOAUTH_ID, false)
            }
          }}
          noAuth={noAuth}
          onToggleNoAuth={toggleNoAuth(NOAUTH_ID, setNoAuth)}
          noAuthNote={noAuth
            ? t('已声明无需鉴权：账号会被正常选路，出网时不带任何鉴权头。')
            : t('本地 Ollama、OpenCode Zen 免费档等无需密钥的上游可选择「无需鉴权」。')}
        />
      </div>

      <div hidden={mode !== 'existing'}>
        <SettingGroup title={t('所属提供商')}>
          <div className='acct-dlg-readonly'>
            <span>{t('提供商名称')}</span>
            <code>{pickedProvider ? pickedName : t('该提供商已不存在（可能已被删除），请刷新后重试')}</code>
          </div>
          {pickedProvider ? (
            <>
              <div className='acct-dlg-readonly'>
                <span>{t('默认协议')}</span>
                <code>{customProtocolLabel(pickedProvider.protocol)}</code>
              </div>
              <div className='acct-dlg-readonly'>
                <span>Base URL</span>
                <code>{pickedProvider.baseUrl || ''}</code>
              </div>
              {existingPreview ? (
                <div className='acct-dlg-endpoint'>
                  <span>{t('完整请求地址预览')}</span>
                  <code>{existingPreview}</code>
                </div>
              ) : null}
            </>
          ) : null}
        </SettingGroup>

        <CredentialGroup
          apiKeyId={EXISTING_APIKEY_ID}
          apiKeyValue={existingApiKey}
          onApiKeyChange={value => {
            setExistingApiKey(value)
            setDraftValue(EXISTING_APIKEY_ID, value)
            if (value.trim() && existingNoAuth) {
              setExistingNoAuth(false)
              writeNoAuthDraft(EXISTING_NOAUTH_ID, false)
            }
          }}
          noAuth={existingNoAuth}
          onToggleNoAuth={toggleNoAuth(EXISTING_NOAUTH_ID, setExistingNoAuth)}
          noAuthNote={existingNoAuth
            ? t('已声明无需鉴权：同家可混放有 Key 与无鉴权账号，各自可配不同的出网代理。')
            : t('同一提供商下可添加多把 API Key，按优先级轮换；重复的 API Key 会更新既有账号。')}
          extraBottom={(
            <Field className='acct-dlg-field'>
              <FieldLabel htmlFor={EXISTING_NAME_ID}>{t('备注名')}</FieldLabel>
              <Input
                id={EXISTING_NAME_ID}
                type='text'
                maxLength={MAX_ACCOUNT_NAME_CHARS}
                placeholder={t('可选，留空则用提供商名称')}
                {...draftProps(EXISTING_NAME_ID)}
              />
            </Field>
          )}
        />
      </div>
    </>
  )
}

/* ─── 底部操作条上的主按钮 ─────────────────── */

type FootContext = {
  presetKey: string
  providerId: string
  setBusy: (value: boolean) => void
  setHint: (message: string) => void
  setDuplicates: (providers: CustomProviderRecord[]) => void
  readProxy: () => ProxyPayload
  readSettings: (options: {
    supportsUsage: boolean
    uaEditable: boolean
    balanceScheme?: string
  }) => { error: string; pane: AddPaneId } | { payload: Record<string, unknown> }
  setPane: (pane: AddPaneId) => void
  resetSettings: () => void
}

function showSubmitError(context: FootContext, error: unknown): void {
  const message = describeError(error)
  toast(t('添加失败：{reason}', { reason: message }), 'err')
  context.setHint(message)
}

function resolveProxyPayload(context: FootContext): ProxyPayload | null | 'error' {
  try {
    return context.readProxy()
  } catch (error) {
    context.setPane('network')
    showSubmitError(context, error)
    return 'error'
  }
}

/** 新建模式：POST /api/custom-providers（提供商 + 首个账号一次建成） */
async function submitCreate(context: FootContext, allowDuplicate = false, target?: CustomProviderRecord): Promise<void> {
  const name = readField(NAME_ID)
  const protocol = readField(PROTOCOL_ID) || protocolOptions()[0].value
  const baseUrl = readField(BASEURL_ID)
  const apiKey = readField(APIKEY_ID)
  const noAuth = readNoAuthDraft(NOAUTH_ID)
  if (!name) {
    context.setPane('basic')
    toast(t('请填写名称'), 'err')
    return
  }
  if (!baseUrl) {
    context.setPane('basic')
    toast(t('请填写 Base URL'), 'err')
    return
  }

  const preset = context.presetKey
    ? shared().wbPresetProviders?.presetOf?.(context.presetKey)
    : null
  const createScheme = presetSchemeOf(context.presetKey)
  const effectiveScheme = target ? (target.balanceScheme || '') : createScheme
  const uaEditable = customUserAgentEditable(target || preset)
  const settingsResult = context.readSettings({
    supportsUsage: Boolean(effectiveScheme),
    uaEditable,
    balanceScheme: effectiveScheme,
  })
  if ('error' in settingsResult) {
    context.setPane(settingsResult.pane)
    toast(settingsResult.error, 'err')
    context.setHint(settingsResult.error)
    return
  }
  const proxyPayload = resolveProxyPayload(context)
  if (proxyPayload === 'error') return

  await runSubmit(context.setBusy, async () => {
    if (!allowDuplicate) {
      await shared().wbProviders?.refreshCustom?.()
      const duplicates = customList().filter(provider => sameProviderEndpoint(provider, protocol, baseUrl))
      if (duplicates.length) {
        context.setDuplicates(duplicates)
        return
      }
    }
    context.setDuplicates([])
    if (target && !customList().some(provider => provider.id === target.id && sameProviderEndpoint(provider, protocol, baseUrl))) {
      toast(t('该提供商已不存在或地址已变更，请重新选择'), 'err')
      return
    }
    const targetWasFirstAndEmpty = target
      ? accountCountOf(target.id) === 0 && !(target.models?.length)
      : false

    const payload: Record<string, unknown> = target ? { provider: target.id } : { name, protocol, baseUrl }
    if (!target) {
      if (preset) payload.presetKey = preset.key
      const quirks = preset?.quirks || {}
      if (quirks.urlSuffix) payload.urlSuffix = quirks.urlSuffix
      if (quirks.headers && Object.keys(quirks.headers).length) payload.headers = { ...quirks.headers }
      if (quirks.anthropicToolType) payload.anthropicToolType = quirks.anthropicToolType
      if (preset?.clientEmulation) payload.clientEmulation = preset.clientEmulation
      if (createScheme) payload.balanceScheme = createScheme
    }
    // 合并右侧分栏配置（启用态 / 失败换号 / 并发 / UA / 自动查询与查询凭证 / 限制器 / 代理）
    Object.assign(payload, settingsResult.payload)
    if (target && target.balanceScheme !== createScheme) {
      delete payload.balanceQuery
    }
    if (noAuth) payload.noAuth = true
    else if (apiKey) payload.apiKey = apiKey
    payload.proxy = proxyPayload

    context.setHint('')
    try {
      const data = (await shared().wbProviders?.customRequest?.(
        'POST', target ? '/api/accounts' : '/api/custom-providers', payload,
      )) as { provider?: { id?: string; name?: string }; account?: { name?: string }; created?: boolean } | null
      clearFields([NAME_ID, BASEURL_ID, APIKEY_ID])
      writeNoAuthDraft(NOAUTH_ID, false)
      context.resetSettings()
      const message = target
        ? addedMessage(data?.account?.name || target.name || '', data?.created)
        : t('✅ 已创建自定义提供商「{name}」并添加账号', { name: data?.provider?.name || name })
      const fetchTarget = !target && data?.provider?.id
        ? { providerId: data.provider.id, name: data.provider.name || name }
        : (target && targetWasFirstAndEmpty
          ? { providerId: target.id, name: target.name || name }
          : undefined)
      await afterCustomAdd(message, fetchTarget)
    } catch (error) {
      showSubmitError(context, error)
    }
  })
}

function addedMessage(label: string, created: boolean | undefined): string {
  const suffix = label ? `：${label}` : ''
  return created !== false
    ? t('✅ {label}账号已添加{name}', { label: '', name: suffix })
    : t('✅ 已更新已有账号{name}（同一提供商内，同一把 API Key 只保留一条）', { name: suffix })
}

/** 已有模式：POST /api/accounts（custom 账号走 provider = custom-xxx 分支） */
async function submitExisting(context: FootContext): Promise<void> {
  const providerId = context.providerId
  const selectedProvider = customList().find(item => item.id === providerId)
  if (!providerId || !selectedProvider) {
    toast(t('该提供商已不存在（可能已被删除），请刷新后重试'), 'err')
    return
  }
  const apiKey = readField(EXISTING_APIKEY_ID)
  const noAuth = readNoAuthDraft(EXISTING_NOAUTH_ID)
  const name = readField(EXISTING_NAME_ID)
  const scheme = String(selectedProvider.balanceScheme || '')
  const uaEditable = customUserAgentEditable(selectedProvider)

  const settingsResult = context.readSettings({
    supportsUsage: Boolean(scheme),
    uaEditable,
    balanceScheme: scheme,
  })
  if ('error' in settingsResult) {
    context.setPane(settingsResult.pane)
    toast(settingsResult.error, 'err')
    context.setHint(settingsResult.error)
    return
  }
  const proxyPayload = resolveProxyPayload(context)
  if (proxyPayload === 'error') return

  const wasFirstAndEmpty = accountCountOf(providerId) === 0 && !(selectedProvider.models?.length)

  await runSubmit(context.setBusy, async () => {
    const payload: Record<string, unknown> = { provider: providerId }
    if (noAuth) payload.noAuth = true
    else if (apiKey) payload.apiKey = apiKey
    if (name) payload.name = name
    Object.assign(payload, settingsResult.payload)
    payload.proxy = proxyPayload

    context.setHint('')
    try {
      const data = (await shared().wbProviders?.customRequest?.(
        'POST', '/api/accounts', payload,
      )) as { account?: { name?: string }; created?: boolean } | null
      clearFields([EXISTING_APIKEY_ID, EXISTING_NAME_ID])
      writeNoAuthDraft(EXISTING_NOAUTH_ID, false)
      context.resetSettings()
      const providerName = selectedProvider.name || ''
      const label = data?.account?.name || providerName
      const fetchTarget = wasFirstAndEmpty
        ? { providerId, name: providerName || providerId }
        : undefined
      await afterCustomAdd(addedMessage(label, data?.created), fetchTarget)
    } catch (error) {
      showSubmitError(context, error)
    }
  })
}

async function removeExistingProvider(providerId: string): Promise<void> {
  if (!providerId) { toast(t('请先选择一个自定义提供商'), 'err'); return }
  const remover = shared().wbCustomProvidersUi?.remove
  if (typeof remover !== 'function') { toast(t('删除功能不可用（脚本未就绪）'), 'err'); return }
  await runSubmit(() => {}, async () => {
    const removed = await remover(providerId)
    if (removed) shared().wbAddAccountModal?.close?.()
  })
}

export function CustomFootActions({
  mode,
  presetKey,
  providerId,
}: {
  mode: CustomMode
  presetKey: string
  providerId: string
}): React.ReactElement {
  const [busy, setBusy] = useAccountAddBusy()
  const proxy = useAddAccountProxy()
  const settings = useAddAccountSettings()
  const [hint, setHint] = React.useState('')
  const [duplicates, setDuplicates] = React.useState<CustomProviderRecord[]>([])
  const context: FootContext = {
    presetKey,
    providerId,
    setBusy,
    setHint,
    setDuplicates,
    readProxy: proxy.read,
    readSettings: settings.readSettingsPayload,
    setPane: settings.setPane,
    resetSettings: settings.resetAll,
  }

  return (
    <>
      {duplicates.length ? (
        <DuplicateProviderDialog
          providers={duplicates}
          onClose={() => setDuplicates([])}
          onCreate={() => { void submitCreate(context, true) }}
          onExisting={provider => { void submitCreate(context, true, provider) }}
        />
      ) : null}
      <span className={`add-foot-hint${hint ? ' err' : ''}`} id='add-form-foot-hint'>{hint}</span>
      <span className='add-foot-actions' id='add-form-foot-actions'>
        {mode === 'existing' ? (
          <Button
            id={REMOVE_BUTTON_ID}
            variant='destructive'
            title={t('级联删除名下全部账号，不可恢复')}
            onClick={() => { void removeExistingProvider(providerId) }}
          >
            {t('删除此提供商')}
          </Button>
        ) : null}
        <Button
          id={CREATE_BUTTON_ID}
          hidden={mode !== 'create'}
          disabled={busy}
          onClick={() => { void submitCreate(context) }}
        >
          {busy && mode === 'create' ? t('提交中…') : t('创建并添加账号')}
        </Button>
        <Button
          id={EXISTING_BUTTON_ID}
          hidden={mode !== 'existing'}
          disabled={busy}
          onClick={() => { void submitExisting(context) }}
        >
          {busy && mode === 'existing' ? t('提交中…') : t('添加账号')}
        </Button>
      </span>
    </>
  )
}
