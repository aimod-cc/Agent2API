/**
 * Agent2API · 「登录 / 添加账号」弹窗：内置家的表单块（视图 + 三个登录引擎的接线）。
 *
 * 替换旧实现里 add-provider-forms.js 的 buildProviderBlock / mountWebLogin /
 * mountSmsLogin / mountOauthLogin / addProviderManual / addProviderDesktop 与
 * add-account.js 里 WorkBuddy 的那一段（账号版本 + 打开方式 + 第三方入口开关）。
 *
 * ── 与旧实现的一处结构差异（刻意的）──────────────────────────
 * 旧实现把「添加方式」做成互斥显隐的分段，但**所有段落始终在 DOM 里**（只切
 * 显隐），因为三个登录引擎都把状态放在自己的闭包里、事件也按 id 绑在那些
 * 节点上（sms-login.js 在 create() 时 addEventListener）。岛上沿用同一手法：
 * 每家的块整块常驻（只有当前选中那家的块可见），块内各段落也只切显隐。
 * 于是「发码 → 切去别的方式 → 切回来 → 提交」时 deviceId 仍在，行为与旧实现一致。
 *
 * 显隐统一使用 `hidden` 属性，切换方式时不卸载表单和登录引擎。
 *
 * ── 引擎的容器契约 ─────────────────────────────────────────
 *   web-login.js：发布按钮文案、禁用态和等待态，表单用 Button / Spinner 渲染。
 *   sms-login.js：create() 时按 id 绑 click，之后现读非受控输入框的 .value。
 *   autoclaw-oauth.js：create() 时抓两个变体按钮文案并绑 click，按 id 读写。
 * 各节点 id 保持稳定，认证请求和取消流程仍由对应引擎维护。
 */

import * as React from 'react'
import {
  Button,
  Input,
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
  SegmentedControl,
  Spinner,
  Textarea,
  Label,
} from '@ui'
import { AddFoot, AddGroup } from './add-account-kit'

import {
  ADD_METHODS,
  MAX_NAME_LENGTH,
  WEB_LOGIN_HINT,
  addButtonTextOf,
  desktopImportAvailable,
  fieldIdOf,
  manualNoteOf,
  manualTitleOf,
  maxLengthOf,
  methodsOf,
  platform,
  type FieldSpec,
  type MethodId,
  type ProviderConfig,
} from './add-account-configs'
import {
  afterAdd,
  addedLabelOf,
  clearFields,
  closeAddModals,
  describeError,
  draftProps,
  postAccount,
  readField,
  shared,
  startLogin,
  toast,
  type AccountRecord,
  type OauthController,
  type WebLoginController,
  type WebLoginView,
} from './add-account-bridge'
import { useAddAccountProxy, useAccountAddBusy } from './add-account-proxy'
import { Note, runAdd } from './add-provider-common'
import { FileImportSection } from './add-provider-file'
import { UserAgentInput, useUserAgentPresets, userAgentEditable, applyUserAgentAfterCreate, createdAccountIdOf } from './add-account-user-agent'
import { DIALOG_SEG_CLASS } from './dialog-segmented'
import { t } from '../i18n'

/** 小浣熊手填段里那个「登录态文件在哪」的链接点了要显示的路径 */
const RACCOON_AUTH_PATH = t(
  '登录态文件路径：~/.box-agent/config/auth.json（Windows：C:\\Users\\<你的用户名>\\.box-agent\\config\\auth.json）')

/* ─── 手填字段 ─────────────────────────────── */

/**
 * `jsonExpand` 字段：解析并把键并进取到的请求体（`false` = 就地拦下，别把
 * 「你粘的不是 JSON」变成后端签名/凭据模块吐出来的一条远端错误）。
 *
 * 只挡两层：解析失败与非对象（`"abc"` / `5` / `[]`）。粘错的常见形态是「多选了
 * 外面的花括号」或「少粘一层」，这两类都在这一层被挡住。
 */
