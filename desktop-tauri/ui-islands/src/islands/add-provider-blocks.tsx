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
 * 显隐一律切 `hidden` 属性，**不是**行内 `display:none`：这些节点都是组件库的
 * DialogSection / Button，自带带 `!important` 的 `flex` / `inline-flex` 工具类，
 * 行内样式与单类名都压不过它（兜底规则见组件库 globals.css 的 `[hidden][hidden]`）。
 *
 * ── 引擎的容器契约（一个字都不能改）──────────────────────────
 *   web-login.js：按 id 找按钮 / 取消 / 提示三个节点，自己写 disabled / innerHTML /
 *                hidden —— 这三个节点的 id 与旧实现逐字一致，
 *                且**文本子节点是常量**（React 不会去覆盖引擎写进去的内容）。
 *   sms-login.js：create() 时按 id 绑 click，之后按 id 现读输入框的 .value ——
 *                六个 id 逐字一致，输入框是非受控的（React 从不写回 value）。
 *   autoclaw-oauth.js：create() 时抓两个变体按钮的文案、绑 click，同样按 id 读写。
 * 三个引擎建的 DOM 都不进 React 树，也不试图 React 化。
 */

import * as React from 'react'
import {
  Button,
  DialogSection,
  Input,
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
  SegmentedControl,
  Switch,
  Textarea,
} from '@ui'

import {
  ADD_METHODS,
  MAX_NAME_LENGTH,
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
  loginPrefs,
  postAccount,
  readField,
  shared,
  toast,
  type AccountRecord,
  type Edition,
  type LoginMode,
  type OauthController,
  type WebLoginController,
} from './add-account-bridge'
// 二进制文件 → base64 的读取工具（订阅导入在用，同一份足够：FileReader +
// readAsDataURL）；`add-provider-blocks` 只借这一个函数，不引订阅那边的状态
import { readFileAsBase64 } from './proxies-sub-state'
import { t } from '../i18n'

/**
 * 弹窗里的分段比页面上大一号（旧 #add-modal .add-seg 的 28px 项高 / 12.5px 字号，
 * 那条规则挂在 #add-modal 这个 ID 作用域上，迁到岛后弹窗不再有这个 id）。
 * 用任意变体就地复刻，避免弹窗里的分段掉回页面档位。
 */
export const ADD_SEG_CLASS =
  'max-w-full flex-wrap [&_[data-slot=segmented-item]]:h-7 [&_[data-slot=segmented-item]]:px-3 [&_[data-slot=segmented-item]]:text-[12.5px]'

/** 小浣熊手填段里那个「登录态文件在哪」的链接点了要显示的路径 */
const RACCOON_AUTH_PATH = t(
  '登录态文件路径：~/.box-agent/config/auth.json（Windows：C:\\Users\\<你的用户名>\\.box-agent\\config\\auth.json）')

/** 手填按钮的忙态：全局一把锁（旧实现的 addBusy 就是模块级，弹窗内互斥） */
let addBusy = false

async function runAdd(setBusy: (value: boolean) => void, task: () => Promise<void>): Promise<void> {
  if (addBusy) return
  addBusy = true
  setBusy(true)
  try {
    await task()
  } finally {
    addBusy = false
    setBusy(false)
  }
}

