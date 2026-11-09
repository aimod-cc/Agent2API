import * as React from 'react'
import { Button, Input, Label, Field, FieldGroup, FieldLabel, SegmentedControl, Spinner, Switch } from '@ui'
import { AddFoot, AddGroup } from './add-account-kit'
import { WEB_LOGIN_HINT, platform } from './add-account-configs'
import { afterAdd, addedLabelOf, closeAddModals, describeError, draftProps, loginPrefs, postAccount, readField, shared, startLogin, toast, type AccountRecord, type Edition, type LoginMode, type WebLoginController, type WebLoginView } from './add-account-bridge'
import { accountFilePayload, accountExportEdition, isAccountExport } from './add-account-file'
import { Note, runAdd } from './add-provider-common'
import { useAddAccountProxy, useAccountAddBusy } from './add-account-proxy'
import { DIALOG_SEG_CLASS } from './dialog-segmented'
import { t } from '../i18n'

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
  footerHost,
}: {
  edition: Edition
  visible: boolean
  footerHost: HTMLElement | null
}): React.ReactElement {
  const [busy, setBusy] = useAccountAddBusy()
  const proxy = useAddAccountProxy()
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
        const fromFile = accountExportEdition(record) ?? editionFromFile(record)
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
        const payload: Record<string, unknown> = isAccountExport(parsed)
          ? accountFilePayload(record, workbuddyProviderId(landed), remark)
          : { ...record, provider: workbuddyProviderId(landed) }
        if (remark) payload.name = remark
        Object.assign(payload, isAccountExport(payload) ? proxy.importFields() : { proxy: proxy.read() })
        const data = (await postAccount(payload)) as { account?: AccountRecord } | null
        setHint('')
        await afterAdd(addedLabelOf(data?.account), landed === 'intl' ? 'WorkBuddy AI' : 'WorkBuddy')
      } catch (error) {
        toast(t('导入失败：{reason}', { reason: describeError(error) }), 'err')
      }
    })
  }

  return (
    <AddGroup label={t('导入凭证文件')} hidden={!visible}>
      <Note text={t('选择官方客户端的登录态凭证文件（workbuddy-desktop.info / workbuddy-desktop-ai.info）或 wb2api 导出的账号 JSON（嵌套 / 扁平形均可）。')} />
      <p className='detail'>{t('也支持本软件导出的单账号 JSON 文件，保留账号凭证与附加信息。')}</p>
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
      <Field orientation='horizontal' className='field-row'>
        <FieldLabel className='lb-inline' htmlFor={WORKBUDDY_FILE_NAME_ID}>{t('备注名')}</FieldLabel>
        <Input
          id={WORKBUDDY_FILE_NAME_ID}
          type='text'
          maxLength={100}
          placeholder={t('可选，留空则用文件里的昵称')}
          {...draftProps(WORKBUDDY_FILE_NAME_ID)}
        />
      </Field>
      <Input
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
      <AddFoot host={footerHost} active={visible} note={<span className='detail'>{hint}</span>}>
        <Button onClick={() => fileInput.current?.click()} disabled={busy}>
          {busy ? t('导入中…') : t('选择凭证文件导入')}
        </Button>
      </AddFoot>
    </AddGroup>
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
  footerHost,
}: {
  edition: Edition
  visible: boolean
  footerHost: HTMLElement | null
}): React.ReactElement {
  const [busy, setBusy] = useAccountAddBusy()
  const proxy = useAddAccountProxy()

  async function importFromDesktop(): Promise<void> {
    await runAdd(setBusy, async () => {
      try {
        const data = (await postAccount({
          provider: workbuddyProviderId(edition),
          importDesktop: true,
          proxy: proxy.read(),
        })) as { account?: AccountRecord } | null
        await afterAdd(addedLabelOf(data?.account), edition === 'intl' ? 'WorkBuddy AI' : 'WorkBuddy')
      } catch (error) {
        // 读不到官方客户端登录态时后端给 400 + 明确原因，原样透出即可
        toast(t('导入失败：{reason}', { reason: describeError(error) }), 'err')
      }
    })
  }

  return (
    <AddGroup label={t('从本机导入桌面端登录态')} hidden={!visible}>
      <Note text={t('读本机官方客户端（Tencent WorkBuddy / WorkBuddy AI）当前登录的账号：自动定位其凭证目录里最新的登录态文件（需已在本机官方客户端登录）。导入后由网关自动续期，无需跟随 2 小时时效手动换令牌。')} />
      <AddFoot host={footerHost} active={visible}>
        <Button onClick={() => void importFromDesktop()} disabled={busy}>
          {busy ? t('导入中…') : t('从本机导入桌面端登录态')}
        </Button>
      </AddFoot>
    </AddGroup>
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

export function WorkBuddyBlock({ active, footerHost, initialEdition }: {
  active: boolean
  footerHost: HTMLElement | null
  initialEdition?: Edition
}): React.ReactElement {
  const proxy = useAddAccountProxy()
  // 三处偏好的初值来自模块级 loginPrefs：旧实现里它们住在静态 DOM 上，
  // 关掉弹窗再打开仍是上次的选择（resetAddStep 只复位提供商与步骤）
  const [edition, setEdition] = React.useState<Edition>(initialEdition ?? loginPrefs.edition)
  const [loginMode, setLoginMode] = React.useState<LoginMode>(initialEdition
    ? initialEdition === 'intl' ? 'external' : 'embedded' : loginPrefs.loginMode)
  const [socialRestore, setSocialRestore] = React.useState(loginPrefs.socialRestore)
  // 网页端没有「内嵌窗口 / 系统浏览器」之分：桥接层一律在新标签页里打开，
  // mode 收下即弃（见 web_shim.rs 的 startLogin）。整行打开方式与第三方入口
  // 开关都收起 —— 后者只在「内嵌窗口」下生效，留着必然是个灰按不动的假选项。
  const webPanel = platform() === 'web'
  const [loginView, setLoginView] = React.useState<WebLoginView>(() => ({
    label: t('打开网页登录'),
    hint: webPanel ? WEB_LOGIN_HINT : t('内嵌窗口打开；完成后自动加入列表，关窗即取消等待'),
    disabled: false,
    waiting: false,
  }))
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
      onStateChange: view => { setLoginView(view); proxy.setBusy(view.disabled) },
      texts: () => {
        // 网页端只有一条路：新标签页。模式相关的文案（「内嵌窗口」「关窗即
        // 取消」）在这里都不成立 —— 桥接层的 startLogin 收下 mode 却不使用它
        if (webPanel) return { button: t('打开网页登录'), hint: WEB_LOGIN_HINT }
        const external = modeRef.current === 'external'
        return {
          button: external ? t('在浏览器中打开登录页') : t('打开网页登录'),
          hint: external
            ? t('系统浏览器打开（复用已登录账号）；完成后自动加入列表')
            : t('内嵌窗口打开；完成后自动加入列表，关窗即取消等待'),
        }
      },
      start: () => startLogin(
        workbuddyProviderId(editionRef.current),
        editionRef.current,
        modeRef.current,
        socialRef.current,
        { proxy: proxy.read() },
      ),
      onSuccess: async result => {
        const editionLabel = editionRef.current === 'intl' ? t('国际版') : t('国内版')
        closeAddModals()
        await shared().wbApp?.refresh?.()
        toast(t('✅ 登录成功，{edition}账号已加入列表', { edition: editionLabel }))
        const warning = (result as { warning?: string } | null)?.warning
        if (warning) toast(t('账号已添加，但请留意：{warning}', { warning }), 'warn')
      },
    }) ?? null
    return () => { controllerRef.current?.dispose(); controllerRef.current = null }
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

  // 第三方入口只在国际版的内嵌窗口里有效；切换模式时保留之前的开关偏好。
  const socialUsable = edition === 'intl' && loginMode === 'embedded'

  // 分段选项随平台变化：网页托管面板没有本机官方客户端，「导入桌面端登录态」
  // 整项收起（与 desktopImportAvailable 的 platform 判断同一口径）。当前选中项
  // 被收起时退回网页登录，避免「所有段都藏着、一个可见的选中项也没有」。
  const effectiveMethod: WorkBuddyMethod =
    method === 'desktop' && platform() === 'web' ? 'web' : method

  return (
    <div className='add-provider-block' hidden={!active}>
      <AddGroup label={t('账号版本')}>
        <p>
          {t('两版账号可同时保存，各自一份模型清单（国内版与国际版是两家提供商，可分别启用与映射）。')}
        </p>
        <SegmentedControl
          aria-label={t('账号版本')}
          className={DIALOG_SEG_CLASS}
          options={[
            { value: 'cn', label: t('国内版（WorkBuddy）') },
            { value: 'intl', label: t('国际版（WorkBuddy AI）') },
          ]}
          value={edition}
          onValueChange={value => pickEdition(value as Edition)}
        />
      </AddGroup>

      <AddGroup label={t('添加方式')}>
        <SegmentedControl
          aria-label={t('{label} 账号的添加方式', { label: 'WorkBuddy' })}
          className={DIALOG_SEG_CLASS}
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
      </AddGroup>

      <AddGroup label={t('网页登录')} hidden={effectiveMethod !== 'web'}>
        <p>{t('完成登录后自动加入账号列表，昵称取自上游。')}</p>
        {/* 网页端没有「内嵌窗口 / 系统浏览器」之分（桥接层一律开新标签页），
            整行收起 —— 连带那个只在「内嵌窗口」下生效的 Google / GitHub 开关 */}
        {webPanel ? null : (
          <>
            <FieldGroup orientation='horizontal' className='add-login-setting-row'>
              <span className='text-[12.5px] whitespace-nowrap text-subtle'>{t('打开方式')}</span>
              <SegmentedControl
                aria-label={t('网页登录的打开方式')}
                className={DIALOG_SEG_CLASS}
                options={[
                  { value: 'embedded', label: t('内嵌窗口') },
                  { value: 'external', label: t('系统默认浏览器') },
                ]}
                value={loginMode}
                onValueChange={pickLoginMode}
              />
            </FieldGroup>
            {socialUsable ? (
              <Label className='add-login-setting-row' title={t('国际版登录页默认只显示邮箱登录，勾选后恢复 Google / GitHub / X 入口')}>
                <span className='text-[12.5px] text-subtle'>{t('恢复 Google / GitHub 入口')}</span>
                <Switch
                  checked={socialRestore}
                  onCheckedChange={next => {
                    setSocialRestore(next)
                    loginPrefs.socialRestore = next
                  }}
                />
              </Label>
            ) : null}
            <p>{t('系统浏览器可复用已有登录态（国际版推荐）；内嵌窗口更干净。')}</p>
          </>
        )}
        <AddFoot host={footerHost} active={active && effectiveMethod === 'web'}
          note={<span className='detail' id='web-login-hint'>{loginView.hint}</span>}>
          <Button id='web-login-button' disabled={loginView.disabled} onClick={() => controllerRef.current?.start()}>
            {loginView.waiting && <Spinner />}{loginView.label}
          </Button>
          <Button
            id='web-login-cancel'
            variant='outline'
            hidden={!loginView.waiting}
            onClick={() => controllerRef.current?.cancel()}
          >
            {t('取消等待')}
          </Button>
        </AddFoot>
      </AddGroup>

      <WorkBuddyFileImportSection edition={edition} visible={active && effectiveMethod === 'file'} footerHost={footerHost} />
      <WorkBuddyDesktopImportSection edition={edition} visible={active && effectiveMethod === 'desktop'} footerHost={footerHost} />
    </div>
  )
}