function expandJsonField(field: FieldSpec, value: string, payload: Record<string, unknown>): boolean {
  let parsed: unknown
  try {
    parsed = JSON.parse(value)
  } catch (error) {
    toast(t('{name} 不是合法 JSON：{reason}', { name: field.label, reason: describeError(error) }), 'err')
    return false
  }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    // JSON 示例里含花括号，不能直接进键（会被 t() 的 {形参} 替换吃掉），作为形参值传入
    toast(t('{name}要粘一个 JSON 对象（形如 {sample}）', {
      name: field.label,
      sample: '{"codearts_provider_credential":{…}}',
    }), 'err')
    return false
  }
  Object.assign(payload, parsed as Record<string, unknown>)
  return true
}

function ManualSection({
  config,
  region,
  visible,
  footerHost,
  userAgent,
  onUserAgent,
}: {
  config: ProviderConfig
  region: string
  visible: boolean
  footerHost: HTMLElement | null
  /** 账号级自定义 User-Agent（草稿在 ProviderBlock 一级：网页登录那条路也要用） */
  userAgent: string
  onUserAgent: (value: string) => void
}): React.ReactElement {
  const prefix = config.provider
  const [busy, setBusy] = useAccountAddBusy()
  const [hint, setHint] = React.useState('')
  const uaPresets = useUserAgentPresets()
  const proxy = useAddAccountProxy()
  const note = manualNoteOf(config)
  const fieldIds = config.fields.map(field => fieldIdOf(config, field))
  const userAgentId = `${prefix}-user-agent`
  // 会话式家（CatPaw / Qoder / Trae 等）的每一步请求都由插件自己拼头，网关没有
  // 覆盖点 —— 不显示输入框，而不是让用户填了发现没生效。
  const uaEditable = userAgentEditable(config.provider)

  async function submit(): Promise<void> {
    const payload: Record<string, unknown> = { provider: config.provider }
    // 地区（Qoder）：整块共用一个分段控件
    if (config.regionOptions?.length) payload.mode = region
    if (userAgent.trim()) payload.userAgent = userAgent.trim()
    for (const field of config.fields) {
      const value = readField(fieldIdOf(config, field))
      if (!field.optional && !value) {
        toast(t('请填写 {label}', { label: field.label }), 'err')
        return
      }
      if (!value) continue
      if (field.jsonExpand) {
        if (!expandJsonField(field, value, payload)) return
        continue
      }
      payload[field.key] = value
    }
    try {
      payload.proxy = proxy.read()
    } catch (error) {
      toast(t('添加失败：{reason}', { reason: describeError(error) }), 'err')
      return
    }
    await runAdd(setBusy, async () => {
      try {
        const data = (await postAccount(payload)) as { account?: AccountRecord } | null
        clearFields(fieldIds)
        onUserAgent('')
        setHint('')
        await afterAdd(addedLabelOf(data?.account), config.label)
      } catch (error) {
        toast(t('添加失败：{reason}', { reason: describeError(error) }), 'err')
      }
    })
  }

  return (
    <AddGroup
      label={manualTitleOf(config)}
      hidden={!visible}
      // 小浣熊那条「登录态文件」链接：旧实现是 document 上的委托，这里收在段落上
      onClick={event => {
        if ((event.target as HTMLElement).closest('[data-raccoon-hint]')) {
          event.preventDefault()
          setHint(RACCOON_AUTH_PATH)
        }
      }}
    >
      <Note {...note} />
      {config.fields.map(field => {
        const id = fieldIdOf(config, field)
        // 必填只在标签上标出（弹窗不是 <form>，原生 required 不生效），真正的拦截在 submit
        const marker = field.optional ? '' : t('（必填）')
        return (
          <div key={id} className={`field-row${field.rows ? ' stack' : ''}`}>
            <Label htmlFor={id}>
              {field.label}
              {marker}
            </Label>
            {field.rows ? (
              <Textarea id={id} rows={field.rows} placeholder={field.placeholder} {...draftProps(id)} />
            ) : (
              <Input
                id={id}
                type='text'
                maxLength={maxLengthOf(field)}
                placeholder={field.placeholder}
                {...draftProps(id)}
              />
            )}
          </div>
        )
      })}
      {uaEditable ? <div className='field-row'>
        <Label htmlFor={userAgentId}>{t('User-Agent（可选）')}</Label>
        <UserAgentInput id={userAgentId} value={userAgent} onChange={onUserAgent}
          presets={uaPresets} disabled={busy} />
      </div> : null}
      {/* 忙态文案在按钮上；这一格只放链接给出的路径与提交失败以外的说明 */}
      <AddFoot host={footerHost} active={visible}
        note={<span className='detail' id={`${prefix}-add-hint`}>{hint}</span>}>
        <Button id={`${prefix}-add-button`} onClick={() => { void submit() }} disabled={busy}>
          {busy ? t('添加中…') : addButtonTextOf(config)}
        </Button>
      </AddFoot>
    </AddGroup>
  )
}