/** 带行内标记的说明：调用方给的是可信常量（与旧实现的 innerHTML 同口径），纯文本走文本节点 */
function Note({ html, text }: { html?: string; text?: string }): React.ReactElement {
  if (html) return <p dangerouslySetInnerHTML={{ __html: html }} />
  return <p>{text || ''}</p>
}

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
}: {
  config: ProviderConfig
  region: string
  visible: boolean
}): React.ReactElement {
  const prefix = config.provider
  const [busy, setBusy] = React.useState(false)
  const [hint, setHint] = React.useState('')
  const note = manualNoteOf(config)
  const fieldIds = config.fields.map(field => fieldIdOf(config, field))

  async function submit(): Promise<void> {
    const payload: Record<string, unknown> = { provider: config.provider }
    // 地区（Qoder）：整块共用一个分段控件
    if (config.regionOptions?.length) payload.mode = region
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
    await runAdd(setBusy, async () => {
      try {
        const data = (await postAccount(payload)) as { account?: AccountRecord } | null
        clearFields(fieldIds)
        setHint('')
        await afterAdd(addedLabelOf(data?.account), config.label)
      } catch (error) {
        toast(t('添加失败：{reason}', { reason: describeError(error) }), 'err')
      }
    })
  }

  return (
    <DialogSection
      hidden={!visible}
      // 小浣熊那条「登录态文件」链接：旧实现是 document 上的委托，这里收在段落上
      onClick={event => {
        if ((event.target as HTMLElement).closest('[data-raccoon-hint]')) {
          event.preventDefault()
          setHint(RACCOON_AUTH_PATH)
        }
      }}
    >
      <h3>{manualTitleOf(config)}</h3>
      <Note {...note} />
      {config.fields.map(field => {
        const id = fieldIdOf(config, field)
        // 必填只在标签上标出（弹窗不是 <form>，原生 required 不生效），真正的拦截在 submit
        const marker = field.optional ? '' : t('（必填）')
        return (
          <div key={id} className={`field-row${field.rows ? ' stack' : ''}`}>
            <label htmlFor={id}>
              {field.label}
              {marker}
            </label>
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
      <div className='field-row'>
        <Button id={`${prefix}-add-button`} onClick={() => { void submit() }} disabled={busy}>
          {busy ? t('添加中…') : addButtonTextOf(config)}
        </Button>
        {/* 引擎不碰这一格：忙态文案在按钮上，这一格只放链接给出的路径与提交失败以外的说明 */}
        <span className='detail' id={`${prefix}-add-hint`}>{hint}</span>
      </div>
    </DialogSection>
  )
}

/* ─── 从本机导入桌面端登录态 ─────────────────── */

function DesktopSection({
  config,
  visible,
}: {
  config: ProviderConfig
  visible: boolean
}): React.ReactElement | null {
  const prefix = config.provider
  const [busy, setBusy] = React.useState(false)
  if (!desktopImportAvailable(config) || !config.desktopNote) return null

  async function submit(): Promise<void> {
    await runAdd(setBusy, async () => {
      try {
        const data = (await postAccount({ provider: config.provider, importDesktop: true })) as
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
    <DialogSection hidden={!visible}>
      <h3>{t('从本机导入桌面端登录态')}</h3>
      <p>{config.desktopNote}</p>
      <div className='field-row'>
        <Button
          id={`${prefix}-desktop-button`}
          title={config.desktopHint}
          onClick={() => { void submit() }}
          disabled={busy}
        >
          {busy ? t('导入中…') : t('从本机导入桌面端登录态')}
        </Button>
      </div>
    </DialogSection>
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
}: {
  config: ProviderConfig
  visible: boolean
}): React.ReactElement | null {
  const guest = config.guestLogin
  const [busy, setBusy] = React.useState(false)
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
          ...(name ? { name } : {}),
        })) as { account?: AccountRecord } | null
        await afterAdd(data?.account?.name || '', config.label)
      } catch (error) {
        toast(t('添加失败：{reason}', { reason: describeError(error) }), 'err')
      }
    })
  }

  return (
    <DialogSection hidden={!visible}>
      <h3>{t('访客模式')}</h3>
      {guest.noteHtml ? <Note html={guest.noteHtml} /> : null}
      <div className='form-field'>
        <label htmlFor={nameId}>{t('备注名')}</label>
        <input
          id={nameId}
          type='text'
          maxLength={MAX_NAME_LENGTH}
          placeholder={guest.namePlaceholder || t('可选')}
          {...draftProps(nameId)}
        />
      </div>
      <div className='field-row'>
        <Button id={`${config.provider}-guest-button`} onClick={() => { void submit() }} disabled={busy}>
          {busy ? guest.busyText || t('正在添加…') : guest.button}
        </Button>
      </div>
    </DialogSection>
  )
}

/* ─── 导入凭证文件（通用版；WorkBuddy 那段是另一套 payload，不要合并）─── */

/**
 * `readFileAsBase64` 返回的是**完整 data URL**（`data:…;base64,<正文>`）——
 * 订阅那条链的服务端会自己 `split_once("base64,")`，而 `POST /api/accounts`
 * 的 `authFileB64` 走的是裸 `BASE64.decode(encoded)`，**不认 data URL 前缀**。
 * 因此这里剥掉逗号前的头部再交出去，否则 KukuAI 的文件导入会得到
 * 「authFileB64 不是合法的 base64」。
 */
function base64BodyOf(dataUrl: string): string {
  const comma = dataUrl.indexOf(',')
  return comma >= 0 ? dataUrl.slice(comma + 1) : dataUrl
}

/**
 * 「导入凭证文件」段（通用）：用户手选桌面客户端的凭证文件，前端读内容
 * （文本 → `authFileContent`；二进制 → `authFileB64`）交给网关解析落库。
 *
 * 与 `DesktopSection` 的差别：那边只有一颗按钮、读的是本机固定路径；这边选
 * 文件、文件可来自任意机器 / 备份，落库后是**可独立转发**的账号。
 * WorkBuddy 的同类段落不并进来：它把文件内容**摊平**成请求体字段
 * （credentials 直读顶层键），payload 形态与这里的三字段契约不同。
 */
