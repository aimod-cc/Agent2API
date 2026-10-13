/**
 * Agent2API · 「添加账号」弹窗：自定义提供商（新建 / 加入已有）。
 *
 * 替换旧 ui/add-custom-provider.js。自定义提供商（custom- 前缀，运行期数据）的
 * 添加方式有两种：新建提供商 + 首个账号（POST /api/custom-providers）、往已有
 * 提供商再加账号（POST /api/accounts）—— 字段与端点都成对出现，与内置家那份
 * 「一份字段配置 + 统一提交」的模型对不上，因此单独一块。
 *
 * ── 两种添加方式怎么选 ────────────────────────────────────
 * **不由块内问**。第 1 步点的是「新建自定义提供商」那张卡还是一家已有提供商的
 * 卡片，本身就已经回答了「给谁加账号」：前者上下文里没有 id → 新建，后者带着
 * 那家的 id → 加入已有并预选它。要改主意关掉表单弹窗回列表重选（第 1 步还在
 * 下面开着）。于是这里只剩「按上下文落到哪种模式」，没有模式切换控件。
 *
 * ── 主按钮为什么在底部操作条 ──────────────────────────────
 * 留在字段流里它会和输入框同一个节奏、主次不分，底部条同时给失败提示一个固定
 * 位置（toast 几秒后就没了）。因此拆成两个组件：表单段（这里）与底部操作条
 * （CustomFootActions），两者只通过 DOM id 与 mode 沟通 —— 提交读的就是
 * 输入框当前值，与旧实现一致。
 */