/* ─── 从本机导入桌面端登录态 ─────────────────── */

function DesktopSection({
  config,
  visible,
  footerHost,
}: {
  config: ProviderConfig
  visible: boolean
  footerHost: HTMLElement | null
}): React.ReactElement | null {
  const prefix = config.provider
  const [busy, setBusy] = useAccountAddBusy()
  const proxy = useAddAccountProxy()
  if (!desktopImportAvailable(config) || !config.desktopNote) return null

  async function submit(): Promise<void> {
    await runAdd(setBusy, async () => {
      try {
        const data = (await postAccount({ provider: config.provider, importDesktop: true, proxy: proxy.read() })) as
          | { account?: AccountRecord }
          | null
        await afterAdd(data?.account?.name || '', config.label)
      } catch (error) {
        // 读不到客户端登录态时后端给 400 + 明确原因，原样透出即可
        toast(t('导入失败：{reason}', { reason: describeError(error) }), 'err')
      }
    })
  }

  return (
    <AddGroup label={t('从本机导入桌面端登录态')} hidden={!visible}>
      <p>{config.desktopNote}</p>
      <AddFoot host={footerHost} active={visible}>
        <Button
          id={`${prefix}-desktop-button`}
          title={config.desktopHint}
          onClick={() => { void submit() }}
          disabled={busy}
        >
          {busy ? t('导入中…') : t('从本机导入桌面端登录态')}
        </Button>
      </AddFoot>
    </AddGroup>
  )
}

/* ─── 访客模式（智谱清言独有）────────────────── */

/**
 * 「访客模式」段：一颗按钮 + 一个可选备注名，点下去由网关换发访客凭证。
 *
 * ── 为什么单独一个组件（而不是并进 ManualSection）──────────────
 * 它**没有任何凭证输入**：用户不需要知道 token 是什么，点一下就有账号。
 * 并进「填写凭证」会让这条路径看起来像「你得先有凭证」，那正好与它的价值相反。
 * 载荷只有 `{ provider, guest: true, name? }` —— 后端 `api::accounts` 的
 * Chatglm 分支按 `guest` 走 `guest/access` 换证（见那边的分支说明）。
 */