function FileImportSection({
  config,
  visible,
}: {
  config: ProviderConfig
  visible: boolean
}): React.ReactElement | null {
  const [busy, setBusy] = React.useState(false)
  // 选中文件名回显：KukuAI 的 Cookies 没有扩展名，按钮旁看不出选的是哪一份
  const [hint, setHint] = React.useState('')
  const fileInput = React.useRef<HTMLInputElement | null>(null)
  const spec = config.fileImport
  // 备注名走草稿机制（与 ManualSection 的 name 字段同一形态）
  const nameId = `${config.provider}-file-name`
  if (!spec) return null

  async function importFile(file: File): Promise<void> {
    const fileImport = config.fileImport
    if (!fileImport) return
    await runAdd(setBusy, async () => {
      try {
        const remark = readField(nameId)
        const payload: Record<string, unknown> = {
          provider: config.provider,
          // 文件名只用于错误文案（后端会把原文件名带上），不参与解析
          authFileName: file.name,
        }
        if (fileImport.binary) {
          payload.authFileB64 = base64BodyOf(await readFileAsBase64(file))
        } else {
          const text = await file.text()
          // JSON 预检只挡「选错文件」这一层：解析失败 / 不是对象在这里就变成
          // 人话，而不是后端凭据模块吐出来的一条远端错误（与 WorkBuddy 同取向）
          let parsed: unknown
          try {
            parsed = JSON.parse(text)
          } catch {
            toast(t('选择的文件不是合法 JSON'), 'err')
            return
          }
          if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
            toast(t('不是有效的凭证文件：内容要是一个 JSON 对象'), 'err')
            return
          }
          // 原始文本整串交出去：各家顶层形状不同（auth.json / settings 文件），
          // 由后端各自的文件导入分支解析，前端不再裁剪
          payload.authFileContent = text
        }
        if (remark) payload.name = remark
        const data = (await postAccount(payload)) as { account?: AccountRecord } | null
        setHint('')
        await afterAdd(addedLabelOf(data?.account), config.label)
      } catch (error) {
        // 文件本身的问题（缺字段 / 密文解不开 / 凭证无效）后端都给 400 + 带文件名的
        // 原因，原样透出即可
        toast(t('导入失败：{reason}', { reason: describeError(error) }), 'err')
      }
    })
  }

  return (
    <DialogSection hidden={!visible}>
      <h3>{t('导入凭证文件')}</h3>
      <Note text={spec.note} />
      <div className='field-row'>
        <label className='lb-inline' htmlFor={nameId}>{t('备注名')}</label>
        <Input
          id={nameId}
          type='text'
          maxLength={MAX_NAME_LENGTH}
          placeholder={t('可选，留空则用文件里的昵称')}
          {...draftProps(nameId)}
        />
      </div>
      <div className='field-row'>
        <input
          ref={fileInput}
          type='file'
          // 空串 = 选择器不过滤（KukuAI 的 Cookies 没有扩展名，无从按类型筛）
          accept={spec.accept}
          hidden
          onChange={event => {
            const file = event.target.files?.[0]
            // 清空 value：同一份文件改内容后再选一次也要能触发 onChange
            event.target.value = ''
            if (file) {
              setHint(file.name)
              void importFile(file)
            }
          }}
        />
        <Button onClick={() => fileInput.current?.click()} disabled={busy}>
          {busy ? t('导入中…') : t('选择凭证文件导入')}
        </Button>
        <span className='detail'>{hint}</span>
      </div>
    </DialogSection>
  )
}

/* ─── 网页登录（web-login.js）─────────────────── */