import * as React from 'react'
import {
  Button,
  Checkbox,
  DialogSection,
  Input,
  Label,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@ui'

import {
  clearFields,
  describeError,
  draftProps,
  readField,
  setDraftValue,
  shared,
  toast,
  type CustomProviderRecord,
} from './add-account-bridge'
import { t } from '../i18n'

/** 展示名长度上限（与后端 custom_providers::MAX_NAME_CHARS 一致，前端先挡一次） */
const MAX_NAME_CHARS = 64
/** 账号备注名长度上限（与 add-provider-forms 的 MAX_NAME_LENGTH 同一口径） */
const MAX_ACCOUNT_NAME_CHARS = 100

/** Base URL 的两种填法：Anthropic 走根地址，OpenAI 兼容要带 /v1（后端按协议拼路径） */
const BASE_HINT_OPENAI = t('OpenAI 兼容填到 /v1；Anthropic 填根地址')
const BASE_HINT_ANTHROPIC = t('Anthropic 协议填根地址，不要带 /v1')
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
const EXISTING_SELECT_ID = 'custom-existing-select'
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

/** 添加成功后的统一收尾：关弹窗、刷新自定义目录与账号列表、提示 */
async function afterCustomAdd(message: string): Promise<void> {
  // 两层一起关（表单弹窗 + 下面的列表弹窗）：只摘外层会让表单留在屏幕上
  shared().wbAddAccountModal?.close?.()
  // 目录先刷：账号行 / 筛选器显示的提供商名都来自 wbProviders 的缓存
  void shared().wbProviders?.refreshCustom?.()
  await shared().wbApp?.refresh?.()
  toast(message)
}

/* ─── 「该上游无需鉴权」勾选框 ─────────────── */

/**
 * 勾选框的状态由 React 持有（组件库的 Checkbox 不是原生 input，`readField`
 * 那套 DOM 读法对它无效），同时写一份到表单草稿 —— 提交动作在**另一个组件**
 * （CustomFootActions）里，它与表单段之间只有「DOM id + 草稿」这一条通道
 * （与协议下拉同一手法，见文件头）。草稿里存 '1' / 空串，读侧 `readField`
 * 对非 input 的 id 会回落到草稿，于是两边不必共享 React 状态。
 */
function writeNoAuthDraft(id: string, value: boolean): void {
  setDraftValue(id, value ? '1' : '')
}

function readNoAuthDraft(id: string): boolean {
  return readField(id) === '1'
}

/** 勾选框 + 说明（两处表单共用同一行结构，差异只在说明文案） */
function NoAuthCheckbox({
  checked, note, onChange,
}: {
  checked: boolean
  note: string
  onChange: (next: boolean) => void
}): React.ReactElement {
  return (
    <div className='add-field'>
      <Label>{t('鉴权')}</Label>
      {/* 组件库的 Checkbox 不是原生 input（自绘的 role=checkbox 按钮），
          用包一层 <label> 建立关联：button 是可标注元素，点文字即可切换 */}
      <div className='col-start-2 row-start-1 flex h-[30px] items-center'>
        <label className='inline-flex cursor-pointer items-center gap-2.5'>
          <Checkbox checked={checked} aria-label={t('该上游无需鉴权')}
            onCheckedChange={next => onChange(next === true)} />
          <span className='text-xs text-subtle'>{t('该上游无需鉴权（不发送鉴权头）')}</span>
        </label>
      </div>
      <span className='hint'>{note}</span>
    </div>
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
  /** 第 1 步每点一次卡就 +1：让预填在「再次点同一张预置卡」时也重新执行（旧 onShow 每次都跑） */
  showToken: number
  /** 目录刷新后的重画信号 */
  version: number
}): React.ReactElement {
  const options = protocolOptions()
  const [protocol, setProtocol] = React.useState(() => readField(PROTOCOL_ID) || options[0].value)
  /** 预置家给的 Base URL 备注（用户一改协议就失效，回到按协议算的那句） */
  const [baseHintOverride, setBaseHintOverride] = React.useState('')
  const [picked, setPicked] = React.useState(() => readField(EXISTING_SELECT_ID))
  /** 两张表单各有一枚「该上游无需鉴权」，初值都从草稿读（关掉再打开还在） */
  const [noAuth, setNoAuth] = React.useState(() => readNoAuthDraft(NOAUTH_ID))
  const [existingNoAuth, setExistingNoAuth] = React.useState(() => readNoAuthDraft(EXISTING_NOAUTH_ID))

  const list = React.useMemo(() => customList(), [version])
  // 待选中的那家（带着上下文进来）优先；它不在列表里（目录还没到 / 已被删除）
  // 时退回已选中的、再退回第一项
  const wanted = providerHint && list.some(item => item.id === providerHint) ? providerHint : ''
  const current = wanted || (list.some(item => item.id === picked) ? picked : (list[0]?.id || ''))
  const pickedName = list.find(item => item.id === current)?.name || t('该提供商')

  // 提交动作在底部操作条那个组件里，它按 id 现读「当前选中的是哪一家」——
  // 下拉是组件库的按钮触发器（不是原生 select），值只能落到草稿里给它读
  React.useEffect(() => {
    setDraftValue(EXISTING_SELECT_ID, current)
  }, [current])

  /**
   * 切到这一家时按上下文落到哪种模式：预置家卡片把名称 / 协议 / Base URL 预填进
   * 新建表单（都可改），而 quirks（上游特判）不进表单 —— 提交时原样随记录写入。
   * 与旧实现的 onShow 同一时序：先按协议刷提示，再填预置值。
   *
   * 顺带预勾「该上游无需鉴权」：预置清单里声明了 `account.noAuth` 的家
   * （OpenCode Zen 的免费档、本地 Ollama）本来就不要 Key —— 它正是 issue #39
   * 里「加了提供商却处处提示要 API Key」的根因，勾上这一项才是一条可用账号。
   * 同一张卡再点一次会重新预填，与其它字段同一时序。
   */
  React.useEffect(() => {
    if (mode !== 'create' || !presetKey) return
    const preset = shared().wbPresetProviders?.presetOf?.(presetKey)
    if (!preset) return
    setDraftValue(NAME_ID, preset.name || '')
    setDraftValue(BASEURL_ID, preset.baseUrl || '')
    if (preset.protocol) {
      setProtocol(preset.protocol)
      setDraftValue(PROTOCOL_ID, preset.protocol)
    }
    const presetNoAuth = preset.account?.noAuth === true
    setNoAuth(presetNoAuth)
    writeNoAuthDraft(NOAUTH_ID, presetNoAuth)
    setBaseHintOverride(preset.hint || '')
    // showToken 参与依赖：同一张预置卡再点一次也要重新预填（旧 onShow 每次进这一屏都跑）
  }, [mode, presetKey, providerHint, showToken])

  const anthropic = protocol === 'anthropic'
  const baseHint = baseHintOverride || (anthropic ? BASE_HINT_ANTHROPIC : BASE_HINT_OPENAI)
  /** 勾选框的统一处理：React 状态 + 草稿一起写（提交动作读草稿） */
  const toggleNoAuth = (id: string, setter: (next: boolean) => void) => (next: boolean) => {
    setter(next)
    writeNoAuthDraft(id, next)
  }

  return (
    <>
      <DialogSection hidden={mode !== 'create'}>
        <div className='add-panel-head'>
          <h3>{t('上游信息')}</h3>
          <span>{t('创建这个提供商，并同时建立它的第一个账号。')}</span>
        </div>
        <div className='add-form'>
          <div className='add-field'>
            <Label htmlFor={NAME_ID}>
              {t('名称')}
              <i className='req' aria-hidden='true'>*</i>
            </Label>
            <Input
              id={NAME_ID}
              type='text'
              maxLength={MAX_NAME_CHARS}
              aria-required='true'
              placeholder={t('如：智谱 GLM')}
              {...draftProps(NAME_ID)}
            />
            <span className='hint'>{t('1~64 个字符，账号列表里按它分组显示')}</span>
          </div>
          <div className='add-field'>
            <Label htmlFor={PROTOCOL_ID}>{t('协议')}</Label>
            {/* 协议换了下方的 Base URL 提示跟着换，因此这里受控 */}
            <Select
              value={protocol}
              onValueChange={next => {
                const value = String(next)
                setProtocol(value)
                setDraftValue(PROTOCOL_ID, value)
                setBaseHintOverride('')
              }}
            >
              {/* .add-field 的网格规则只认原生 select（`> select`），触发器要自己带位置 */}
              <SelectTrigger id={PROTOCOL_ID} className='col-start-2 row-start-1 w-full'>
                <SelectValue>{options.find(item => item.value === protocol)?.label || protocol}</SelectValue>
              </SelectTrigger>
              <SelectContent>
                {options.map(item => (
                  <SelectItem key={item.value} value={item.value}>{item.label}</SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div className='add-field'>
            <Label htmlFor={BASEURL_ID}>
              Base URL
              <i className='req' aria-hidden='true'>*</i>
            </Label>
            <Input
              id={BASEURL_ID}
              type='text'
              aria-required='true'
              placeholder={anthropic ? BASE_PLACEHOLDER_ANTHROPIC : BASE_PLACEHOLDER_OPENAI}
              {...draftProps(BASEURL_ID)}
            />
            <span className='hint' id='custom-baseurl-hint'>{baseHint}</span>
          </div>
          <div className='add-field'>
            <Label htmlFor={APIKEY_ID}>API Key</Label>
            <Input
              id={APIKEY_ID}
              type='password'
              autoComplete='new-password'
              placeholder='sk-…'
              // 勾了「无需鉴权」就没有 key 可填（后端也按互斥归一：两个字段
              // 只能有一个成立，见 custom_accounts::add_custom_account）
              disabled={noAuth}
              {...draftProps(APIKEY_ID)}
            />
            <span className='hint'>
              {noAuth
                ? t('不需要 Key：转发与拉取模型都不发送鉴权头')
                : t('上游不要鉴权时留空，并勾选下面一项（留空又不勾会被当成未配置凭证）')}
            </span>
          </div>
          <NoAuthCheckbox
            checked={noAuth}
            note={noAuth
              ? t('已声明无需鉴权：账号会被正常选路，出网时不带任何鉴权头')
              : t('本地 Ollama、OpenCode Zen 免费档这类上游要勾上，否则账号不可用')}
            onChange={toggleNoAuth(NOAUTH_ID, setNoAuth)}
          />
        </div>
      </DialogSection>

      <DialogSection hidden={mode !== 'existing'}>
        <div className='add-panel-head'>
          <h3>{t('账号信息')}</h3>
          {/* 提供商名是用户数据，留在 JSX 里交给 React 转义（不进 dangerouslySetInnerHTML）；
              两边的中文碎片各自走 t()，全角空格是原文的一部分 */}
          <span>
            {t('添加到 ')}<b>{pickedName}</b>{t('　同一家可以放多把 key，按优先级轮换。')}
          </span>
        </div>
        <div className='add-form'>
          <div className='add-field'>
            <Label htmlFor={EXISTING_SELECT_ID}>{t('提供商')}</Label>
            <Select
              value={current}
              onValueChange={next => setPicked(String(next))}
            >
              <SelectTrigger id={EXISTING_SELECT_ID} className='col-start-2 row-start-1 w-full'>
                <SelectValue>{pickedName}</SelectValue>
              </SelectTrigger>
              <SelectContent>
                {list.map(item => (
                  <SelectItem key={item.id} value={item.id}>{item.name || item.id}</SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div className='add-field'>
            <Label htmlFor={EXISTING_APIKEY_ID}>API Key</Label>
            <Input
              id={EXISTING_APIKEY_ID}
              type='password'
              autoComplete='new-password'
              placeholder='sk-…'
              disabled={existingNoAuth}
              {...draftProps(EXISTING_APIKEY_ID)}
            />
            <span className='hint'>
              {existingNoAuth
                ? t('不需要 Key：转发与拉取模型都不发送鉴权头')
                : t('上游不要鉴权时留空，并勾选下面一项（留空又不勾会被当成未配置凭证）')}
            </span>
          </div>
          <NoAuthCheckbox
            checked={existingNoAuth}
            note={existingNoAuth
              ? t('已声明无需鉴权：账号会被正常选路，出网时不带任何鉴权头')
              : t('同一家可以混着放：有 Key 的账号与无鉴权账号各按各的规则走')}
            onChange={toggleNoAuth(EXISTING_NOAUTH_ID, setExistingNoAuth)}
          />
          <div className='add-field'>
            <Label htmlFor={EXISTING_NAME_ID}>{t('备注名')}</Label>
            <Input
              id={EXISTING_NAME_ID}
              type='text'
              maxLength={MAX_ACCOUNT_NAME_CHARS}
              placeholder={t('可选')}
              {...draftProps(EXISTING_NAME_ID)}
            />
            <span className='hint'>{t('留空则用提供商名称')}</span>
          </div>
        </div>
      </DialogSection>
    </>
  )
}

/* ─── 底部操作条上的主按钮 ─────────────────── */

/** 提交这一屏要用到的两件事：忙态与失败提示（都由底部操作条那个组件持有） */
type FootContext = {
  presetKey: string
  setBusy: (value: boolean) => void
  setHint: (message: string) => void
}

/** 失败提示同时写 toast 与底部条（toast 几秒后就没了） */
function showSubmitError(context: FootContext, error: unknown): void {
  const message = describeError(error)
  toast(t('添加失败：{reason}', { reason: message }), 'err')
  context.setHint(message)
}

/** 新建模式：POST /api/custom-providers（提供商 + 首个账号一次建成） */
async function submitCreate(context: FootContext): Promise<void> {
  const name = readField(NAME_ID)
  const protocol = readField(PROTOCOL_ID) || protocolOptions()[0].value
  const baseUrl = readField(BASEURL_ID)
  const apiKey = readField(APIKEY_ID)
  const noAuth = readNoAuthDraft(NOAUTH_ID)
  // 必填拦截在本地先做一次（弹窗不是 <form>，原生 required 不生效）
  if (!name) { toast(t('请填写名称'), 'err'); return }
  if (!baseUrl) { toast(t('请填写 Base URL'), 'err'); return }
  await runSubmit(context.setBusy, async () => {
    const payload: Record<string, unknown> = { name, protocol, baseUrl }
    // 预置家的上游特判随记录写入（转发层按这些字段修正请求，见 preset-providers.js 的 quirks）
    const preset = context.presetKey
      ? shared().wbPresetProviders?.presetOf?.(context.presetKey)
      : null
    const quirks = preset?.quirks || {}
    if (quirks.urlSuffix) payload.urlSuffix = quirks.urlSuffix
    if (quirks.headers && Object.keys(quirks.headers).length) payload.headers = { ...quirks.headers }
    if (quirks.anthropicToolType) payload.anthropicToolType = quirks.anthropicToolType
    // 客户端形态伪装（OpenCode 免费档）：只有预置清单声明了的家才有，
    // 建完可在账号设置的「提供商」一段里改
    if (preset?.clientEmulation) payload.clientEmulation = preset.clientEmulation
    // 两个凭证字段互斥（后端也这么归一）：勾了无需鉴权就不带 apiKey 上去
    if (noAuth) payload.noAuth = true
    else if (apiKey) payload.apiKey = apiKey
    context.setHint('')
    try {
      const data = (await shared().wbProviders?.customRequest?.(
        'POST', '/api/custom-providers', payload,
      )) as { provider?: { name?: string } } | null
      clearFields([NAME_ID, BASEURL_ID, APIKEY_ID])
      // 勾选框与草稿一起复位（下次进来是干净的默认态，不被上一家预勾影响）
      writeNoAuthDraft(NOAUTH_ID, false)
      const created = data?.provider?.name || name
      await afterCustomAdd(t('✅ 已创建自定义提供商「{name}」并添加账号', { name: created }))
    } catch (error) {
      showSubmitError(context, error)
    }
  })
}

/** 已有模式：POST /api/accounts（custom 账号走 provider = custom-xxx 分支） */
async function submitExisting(context: FootContext): Promise<void> {
  const providerId = readField(EXISTING_SELECT_ID)
  if (!providerId) { toast(t('请先选择一个自定义提供商'), 'err'); return }
  const apiKey = readField(EXISTING_APIKEY_ID)
  const noAuth = readNoAuthDraft(EXISTING_NOAUTH_ID)
  const name = readField(EXISTING_NAME_ID)
  await runSubmit(context.setBusy, async () => {
    const payload: Record<string, unknown> = { provider: providerId }
    // 与新建模式同一口径：勾了无需鉴权就不带 apiKey
    if (noAuth) payload.noAuth = true
    else if (apiKey) payload.apiKey = apiKey
    if (name) payload.name = name
    context.setHint('')
    try {
      const data = (await shared().wbProviders?.customRequest?.(
        'POST', '/api/accounts', payload,
      )) as { account?: { name?: string } } | null
      clearFields([EXISTING_APIKEY_ID, EXISTING_NAME_ID])
      writeNoAuthDraft(EXISTING_NOAUTH_ID, false)
      const label = data?.account?.name
        || customList().find(item => item.id === providerId)?.name
        || ''
      // 与 add-account-bridge 的 afterAdd 同键同形（label 留空，账号名的冒号前缀作为形参值拼入）
      await afterCustomAdd(t('✅ {label}账号已添加{name}', { label: '', name: label ? `：${label}` : '' }))
    } catch (error) {
      showSubmitError(context, error)
    }
  })
}

/**
 * 删除「加入已有」模式下选中的那一家（级联删账号）。
 *
 * 动作本身全在 `wbCustomProvidersUi.remove` 里 —— 二次确认（说明将级联删掉多少
 * 账号）、POST /api/custom-providers/remove、刷新目录与账号列表都在那边，与账号
 * 设置弹窗里的「删除提供商」共用同一条链；这里只回答两件事：删的是哪一家
 * （下拉当前值）、删完收什么尾（关弹窗 —— 名下账号连同删光，弹窗里没有可停留
 * 的上下文了）。
 */
async function removeExistingProvider(): Promise<void> {
  const providerId = readField(EXISTING_SELECT_ID)
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
}: {
  mode: CustomMode
  presetKey: string
}): React.ReactElement {
  const [busy, setBusy] = React.useState(false)
  const [hint, setHint] = React.useState('')
  const context: FootContext = { presetKey, setBusy, setHint }

  return (
    <>
      <span className={`add-foot-hint${hint ? ' err' : ''}`} id='add-form-foot-hint'>{hint}</span>
      <span className='add-foot-actions' id='add-form-foot-actions'>
        {mode === 'existing' ? (
          <Button
            id={REMOVE_BUTTON_ID}
            variant='destructive'
            title={t('级联删除名下全部账号，不可恢复')}
            onClick={() => { void removeExistingProvider() }}
          >
            {t('删除此提供商')}
          </Button>
        ) : null}
        {/* 两颗主按钮都常驻、按模式切显隐（与旧实现一致：文案与提交函数成对写在一处） */}
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