function GuestSection({
  config,
  visible,
  footerHost,
}: {
  config: ProviderConfig
  visible: boolean
  footerHost: HTMLElement | null
}): React.ReactElement | null {
  const guest = config.guestLogin
  const [busy, setBusy] = useAccountAddBusy()
  const proxy = useAddAccountProxy()
  if (!guest) return null
  const nameId = fieldIdOf(config, { key: 'name', label: '', placeholder: '' })

  async function submit(): Promise<void> {
    await runAdd(setBusy, async () => {
      // 备注名从输入框读（与 ManualSection 同一取值口径：空串交给后端兜底）
      const input = document.getElementById(nameId) as HTMLInputElement | null
      const name = input?.value.trim() || ''
      try {
        const data = (await postAccount({
          provider: config.provider,
          guest: true,
          proxy: proxy.read(),
          ...(name ? { name } : {}),
        })) as { account?: AccountRecord } | null
        await afterAdd(data?.account?.name || '', config.label)
      } catch (error) {
        toast(t('添加失败：{reason}', { reason: describeError(error) }), 'err')
      }
    })
  }

  return (
    <AddGroup label={t('访客模式')} hidden={!visible}>
      {guest.noteHtml ? <Note html={guest.noteHtml} /> : null}
      <div className='form-field'>
        <Label htmlFor={nameId}>{t('备注名')}</Label>
        <Input
          id={nameId}
          type='text'
          maxLength={MAX_NAME_LENGTH}
          placeholder={guest.namePlaceholder || t('可选')}
          {...draftProps(nameId)}
        />
      </div>
      <AddFoot host={footerHost} active={visible}>
        <Button id={`${config.provider}-guest-button`} onClick={() => { void submit() }} disabled={busy}>
          {busy ? guest.busyText || t('正在添加…') : guest.button}
        </Button>
      </AddFoot>
    </AddGroup>
  )
}

/* ─── 网页登录（web-login.js）─────────────────── */