function WebLoginSection({
  config,
  mode,
  onModeChange,
  region,
  visible,
}: {
  config: ProviderConfig
  mode: string
  onModeChange: (value: string) => void
  region: string
  visible: boolean
}): React.ReactElement | null {
  const web = config.webLogin
  const prefix = config.provider
  const modeRef = React.useRef(mode)
  const regionRef = React.useRef(region)
  const controllerRef = React.useRef<WebLoginController | null>(null)
  modeRef.current = mode
  regionRef.current = region

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
      texts: () => {
        const active = modes?.find(item => item.value === modeRef.current)
        return { button: web.button, hint: active?.hint || web.hint || '' }
      },
      // 传哪个 edition 给壳侧：写死的 edition → 地区分段当前值（Qoder 两站都要登录）
      // → 'cn'（其余家既没有地区级，后端那条链也不读这个字段）
      start: () => shared().workbuddyDesktop?.startLogin(
        web.edition || regionRef.current || 'cn',
        modeRef.current || 'embedded',
        prefix,
      ),
      onSuccess: async result => {
        closeAddModals()
        await shared().wbApp?.refresh?.()
        // 任务载荷里的警告（如 KukuAI「账号未通过上游复核」）：账号已入库，
        // 但要让用户立刻知道可能需要换一个百度账号，而不是等刷新模型才发现。
        const warning = (result as { payload?: { warning?: string } } | null)?.payload?.warning
        if (warning) toast(t('账号已添加，但请留意：{warning}', { warning }), 'err')
        else toast(t('✅ {label}账号已添加', { label: config.label }))
      },
    }) ?? null
  }, [])

  // 切换打开方式只影响文案（方法列表与所选方法都不变）
  React.useEffect(() => {
    controllerRef.current?.syncTexts()
  }, [mode])

  if (!web) return null
  return (
    <DialogSection hidden={!visible}>
      <h3>{t('网页登录')}</h3>
      <Note html={web.noteHtml} />
      {web.modes?.length ? (
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
      <div className='field-row'>
        {/* 按钮与提示的文本子节点必须是常量：引擎会直接改它们的 textContent */}
        <Button id={`${prefix}-web-button`} onClick={() => controllerRef.current?.start()}>
          {web.button}
        </Button>
        {/* 收起时靠 hidden 属性（不是行内 display）：引擎在 waiting 与空闲之间
            切的就是它（见 web-login.js 的 applyState）。按钮是组件库的 Button，
            inline-flex 工具类带 !important，行内样式压不过 */}
        <Button
          id={`${prefix}-web-cancel`}
          variant='outline'
          hidden
          onClick={() => controllerRef.current?.cancel()}
        >
          {t('取消等待')}
        </Button>
        <span className='detail' id={`${prefix}-web-hint`}>
          {web.hint || web.modes?.[0]?.hint || ''}
        </span>
      </div>
    </DialogSection>
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
}: {
  config: ProviderConfig
  visible: boolean
}): React.ReactElement | null {
  const sms = config.smsLogin
  const prefix = config.provider

  React.useEffect(() => {
    if (!sms) return
    // DOM 由下面的 JSX 拼好（id 与旧实现逐字一致），交互全在引擎里
    shared().wbSmsLogin?.create({
      provider: prefix,
      onSuccess: data => afterAdd(addedLabelOf((data as { account?: AccountRecord } | null)?.account), config.label),
    })
  }, [])

  if (!sms) return null
  return (
    <DialogSection hidden={!visible}>
      <h3>{t('手机验证码登录')}</h3>
      <Note
        html={sms.noteHtml}
        text={t('用 AutoClaw 账号绑定的手机号登录：点击「获取验证码」，收到短信后填入下方并登录。验证码由本机直接提交给官方接口，界面不显示 token。')}
      />
      <div className='sms-form'>
        {/* 第 1 行：手机号与发码合成一格。按钮内嵌在输入框尾部（组件库的
            InputGroup，就是为「输入框带一个动作」准备的），不再自己占一行 */}
        <div className='sms-field'>
          <label className='lb' htmlFor={`${prefix}-sms-phone`}>
            {t('手机号')}<i className='req'>*</i>
          </label>
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
          <label className='lb' htmlFor={`${prefix}-sms-code`}>
            {t('验证码')}<i className='req'>*</i>
          </label>
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
            <label className='lb-inline' htmlFor={`${prefix}-sms-name`}>{t('备注名')}</label>
            <Input
              id={`${prefix}-sms-name`}
              className='sms-name'
              type='text'
              placeholder={t('可选，留空则用脱敏手机号')}
              {...draftProps(`${prefix}-sms-name`)}
            />
          </div>
        </div>

        <div className='sms-foot'>
          <Button id={`${prefix}-sms-submit`}>{t('登录并添加')}</Button>
          {/* 失败时引擎会给它挂 .err（见 setHint） */}
          <span className='detail sms-hint' id={`${prefix}-sms-hint`} />
        </div>
      </div>
    </DialogSection>
  )
}

/* ─── AutoClaw 国际版的 OAuth 网页登录（autoclaw-oauth.js）─── */

function OauthSection({
  config,
  mode,
  onModeChange,
  visible,
}: {
  config: ProviderConfig
  mode: string
  onModeChange: (value: string) => void
  visible: boolean
}): React.ReactElement | null {
  const oauth = config.oauthLogin
  const prefix = config.provider
  const modeRef = React.useRef(mode)
  const controllerRef = React.useRef<OauthController | null>(null)
  modeRef.current = mode

  const hintOf = (): string =>
    oauth?.modes?.find(item => item.value === modeRef.current)?.hint || oauth?.hint || ''

  React.useEffect(() => {
    const engine = shared().wbAutoclawOauth
    if (!engine || !oauth) return
    controllerRef.current = engine.create({
      provider: prefix,
      // 打开方式现读（用户可能在跑验证码期间又切了那一级），快照会让最后一次切换失效
      mode: () => modeRef.current || 'embedded',
      hint: hintOf,
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
    <DialogSection hidden={!visible}>
      <h3>{oauth.title || t('网页登录')}</h3>
      <Note html={oauth.noteHtml} />
      {oauth.modes?.length ? (
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
      <div className='field-row'>
        {/* 两个变体各一颗：上游是两个独立端点、两套账号体系，用下拉会让用户猜。
            点击处理**不在这里**：autoclaw-oauth.js 的 create() 会自己给这三颗绑
            click（旧实现同一分工），这里再绑一次会让 start() 被调两遍 */}
        <Button id={`${prefix}-oauth-zai`}>{t('使用 Zai 账号登录')}</Button>
        <Button id={`${prefix}-oauth-google`} variant='outline'>{t('使用 Google 账号登录')}</Button>
        {/* 取消按钮的显隐由 autoclaw-oauth.js 的 paintCancel 管（同样是 hidden 属性） */}
        <Button id={`${prefix}-oauth-cancel`} variant='outline' hidden>
          {t('取消')}
        </Button>
      </div>
      <div className='field-row'>
        <span className='detail' id={`${prefix}-oauth-hint`}>
          {oauth.modes?.[0]?.hint || oauth.hint || ''}
        </span>
      </div>
    </DialogSection>
  )
}

/* ─── 一家的整块（地区 / 添加方式 / 各段落）─────────── */

export function ProviderBlock({
  config,
  active,
}: {
  config: ProviderConfig
  active: boolean
}): React.ReactElement {
  const initialRegion = config.regionOptions?.[0]?.value ?? ''
  const [region, setRegion] = React.useState(initialRegion)
  const [method, setMethod] = React.useState<MethodId>(() => methodsOf(config, initialRegion)[0])
  const [webMode, setWebMode] = React.useState(config.webLogin?.modes?.[0]?.value ?? '')
  const [oauthMode, setOauthMode] = React.useState(config.oauthLogin?.modes?.[0]?.value ?? '')

  // 地区切换后方法列表重算：收起的那一项可能正是当前选中项，此时退到第一项
  //（否则屏幕上会出现「所有段都藏着、一个可见的选中项也没有」）
  const methods = methodsOf(config, region)
  const effective = methods.includes(method) ? method : methods[0]
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
        <DialogSection>
          <h3>{t('地区')}</h3>
          <SegmentedControl
            aria-label={t('{label} 账号地区', { label: config.label })}
            className={ADD_SEG_CLASS}
            options={config.regionOptions}
            value={region}
            onValueChange={setRegion}
          />
        </DialogSection>
      ) : null}

      <DialogSection>
        <h3>{t('添加方式')}</h3>
        <SegmentedControl
          aria-label={t('{label} 账号的添加方式', { label: config.label })}
          className={ADD_SEG_CLASS}
          options={methodOptions}
          value={effective}
          onValueChange={value => setMethod(value as MethodId)}
        />
      </DialogSection>

      <GuestSection config={config} visible={effective === 'guest'} />
      <OauthSection
        config={config}
        mode={oauthMode}
        onModeChange={setOauthMode}
        visible={effective === 'oauth'}
      />
      <SmsSection config={config} visible={effective === 'sms'} />
      <WebLoginSection
        config={config}
        mode={webMode}
        onModeChange={setWebMode}
        region={region}
        visible={effective === 'web'}
      />
      <ManualSection config={config} region={region} visible={effective === 'manual'} />
      <FileImportSection config={config} visible={effective === 'file'} />
      <DesktopSection config={config} visible={effective === 'desktop'} />
    </div>
  )
}

/* ─── WorkBuddy 导入凭证文件 ─────────────────── */

/** 官方客户端凭证目录（点「凭证文件在哪」时显示；国内版 / 国际版共用同一目录） */
const WORKBUDDY_AUTH_PATH = t(
  '官方客户端凭证目录：Windows 是 C:\\Users\\<你的用户名>\\AppData\\Local\\CodeBuddyExtension\\Data\\Public\\auth，macOS 是 ~/Library/Application Support/CodeBuddyExtension/Data/Public/auth',
)

/** 备注名输入框的 id（草稿按 id 存，见 add-account-bridge 的 draft 机制） */
const WORKBUDDY_FILE_NAME_ID = 'workbuddy-file-name'

/**
 * 从凭证对象取字段：先读嵌套对象（`auth` / `account`，官方 `.info` 与 wb2api
 * 的嵌套形），读不到再读顶层键（wb2api 的手写扁平形）。键名清单与后端
 * `store_crud::add_account` 的取值口径一致，前端先挡一道是为了把「粘错文件」
 * 变成一句人话，而不是后端 400 的「缺少 accessToken」。
 */
function credentialFieldOf(
  record: Record<string, unknown>,
  keys: string[],
  nestedKey: string,
): string {
  const nested = record[nestedKey]
  const sources: Array<Record<string, unknown>> = []
  if (nested && typeof nested === 'object' && !Array.isArray(nested)) {
    sources.push(nested as Record<string, unknown>)
  }
  sources.push(record)
  for (const source of sources) {
    for (const key of keys) {
      const value = source[key]
      if (typeof value === 'string' && value.trim()) return value.trim()
    }
  }
  return ''
}

/** 按文件内容推断归属：`auth.domain`（官方 .info）或顶层 `domain`（扁平形） */
function editionFromFile(record: Record<string, unknown>): Edition | null {
  const domain = credentialFieldOf(record, ['domain'], 'auth')
  if (/workbuddy\.ai/i.test(domain)) return 'intl'
  if (/workbuddy\.cn|codebuddy\.cn/i.test(domain)) return 'cn'
  return null
}

/** WorkBuddy 块「添加方式」的选项 id。不进 ADD_METHODS：本块不走通用分派，选项固定这三项 */
type WorkBuddyMethod = 'web' | 'file' | 'desktop'

/**
 * 「导入凭证文件」段（WorkBuddyBlock 专用）：选文件导入 —— 官方客户端登录态
 * （`.info`）或 wb2api 的账号 JSON。「从本机导入桌面端登录态」是独立的添加
 * 方式（见 `WorkBuddyDesktopImportSection`），不放在本段里。
 * 归属（国内版 / 国际版）优先按文件内容的 domain 推断：文件自己的事实比
 * 「账号版本」分段更权威，推不出才用分段当前值。
 */
function WorkBuddyFileImportSection({
  edition,
  visible,
}: {
  edition: Edition
  visible: boolean
}): React.ReactElement {
  const [busy, setBusy] = React.useState(false)
  const [hint, setHint] = React.useState('')
  const fileInput = React.useRef<HTMLInputElement | null>(null)

  async function importParsed(file: File): Promise<void> {
    await runAdd(setBusy, async () => {
      try {
        let parsed: unknown
        try {
          parsed = JSON.parse(await file.text())
        } catch {
          toast(t('选择的文件不是合法 JSON'), 'err')
          return
        }
        if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
          toast(t('不是有效的凭证文件：内容要是一个 JSON 对象'), 'err')
          return
        }
        const record = parsed as Record<string, unknown>
        const accessToken = credentialFieldOf(record, ['accessToken', 'token', 'access_token'], 'auth')
        const uid = credentialFieldOf(record, ['uid'], 'account')
        if (!accessToken || !uid) {
          toast(t('不是有效的凭证文件：需要包含 accessToken 与 uid'), 'err')
          return
        }
        const fromFile = editionFromFile(record)
        const landed = fromFile ?? edition
        if (fromFile && fromFile !== edition) {
          toast(
            t('文件是{edition}凭证，已按该版本导入', {
              edition: fromFile === 'intl' ? t('国际版') : t('国内版'),
            }),
            'warn',
          )
        }
        const remark = readField(WORKBUDDY_FILE_NAME_ID)
        const payload: Record<string, unknown> = { ...record, provider: workbuddyProviderId(landed) }
        if (remark) payload.name = remark
        const data = (await postAccount(payload)) as { account?: AccountRecord } | null
        setHint('')
        await afterAdd(addedLabelOf(data?.account), landed === 'intl' ? 'WorkBuddy AI' : 'WorkBuddy')
      } catch (error) {
        toast(t('导入失败：{reason}', { reason: describeError(error) }), 'err')
      }
    })
  }

  return (
    <DialogSection hidden={!visible}>
      <h3>{t('导入凭证文件')}</h3>
      <Note text={t('选择官方客户端的登录态凭证文件（workbuddy-desktop.info / workbuddy-desktop-ai.info）或 wb2api 导出的账号 JSON（嵌套 / 扁平形均可）。')} />
      <p className='detail'>
        <a
          href='#'
          onClick={event => {
            event.preventDefault()
            setHint(WORKBUDDY_AUTH_PATH)
          }}
        >
          {t('凭证文件在哪？')}
        </a>
      </p>
      <div className='field-row'>
        <label className='lb-inline' htmlFor={WORKBUDDY_FILE_NAME_ID}>{t('备注名')}</label>
        <Input
          id={WORKBUDDY_FILE_NAME_ID}
          type='text'
          maxLength={100}
          placeholder={t('可选，留空则用文件里的昵称')}
          {...draftProps(WORKBUDDY_FILE_NAME_ID)}
        />
      </div>
      <div className='field-row'>
        <input
          ref={fileInput}
          type='file'
          accept='.info,.json,application/json'
          hidden
          onChange={event => {
            const file = event.target.files?.[0]
            // 清空 value：同一份文件改内容后再选一次也要能触发 onChange
            event.target.value = ''
            if (file) void importParsed(file)
          }}
        />
        <Button onClick={() => fileInput.current?.click()} disabled={busy}>
          {busy ? t('导入中…') : t('选择凭证文件导入')}
        </Button>
        <span className='detail'>{hint}</span>
      </div>
    </DialogSection>
  )
}

/**
 * 「从本机导入桌面端登录态」段（WorkBuddyBlock 专用）：后端扫描官方客户端凭证
 * 目录、取最新的登录态文件落库（`importDesktop` 链路，见
 * `account_store::workbuddy_import`）。与「导入凭证文件」分成两个添加方式 ——
 * 对齐小浣熊等家 DesktopSection 的形态：一颗按钮，不选文件。
 */
function WorkBuddyDesktopImportSection({
  edition,
  visible,
}: {
  edition: Edition
  visible: boolean
}): React.ReactElement {
  const [busy, setBusy] = React.useState(false)

  async function importFromDesktop(): Promise<void> {
    await runAdd(setBusy, async () => {
      try {
        const data = (await postAccount({
          provider: workbuddyProviderId(edition),
          importDesktop: true,
        })) as { account?: AccountRecord } | null
        await afterAdd(addedLabelOf(data?.account), edition === 'intl' ? 'WorkBuddy AI' : 'WorkBuddy')
      } catch (error) {
        // 读不到官方客户端登录态时后端给 400 + 明确原因，原样透出即可
        toast(t('导入失败：{reason}', { reason: describeError(error) }), 'err')
      }
    })
  }

  return (
    <DialogSection hidden={!visible}>
      <h3>{t('从本机导入桌面端登录态')}</h3>
      <Note text={t('读本机官方客户端（Tencent WorkBuddy / WorkBuddy AI）当前登录的账号：自动定位其凭证目录里最新的登录态文件（需已在本机官方客户端登录）。导入后由网关自动续期，无需跟随 2 小时时效手动换令牌。')} />
      <div className='field-row'>
        <Button onClick={() => void importFromDesktop()} disabled={busy}>
          {busy ? t('导入中…') : t('从本机导入桌面端登录态')}
        </Button>
      </div>
    </DialogSection>
  )
}

/**
 * 账号版本 → provider id。
 *
 * WorkBuddy 拆家（2026-10）后两个地区是**两家 provider**（`workbuddy` /
 * `workbuddy-intl`，见 `providers::workbuddy::region`）。界面上仍然是同一个
 * 「添加账号」块里的一个分段控件，因此这个映射只在这里写一份 —— 壳侧按它记录
 * 「这次登录属于哪一家」、后端按它决定账号落进哪一组，写错任一处的症状都是
 * 「用国际版登录、账号进了国内版组」（转发稳定 401）。
 */
function workbuddyProviderId(edition: Edition): string {
  return edition === 'intl' ? 'workbuddy-intl' : 'workbuddy'
}

/* ─── WorkBuddy（账号版本 + 添加方式[网页登录 / 导入凭证文件 / 导入桌面端登录态]）───
 *
 * 结构与其余各家不同：它有两处静态分段（账号版本 / 打开方式）与一个第三方入口
 * 开关，且网页登录的文案随打开方式变；「添加方式」分段在网页登录、导入凭证
 * 文件与导入桌面端登录态之间切换（显隐同 ProviderBlock 的手法：hidden 属性，
 * 不是行内样式）。旧实现里这段 DOM 写在 index.html 里、交互在 add-account.js；
 * 迁到岛上后整块在这里，index.html 的那 47 行骨架随之删除。 */

export function WorkBuddyBlock({ active }: { active: boolean }): React.ReactElement {
  // 三处偏好的初值来自模块级 loginPrefs：旧实现里它们住在静态 DOM 上，
  // 关掉弹窗再打开仍是上次的选择（resetAddStep 只复位提供商与步骤）
  const [edition, setEdition] = React.useState<Edition>(loginPrefs.edition)
  const [loginMode, setLoginMode] = React.useState<LoginMode>(loginPrefs.loginMode)
  const [socialRestore, setSocialRestore] = React.useState(loginPrefs.socialRestore)
  // 添加方式（与各家 ProviderBlock 的「添加方式」分段同一形态）：不进 loginPrefs
  // 持久化 —— 通用块的 method 也不持久化，弹窗重开回默认项
  const [method, setMethod] = React.useState<WorkBuddyMethod>('web')
  const editionRef = React.useRef(edition)
  const modeRef = React.useRef(loginMode)
  const socialRef = React.useRef(socialRestore)
  const controllerRef = React.useRef<WebLoginController | null>(null)
  editionRef.current = edition
  modeRef.current = loginMode
  socialRef.current = socialRestore

  React.useEffect(() => {
    const engine = shared().wbWebLogin
    if (!engine) return
    controllerRef.current = engine.create({
      // provider 随分段控件**动态解析**（getter）：引擎按
      // `loginProvider === config.provider` 判断「等待中的是不是本家」，
      // 写死一个 id 会让切到国际版之后按钮禁用态、取消与提示全部失联
      // （壳侧记的是 workbuddy-intl）。
      get provider() {
        return workbuddyProviderId(editionRef.current)
      },
      buttonId: 'web-login-button',
      cancelId: 'web-login-cancel',
      hintId: 'web-login-hint',
      busyText: t('等待网页登录…'),
      texts: () => {
        const external = modeRef.current === 'external'
        return {
          button: external ? t('在浏览器中打开登录页') : t('打开网页登录'),
          hint: external
            ? t('系统浏览器打开（复用已登录账号）；完成后自动加入列表')
            : t('内嵌窗口打开；完成后自动加入列表，关窗即取消等待'),
        }
      },
      start: () => shared().workbuddyDesktop?.startLogin(
        editionRef.current,
        modeRef.current,
        // 归属取当前分段：拆家后它是权威（后端按 provider id 反查地区，
        // 不再从 `edition` 反推，见 api::session 的 login_start）
        workbuddyProviderId(editionRef.current),
        socialRef.current,
      ),
      onSuccess: async () => {
        const editionLabel = editionRef.current === 'intl' ? t('国际版') : t('国内版')
        closeAddModals()
        await shared().wbApp?.refresh?.()
        toast(t('✅ 登录成功，{edition}账号已加入列表', { edition: editionLabel }))
      },
    }) ?? null
  }, [])

  // 打开方式影响登录按钮与提示的文案（等待中不覆盖，交给引擎的 applyState）
  React.useEffect(() => {
    controllerRef.current?.syncTexts()
  }, [edition, loginMode])

  /** 切版本要联动打开方式：国际版默认系统浏览器、国内版默认内嵌窗口（沿用改造前的联动） */
  function pickEdition(next: Edition): void {
    const mode: LoginMode = next === 'intl' ? 'external' : 'embedded'
    setEdition(next)
    loginPrefs.edition = next
    setLoginMode(mode)
    loginPrefs.loginMode = mode
  }

  function pickLoginMode(next: string): void {
    const mode: LoginMode = next === 'external' ? 'external' : 'embedded'
    setLoginMode(mode)
    loginPrefs.loginMode = mode
  }

  /**
   * 「恢复 Google / GitHub 入口」的可用条件：**国际版 + 内嵌窗口**。
   * 系统浏览器模式下页面跑在用户自己的浏览器里，我们没有注入能力；国内版登录页
   * 根本没有这两个入口（它用微信 / 手机号 / 邮箱 / SSO），壳侧也会忽略这个值。
   * 不满足时置灰，避免给出一个「选了不生效」的假选项。
   */
  const intl = edition === 'intl'
  const embedded = loginMode === 'embedded'
  const socialUsable = intl && embedded
  const socialTitle = socialUsable
    ? t('国际版登录页默认只显示邮箱登录，勾选后恢复 Google / GitHub / X 入口')
    : !intl
      ? t('国内版登录页没有 Google / GitHub 入口（它用微信 / 手机号 / 邮箱登录）')
      : t('只有「内嵌窗口」能恢复第三方入口：系统浏览器里我们无法改动登录页')

  // 分段选项随平台变化：网页托管面板没有本机官方客户端，「导入桌面端登录态」
  // 整项收起（与 desktopImportAvailable 的 platform 判断同一口径）。当前选中项
  // 被收起时退回网页登录，避免「所有段都藏着、一个可见的选中项也没有」。
  const effectiveMethod: WorkBuddyMethod =
    method === 'desktop' && platform() === 'web' ? 'web' : method

  return (
    <div className='add-provider-block' hidden={!active}>
      <DialogSection>
        <h3>{t('账号版本')}</h3>
        <p>
          {t('两版账号可同时保存，各自一份模型清单（国内版与国际版是两家提供商，可分别启用与映射）。')}
        </p>
        <SegmentedControl
          aria-label={t('账号版本')}
          className={ADD_SEG_CLASS}
          options={[
            { value: 'cn', label: t('国内版（WorkBuddy）') },
            { value: 'intl', label: t('国际版（WorkBuddy AI）') },
          ]}
          value={edition}
          onValueChange={value => pickEdition(value as Edition)}
        />
      </DialogSection>

      <DialogSection>
        <h3>{t('添加方式')}</h3>
        <SegmentedControl
          aria-label={t('{label} 账号的添加方式', { label: 'WorkBuddy' })}
          className={ADD_SEG_CLASS}
          options={[
            { value: 'web', label: t('网页登录') },
            { value: 'file', label: t('导入凭证文件') },
            ...(platform() !== 'web'
              ? [{ value: 'desktop', label: t('导入桌面端登录态') }]
              : []),
          ]}
          value={effectiveMethod}
          onValueChange={value => setMethod(value as WorkBuddyMethod)}
        />
      </DialogSection>

      <DialogSection hidden={effectiveMethod !== 'web'}>
        <h3>{t('网页登录')}</h3>
        <p>{t('完成登录后自动加入账号列表，昵称取自上游。')}</p>
        <div className='field-row'>
          <span className='text-[12.5px] whitespace-nowrap text-subtle'>{t('打开方式')}</span>
          <SegmentedControl
            aria-label={t('网页登录的打开方式')}
            className={ADD_SEG_CLASS}
            options={[
              { value: 'embedded', label: t('内嵌窗口') },
              { value: 'external', label: t('系统默认浏览器') },
            ]}
            value={loginMode}
            onValueChange={pickLoginMode}
          />
          <label className='flex items-center gap-2' title={socialTitle}>
            <Switch
              checked={socialRestore}
              disabled={!socialUsable}
              onCheckedChange={next => {
                // 勾选本身只影响发起登录时传给壳侧的值，不需要重建界面
                setSocialRestore(next)
                loginPrefs.socialRestore = next
              }}
            />
            <span className='text-[12.5px] text-subtle'>{t('恢复 Google / GitHub 入口')}</span>
          </label>
        </div>
        <p>{t('系统浏览器可复用已有登录态（国际版推荐）；内嵌窗口更干净。')}</p>
        <div className='field-row'>
          {/* 按钮与提示的文本子节点必须是常量：引擎会直接改它们的 textContent */}
          <Button id='web-login-button' onClick={() => controllerRef.current?.start()}>
            {t('打开网页登录')}
          </Button>
          <Button
            id='web-login-cancel'
            variant='outline'
            hidden
            onClick={() => controllerRef.current?.cancel()}
          >
            {t('取消等待')}
          </Button>
          <span className='detail' id='web-login-hint'>
            {t('内嵌窗口打开；完成后自动加入列表，关窗即取消等待')}
          </span>
        </div>
      </DialogSection>

      <WorkBuddyFileImportSection edition={edition} visible={effectiveMethod === 'file'} />
      <WorkBuddyDesktopImportSection edition={edition} visible={effectiveMethod === 'desktop'} />
    </div>
  )
}