function WebLoginSection({
  config,
  mode,
  onModeChange,
  region,
  visible,
  footerHost,
  userAgentRef,
  userAgent,
  onUserAgent,
}: {
  config: ProviderConfig
  mode: string
  onModeChange: (value: string) => void
  region: string
  visible: boolean
  footerHost: HTMLElement | null
  /** UA 的当前值：登录桥建号拿不到请求体，成功后才补写，所以要读最新的那一份 */
  userAgentRef: React.RefObject<string>
  userAgent: string
  onUserAgent: (value: string) => void
}): React.ReactElement | null {
  const web = config.webLogin
  const prefix = config.provider
  const uaPresets = useUserAgentPresets()
  const uaEditable = userAgentEditable(config.provider)
  const modeRef = React.useRef(mode)
  const regionRef = React.useRef(region)
  const controllerRef = React.useRef<WebLoginController | null>(null)
  const proxy = useAddAccountProxy()
  modeRef.current = mode
  regionRef.current = region
  // 网页端没有「内嵌窗口 / 系统浏览器」之分：桥接层收下 mode 后不使用它，
  // 一律 window.open 开新标签页（见 web_shim.rs 的 startLogin）。这一级整级
  // 收起，提示换成与实际行为一致的那一句。
  const isWeb = platform() === 'web'
  const [loginView, setLoginView] = React.useState<WebLoginView>(() => ({
    label: web?.button || '',
    hint: isWeb ? (web?.webHint || WEB_LOGIN_HINT) : (web?.hint || web?.modes?.[0]?.hint || ''),
    disabled: false,
    waiting: false,
  }))

  React.useEffect(() => {
    const engine = shared().wbWebLogin
    if (!engine || !web) return
    const modes = web.modes
    // 控制器在块挂载时就建（与旧实现的加载期 create 同一取向）：隐藏状态不影响
    // 主进程状态推送落到它身上的时机，切到这一家时禁用态与文案已经是对的
    controllerRef.current = engine.create({
      provider: prefix,
      buttonId: `${prefix}-web-button`,
      cancelId: `${prefix}-web-cancel`,
      hintId: `${prefix}-web-hint`,
      busyText: web.busyText,
      onStateChange: view => { setLoginView(view); proxy.setBusy(view.disabled) },
      texts: () => {
        const active = modes?.find(item => item.value === modeRef.current)
        return {
          button: web.button,
          hint: isWeb ? (web.webHint || WEB_LOGIN_HINT) : (active?.hint || web.hint || ''),
        }
      },
      // 传哪个 edition 给壳侧：写死的 edition → 地区分段当前值（Qoder 两站都要登录）
      // → 'cn'（其余家既没有地区级，后端那条链也不读这个字段）
      start: () => startLogin(
        prefix,
        web.edition || regionRef.current || 'cn',
        modeRef.current || 'embedded',
        undefined,
        { proxy: proxy.read() },
      ),
      onSuccess: async result => {
        // 网页登录由共享登录桥建号：UA 只能在建好之后补写（请求体里带不了）。
        // 必须在 closeAddModals 之前发 —— 关窗会卸载本组件，但这次请求已经发出。
        await applyUserAgentAfterCreate(createdAccountIdOf(result), config.provider, userAgentRef.current ?? '')
        closeAddModals()
        await shared().wbApp?.refresh?.()
        // 任务载荷里的警告（如 KukuAI「账号未通过上游复核」）：账号已入库，
        // 但要让用户立刻知道可能需要换一个百度账号，而不是等刷新模型才发现。
        const warning = (result as { warning?: string } | null)?.warning
        if (warning) toast(t('账号已添加，但请留意：{warning}', { warning }), 'err')
        else toast(t('✅ {label}账号已添加', { label: config.label }))
      },
    }) ?? null
    return () => { controllerRef.current?.dispose(); controllerRef.current = null }
  }, [])

  // 切换打开方式只影响文案（方法列表与所选方法都不变）
  React.useEffect(() => {
    controllerRef.current?.syncTexts()
  }, [mode])

  if (!web) return null
  return (
    <AddGroup label={t('网页登录')} hidden={!visible}>
      <Note html={web.noteHtml} />
      {web.modes?.length && !isWeb ? (
        <div className='flex flex-col gap-2.5 border-l-2 border-border pl-[11px]'>
          <span className='text-[12.5px] text-subtle'>{t('打开方式')}</span>
          <SegmentedControl
            aria-label={t('{label} 网页登录的打开方式', { label: config.label })}
            options={web.modes}
            value={mode}
            onValueChange={onModeChange}
          />
        </div>
      ) : null}
      {/* 网页登录由共享登录桥建号，UA 只能在建号成功后补写（见 onSuccess）——
          但输入框放在这里，用户不必为了填 UA 换一种添加方式 */}
      {uaEditable ? <div className='field-row'>
        <Label htmlFor={`${prefix}-web-user-agent`}>{t('User-Agent（可选）')}</Label>
        <UserAgentInput id={`${prefix}-web-user-agent`} value={userAgent}
          onChange={onUserAgent} presets={uaPresets} disabled={loginView.waiting} />
      </div> : null}
      <AddFoot host={footerHost} active={visible}
        note={<span className='detail' id={`${prefix}-web-hint`}>{loginView.hint}</span>}>
        <Button id={`${prefix}-web-button`} disabled={loginView.disabled || (isWeb && (web.desktopOnly || config.provider === 'doubaowork'))} onClick={() => controllerRef.current?.start()}>
          {loginView.waiting && <Spinner />}{loginView.label}
        </Button>
        <Button
          id={`${prefix}-web-cancel`}
          variant='outline'
          hidden={!loginView.waiting}
          onClick={() => controllerRef.current?.cancel()}
        >
          {t('取消等待')}
        </Button>
      </AddFoot>
    </AddGroup>
  )
}

/* ─── 手机验证码登录（sms-login.js）─────────────── */

/**
 * 表单三行收完（排法见 ui/css/page-accounts-providers.css 的 .sms-form 一段）：
 *
 *   手机号 *   [ 11 位大陆手机号        | 获取验证码 ]   ← 发码按钮内嵌在输入框里
 *   验证码 *   [ 6 位数字 ]  备注名 [ 可选，留空则用脱敏手机号 ]
 *              [登录并添加]  提示…
 *
 * 六个 id 与旧实现逐字一致（引擎 create() 时按 id 绑 click、之后按 id 现读
 * .value 与写按钮文案 / 禁用态），换掉的只是外面那层布局 —— 这条契约不能动。
 */
function SmsSection({
  config,
  visible,
  footerHost,
}: {
  config: ProviderConfig
  visible: boolean
  footerHost: HTMLElement | null
}): React.ReactElement | null {
  const sms = config.smsLogin
  const prefix = config.provider
  const proxy = useAddAccountProxy()

  React.useEffect(() => {
    if (!sms) return
    // DOM 由下面的 JSX 拼好（id 与旧实现逐字一致），交互全在引擎里
    shared().wbSmsLogin?.create({
      provider: prefix,
      proxy: proxy.read,
      onBusyChange: proxy.setBusy,
      onSuccess: data => afterAdd(addedLabelOf((data as { account?: AccountRecord } | null)?.account), config.label),
    })
  }, [])

  if (!sms) return null
  return (
    <AddGroup label={t('手机验证码登录')} hidden={!visible}>
      <Note
        html={sms.noteHtml}
        text={t('用 AutoClaw 账号绑定的手机号登录：点击「获取验证码」，收到短信后填入下方并登录。验证码由本机直接提交给官方接口，界面不显示 token。')}
      />
      <div className='sms-form'>
        {/* 第 1 行：手机号与发码合成一格。按钮内嵌在输入框尾部（组件库的
            InputGroup，就是为「输入框带一个动作」准备的），不再自己占一行 */}
        <div className='sms-field'>
          <Label className='lb' htmlFor={`${prefix}-sms-phone`}>
            {t('手机号')}<i className='req'>*</i>
          </Label>
          <InputGroup>
            <InputGroupInput
              id={`${prefix}-sms-phone`}
              type='text'
              maxLength={11}
              placeholder={t('11 位大陆手机号')}
              {...draftProps(`${prefix}-sms-phone`)}
            />
            <InputGroupAddon align='inline-end'>
              {/* 引擎在 create() 时就绑上这一颗的 click（文案与禁用态也归它管）。
                  ghost + 主色字：不描边框，也不在 30px 高的输入框里再嵌一个盒子 */}
              <InputGroupButton id={`${prefix}-sms-send`} className='text-primary-fg'>
                {t('获取验证码')}
              </InputGroupButton>
            </InputGroupAddon>
          </InputGroup>
        </div>

        {/* 第 2 行：验证码只占 118px（6 个数字），右边并上同样是短字段的备注名 */}
        <div className='sms-field'>
          <Label className='lb' htmlFor={`${prefix}-sms-code`}>
            {t('验证码')}<i className='req'>*</i>
          </Label>
          <div className='sms-pair'>
            {/* 验证码**不进草稿**：引擎在登录成功后会把它清掉（codeNode.value = ''），
                草稿若留着，下次挂载会把这个已经用过的码填回去 —— 手机号与备注名照旧留 */}
            <Input
              id={`${prefix}-sms-code`}
              className='sms-code'
              type='text'
              maxLength={6}
              placeholder={t('6 位数字')}
            />
            <Label className='lb-inline' htmlFor={`${prefix}-sms-name`}>{t('备注名')}</Label>
            <Input
              id={`${prefix}-sms-name`}
              className='sms-name'
              type='text'
              placeholder={t('可选，留空则用脱敏手机号')}
              {...draftProps(`${prefix}-sms-name`)}
            />
          </div>
        </div>

        <AddFoot host={footerHost} active={visible}
          note={<span className='detail sms-hint' id={`${prefix}-sms-hint`} />}>
          <Button id={`${prefix}-sms-submit`}>{t('登录并添加')}</Button>
        </AddFoot>
      </div>
    </AddGroup>
  )
}

/* ─── AutoClaw 国际版的 OAuth 网页登录（autoclaw-oauth.js）─── */

function OauthSection({
  config,
  mode,
  onModeChange,
  visible,
  footerHost,
}: {
  config: ProviderConfig
  mode: string
  onModeChange: (value: string) => void
  visible: boolean
  footerHost: HTMLElement | null
}): React.ReactElement | null {
  const oauth = config.oauthLogin
  const prefix = config.provider
  const proxy = useAddAccountProxy()
  const modeRef = React.useRef(mode)
  const controllerRef = React.useRef<OauthController | null>(null)
  modeRef.current = mode
  // 与 WebLoginSection 同一条判据：网页端没有「内嵌窗口 / 系统浏览器」之分，
  // 桥接层一律在新标签页里打开（见 web_shim.rs 的 startAutoclawOauthLogin）
  const isWeb = platform() === 'web'

  const hintOf = (): string =>
    isWeb
      ? (oauth?.webHint || WEB_LOGIN_HINT)
      : (oauth?.modes?.find(item => item.value === modeRef.current)?.hint || oauth?.hint || '')

  React.useEffect(() => {
    const engine = shared().wbAutoclawOauth
    if (!engine || !oauth) return
    controllerRef.current = engine.create({
      provider: prefix,
      // 打开方式现读（用户可能在跑验证码期间又切了那一级），快照会让最后一次切换失效
      mode: () => modeRef.current || 'embedded',
      hint: hintOf,
      proxy: proxy.read,
      onBusyChange: proxy.setBusy,
      // 全程挂着的取消按钮：验证码阶段点它 = 作废滑块等待，等登录阶段点它 = 撤掉壳侧那一轮
      cancelId: `${prefix}-oauth-cancel`,
      // 不传账号名：这条链的账号由网关侧在回调里落库（壳只回 {ok:true}），
      // 传 provider 名当 name 会得到「AutoClaw 国际版账号已添加：AutoClaw 国际版」这种同义重复
      onSuccess: () => afterAdd('', config.label),
    }) ?? null
  }, [])

  React.useEffect(() => {
    controllerRef.current?.syncTexts()
  }, [mode])

  if (!oauth) return null
  return (
    <AddGroup label={oauth.title || t('网页登录')} hidden={!visible}>
      <Note html={oauth.noteHtml} />
      {oauth.modes?.length && !isWeb ? (
        <div className='flex flex-col gap-2.5 border-l-2 border-border pl-[11px]'>
          <span className='text-[12.5px] text-subtle'>{t('打开方式')}</span>
          <SegmentedControl
            aria-label={t('{label} 网页登录的打开方式', { label: config.label })}
            options={oauth.modes}
            value={mode}
            onValueChange={onModeChange}
          />
        </div>
      ) : null}
      <AddFoot host={footerHost} active={visible}
        note={<span className='detail' id={`${prefix}-oauth-hint`}>{hintOf()}</span>}>
        {/* 两个变体各一颗：上游是两个独立端点、两套账号体系，用下拉会让用户猜。
            点击处理**不在这里**：autoclaw-oauth.js 的 create() 会自己给这三颗绑
            click（旧实现同一分工），这里再绑一次会让 start() 被调两遍 —— 也正因如此
            它们必须常驻在 DOM 里（见 add-account-kit 的 AddFoot 说明） */}
        <Button id={`${prefix}-oauth-zai`}>{t('使用 Zai 账号登录')}</Button>
        <Button id={`${prefix}-oauth-google`} variant='outline'>{t('使用 Google 账号登录')}</Button>
        {/* 取消按钮的显隐由 autoclaw-oauth.js 的 paintCancel 管（同样是 hidden 属性） */}
        <Button id={`${prefix}-oauth-cancel`} variant='outline' hidden>
          {t('取消')}
        </Button>
      </AddFoot>
    </AddGroup>
  )
}

/* ─── 一家的整块（地区 / 添加方式 / 各段落）─────────── */

export function ProviderBlock({
  config,
  active,
  footerHost,
  selectedMethod,
  hideMethodPicker = false,
}: {
  config: ProviderConfig
  active: boolean
  footerHost: HTMLElement | null
  selectedMethod?: MethodId
  hideMethodPicker?: boolean
}): React.ReactElement {
  const initialRegion = config.regionOptions?.[0]?.value ?? ''
  const [region, setRegion] = React.useState(initialRegion)
  const [method, setMethod] = React.useState<MethodId>(() => methodsOf(config, initialRegion)[0])
  const [webMode, setWebMode] = React.useState(config.webLogin?.modes?.[0]?.value ?? '')
  const [oauthMode, setOauthMode] = React.useState(config.oauthLogin?.modes?.[0]?.value ?? '')
  // 账号级自定义 User-Agent：草稿放在这一级，因为「填写凭证」与「网页登录」两条
  // 建号路径都要用它（前者随请求体提交，后者只能建号后补写）。留空 = 用该家默认。
  const [userAgent, setUserAgent] = React.useState('')
  const userAgentRef = React.useRef(userAgent)
  userAgentRef.current = userAgent

  // 地区切换后方法列表重算：收起的那一项可能正是当前选中项，此时退到第一项
  //（否则屏幕上会出现「所有段都藏着、一个可见的选中项也没有」）
  const methods = methodsOf(config, region)
  const effective = selectedMethod ?? (methods.includes(method) ? method : methods[0])
  const methodOptions = React.useMemo(
    () => ADD_METHODS.filter(item => methods.includes(item.id)).map(item => ({
      value: item.id,
      label: item.label,
    })),
    [methods.join(',')],
  )

  return (
    <div className='add-provider-block' hidden={!active}>
      {config.regionOptions?.length ? (
        <AddGroup label={t('地区')}>
          <SegmentedControl
            aria-label={t('{label} 账号地区', { label: config.label })}
            className={DIALOG_SEG_CLASS}
            options={config.regionOptions}
            value={region}
            onValueChange={setRegion}
          />
        </AddGroup>
      ) : null}

      <AddGroup label={t('添加方式')} hidden={hideMethodPicker}>
        <SegmentedControl
          aria-label={t('{label} 账号的添加方式', { label: config.label })}
          className={DIALOG_SEG_CLASS}
          options={methodOptions}
          value={effective}
          onValueChange={value => setMethod(value as MethodId)}
        />
      </AddGroup>

      {/* 段落的 `visible` 要连「这一家是不是当前选中的提供商」一起算：段落自己的
          hidden 只在块内生效，而主按钮是 portal 到弹窗底栏的 —— 出了块就没有祖先
          帮忙挡着了。少了这一半，二十来家的底栏槽位会同时挤在底栏那一行里
          （每列只剩一个字宽，提示文字变成竖排）。

          注意不能改成「不选中就不传给段落 footerHost」：那会让按钮从原位搬到
          底栏，节点一换，sms-login / autoclaw-oauth 在 create() 时绑的 click 就丢了。 */}
      <GuestSection config={config} visible={active && effective === 'guest'} footerHost={footerHost} />
      <OauthSection
        config={config}
        mode={oauthMode}
        onModeChange={setOauthMode}
        visible={active && effective === 'oauth'}
        footerHost={footerHost}
      />
      <SmsSection config={config} visible={active && effective === 'sms'} footerHost={footerHost} />
      <WebLoginSection
        config={config}
        mode={webMode}
        onModeChange={setWebMode}
        region={region}
        visible={active && effective === 'web'}
        footerHost={footerHost}
        userAgentRef={userAgentRef}
        userAgent={userAgent}
        onUserAgent={setUserAgent}
      />
      <ManualSection config={config} region={region} visible={active && effective === 'manual'}
        footerHost={footerHost} userAgent={userAgent} onUserAgent={setUserAgent} />
      <FileImportSection config={config} visible={active && effective === 'file'} footerHost={footerHost}
        userAgent={userAgent} />
      <DesktopSection config={config} visible={active && effective === 'desktop'} footerHost={footerHost} />
    </div>
  )
}
