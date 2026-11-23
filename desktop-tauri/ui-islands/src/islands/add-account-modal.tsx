/**
 * Agent2API · 「登录 / 添加账号」弹窗（React 岛，命令式弹窗形态）。
 *
 * 替换旧的四份脚本：ui/add-account.js（弹窗外壳 + WorkBuddy 那段）、
 * ui/add-provider-forms.js（两步结构 + 各家表单块 + 分段交互）、
 * ui/add-provider-import.js（导入段）、ui/add-custom-provider.js（自定义块）
 * 以及四份纯常量的表单配置（add-qoder / add-cline / add-accio / add-zcode）。
 *
 * ── 挂载形态：与面板岛不同 ──────────────────────────────────
 * 这个弹窗不是页面，是命令式弹窗（按需创建 / 关闭即卸）：open() 建宿主 div 挂
 * React root，close() 时 unmount + 摘宿主（与 confirm-dialog.tsx / conc-dialog.tsx
 * 同一手法）。index.html 里那段 47 行的静态骨架（#add-modal）随之删除。
 *
 * ── 两层弹窗（与旧实现的对应关系）──────────────────────────
 *   第 1 步 #add-modal       → 这里的列表 Dialog（选提供商），宽 1100px；
 *   第 2 步 #add-form-modal  → 这里的表单 Dialog（选方式 + 填凭证），宽 620px。
 * 两层都用组件库的 Dialog：Esc / 点遮罩 / 焦点陷阱 / 关闭后焦点归位由 Base UI
 * 内建。表单首次选中时才挂载，同一次弹窗内返回列表只隐藏，保留验证码与登录状态；
 * 关闭整个弹窗时统一卸载并清理登录控制器。输入草稿由 add-account-bridge 保留。
 *
 * ── 对外接口（调用点逐个 grep 确认过，见最终报告）────────────
 *   window.wbAddAccountModal  = { open, openProvider, close } —— 跨模块打开弹窗或指定提供商
 *   window.wbAccountAddForms  = { syncAddProvider, openNewCustomForm }
 *     · syncAddProvider   账号页（account-panel.js）在「添加账号」按钮上同步调用
 *     · openNewCustomForm 模型管理页左栏的「＋ 新建自定义提供商」直达新建表单
 * 旧接口里其余成员（closeFormStep / registerAddForm / bindSeg / segValueOf /
 * setSegValue / SEG_EVENT）只被本子系统内部用，随旧文件一起消失。
 */

import * as React from 'react'
import { createRoot } from 'react-dom/client'
import {
  Badge, Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle,
  NavItem, NavList, Sidebar,
} from '@ui'

import { shared, accountProviderAvailable, toast, type CustomProviderRecord, type SharedWindow } from './add-account-bridge'
import { CUSTOM_PROVIDER, WORKBUDDY_PROVIDER } from './add-account-constants'
import { CustomFootActions, type CustomMode } from './add-custom-provider'
import { NEW_PROVIDER_CARD_ID, PRESET_CARD_PREFIX } from './add-provider-pick'
import { AddAccountPicker } from './add-account-picker'
import { AddAccountForms, accountFormKey, type VisitedAccountForm } from './add-account-forms'
import { useAddAccountCatalog } from './add-account-catalog'
import { useProviderIcon } from './provider-icon'
import { AddAccountProxyProvider, useAddAccountProxy } from './add-account-proxy'
import {
  AddAccountForwardingPane, AddAccountLimiterPane, AddAccountNetworkPane, AddAccountQueryPane,
  AddAccountSettingsProvider, useAddAccountSettings, type AddPaneId,
} from './add-account-settings'
import { AccountSettingsIcon } from './accounts-settings-fields'
import { providerFeatures } from './accounts-features'
import { isOpencodeZenProvider } from './accounts-domain'
import { customUserAgentEditable, userAgentEditable } from './add-account-user-agent'
import { managedCaptchaBusy, subscribeManagedCaptchaBusy } from '../../../ui-shared/plugin-login-managed'
import '../styles/add-account-dialog.css'
import { t } from '../i18n'

/**
 * 表单层头部那枚提供商标识：收录过图标的家出真图标，其余用首字母徽章
 * （与第 1 步卡片同一份图标映射，别处不要照抄）。
 */
function ProviderMark({ provider, label }: { provider: string; label: string }): React.ReactElement {
  const { src: icon, onError } = useProviderIcon(provider)
  if (icon) return <span className='acct-add-head-mark'><img src={icon} alt='' onError={onError} /></span>
  const initial = String(label || '?').trim().slice(0, 1).toUpperCase() || '?'
  return <span className='acct-add-head-mark' aria-hidden='true'>{initial}</span>
}

type Step = 'pick' | 'form'

type IslandState = {
  step: Step
  direct: boolean
  pickerRevision: number
  forms: VisitedAccountForm[]
  /** 当前选中哪一家的块（provider id / 'workbuddy' / 'custom'） */
  provider: string
  /** 从某一家已有自定义提供商的卡片进来时的 id（空串 = 没有指定） */
  providerHint: string
  /** 从预置家卡片进来时的 key（空串 = 不是从预置卡进来的） */
  presetKey: string
  /** 第 1 步每点一次卡 +1：预填 / 目录重读这类「进这一屏就要跑一次」的动作用它当信号 */
  showToken: number
}

/**
 * 打开弹窗（以及点「添加账号」）时的复位落点：第 1 步 + **「全部」段** + WorkBuddy。
 * 落「全部」而不是 Agent：搜索框只过滤当前分段，默认停在 Agent 时，找一家在
 * 别段里的（Cline Pass、某家的自定义中转）会先看到「没有匹配」—— 进来就该看见全部。
 * 单分类段照样在分段栏里，知道找哪一类时那是更短的路。
 */
const INITIAL: IslandState = {
  step: 'pick',
  direct: false,
  pickerRevision: 0,
  forms: [],
  provider: WORKBUDDY_PROVIDER,
  providerHint: '',
  presetKey: '',
  showToken: 0,
}

/** 命令式外壳暴露给模块级函数的那几个动作（组件挂载后登记，卸载时撤销） */
export type IslandHandle = {
  /** 重画卡片列表并复位到第 1 步（旧 resetAddStep，对外叫 syncAddProvider） */
  reset: () => void
  /** 直达「新建自定义提供商」表单（旧 openNewCustomForm） */
  openNewCustomForm: () => void
  openProvider: (provider: string) => void
}

const customList = (): CustomProviderRecord[] => shared().wbProviders?.customList?.() || []

function accountEntryAvailable(id: string): boolean {
  return id === NEW_PROVIDER_CARD_ID || id.startsWith(PRESET_CARD_PREFIX) || id === CUSTOM_PROVIDER
    || customList().some(item => item.id === id) || accountProviderAvailable(id)
}

function requireAccountEntry(id: string): boolean {
  if (accountEntryAvailable(id)) return true
  toast(t('该提供商当前不可用，请安装并启用对应插件后重试'), 'warn')
  return false
}

function providerFormState(id: string, previous: IslandState, direct = false): IslandState {
  const next: IslandState = { ...previous, step: 'form', direct, showToken: previous.showToken + 1,
    provider: id, providerHint: '', presetKey: '' }
  if (id === NEW_PROVIDER_CARD_ID) next.provider = CUSTOM_PROVIDER
  else if (id.startsWith(PRESET_CARD_PREFIX)) {
    next.provider = CUSTOM_PROVIDER
    next.presetKey = id.slice(PRESET_CARD_PREFIX.length)
  } else if (id.startsWith('custom-') || customList().some(item => item.id === id)) {
    next.provider = CUSTOM_PROVIDER
    next.providerHint = id
  }
  const key = accountFormKey(next.provider)
  const form: VisitedAccountForm = { key, provider: next.provider, direct, providerHint: next.providerHint,
    presetKey: next.presetKey, showToken: next.showToken }
  if (!previous.forms.some(item => item.key === key)) next.forms = [...previous.forms, form]
  else if (next.provider === CUSTOM_PROVIDER || direct) {
    next.forms = previous.forms.map(item => item.key === key ? form : item)
  }
  return next
}

function AddAccountFormContent({
  state,
  version,
  availability,
  formFootHost,
  backToPick,
}: {
  state: IslandState
  version: number
  availability: string
  formFootHost: HTMLElement
  backToPick: () => void
}): React.ReactElement {
  const proxy = useAddAccountProxy()
  const settings = useAddAccountSettings()
  const paneScroll = React.useRef<HTMLDivElement | null>(null)
  const panePositions = React.useRef<Partial<Record<AddPaneId, number>>>({})

  const isCustom = state.provider === CUSTOM_PROVIDER
  const customMode: CustomMode = state.providerHint ? 'existing' : 'create'
  const pickedProvider = isCustom && state.providerHint
    ? customList().find(item => item.id === state.providerHint)
    : undefined
  const createPreset = isCustom && !state.providerHint && state.presetKey
    ? shared().wbPresetProviders?.presetOf?.(state.presetKey)
    : null
  const customLabel = state.providerHint ? (pickedProvider?.name || '') : (createPreset?.name || '')
  const providerLabel = t(shared().wbProviders?.labelOf?.(state.provider) || state.provider)
  const heading = isCustom
    ? (state.providerHint
      ? t('添加 {name} 账号', { name: customLabel || t('自定义提供商') })
      : state.presetKey
        ? t('使用 {name} 模板新建提供商', { name: customLabel })
        : t('新建自定义提供商'))
    : state.provider === 'deepseek-web' ? t('添加 DeepSeek 账号') : t('添加 {name} 账号', { name: providerLabel })

  const balanceScheme = isCustom
    ? String(pickedProvider?.balanceScheme || createPreset?.balance?.scheme || '')
    : ''
  const supportsUsage = isCustom ? Boolean(balanceScheme) : providerFeatures(state.provider).usage
  const uaEditable = isCustom
    ? (customMode === 'existing' ? customUserAgentEditable(pickedProvider) : customUserAgentEditable(createPreset))
    : userAgentEditable(state.provider)
  const allowRotate = isCustom
    ? isOpencodeZenProvider(
      pickedProvider || (createPreset
        ? { clientEmulation: createPreset.clientEmulation, baseUrl: createPreset.baseUrl }
        : undefined),
      createPreset?.baseUrl || pickedProvider?.baseUrl,
    )
    : false
  const concDefault = isCustom ? 0 : (providerFeatures(state.provider).concurrencyDefault || 0)
  const effectiveProvider = state.providerHint || state.provider

  const panes: AddPaneId[] = supportsUsage
    ? ['basic', 'forwarding', 'query', 'limiter', 'network']
    : ['basic', 'forwarding', 'limiter', 'network']

  const paneInfo: Record<AddPaneId, { title: string; description: string; icon: string }> = {
    basic: {
      title: isCustom ? t('连接与凭证') : t('登录与凭证'),
      description: isCustom
        ? (customMode === 'create' ? t('按分组填写连接参数与账号凭证，右侧可实时预览完整请求地址。') : t('查看所属站点信息，填写当前账号的 API Key 与备注名。'))
        : t('选择登录方式并填写对应凭证。'),
      icon: 'key',
    },
    forwarding: {
      title: t('转发行为'),
      description: t('预设账号启用状态、失败换号策略、并发上限及自定义 User-Agent。'),
      icon: 'sliders',
    },
    query: {
      title: t('查询设置'),
      description: t('预设自动余额刷新间隔、失败重试次数及查询凭证。'),
      icon: 'refresh',
    },
    limiter: {
      title: t('限制器'),
      description: t('创建时即可配置余额或本地用量保护规则。'),
      icon: 'shield',
    },
    network: {
      title: t('网络'),
      description: t('设置当前账号访问上游时使用的出网代理。'),
      icon: 'proxies',
    },
  }

  const activePane: AddPaneId = panes.includes(settings.pane) ? settings.pane : 'basic'

  // 每次在第 1 步点卡片重新打开表单时，回到首屏（连接与凭证）
  React.useEffect(() => {
    settings.setPane('basic')
    panePositions.current = {}
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state.showToken])

  React.useLayoutEffect(() => {
    if (paneScroll.current) paneScroll.current.scrollTop = panePositions.current[activePane] || 0
  }, [activePane])

  function selectPane(next: AddPaneId): void {
    if (proxy.busy) return
    if (paneScroll.current) panePositions.current[activePane] = paneScroll.current.scrollTop
    settings.setPane(next)
  }

  return (
    <DialogContent keepMounted overlayForceRender className='acct-settings-dialog acct-add-form-dialog'>
      <DialogHeader className='acct-dlg-header'>
        <ProviderMark provider={state.provider} label={customLabel || providerLabel} />
        <div className='acct-dlg-identity'>
          <div className='acct-dlg-title-row'>
            <DialogTitle>{heading}</DialogTitle>
            {isCustom ? <Badge variant='secondary' shape='tag'>{t('自定义 API')}</Badge> : null}
          </div>
          <p>
            <span>{isCustom ? (customLabel || t('自定义提供商')) : providerLabel}</span>
            <span className='acct-dlg-title-dot'>·</span>
            <span>{isCustom && customMode === 'create' ? t('新建提供商并添加账号') : t('新建账号配置')}</span>
          </p>
        </div>
      </DialogHeader>
      <DialogBody className='acct-dlg-body'>
        <div className='acct-dlg'>
          <Sidebar className='acct-dlg-rail' aria-label={t('添加账号分组')}>
            <NavList className='acct-dlg-nav'>
              <div className='acct-dlg-nav-label'>{t('添加设置')}</div>
              {panes.map(id => (
                <NavItem
                  key={id}
                  active={activePane === id}
                  title={paneInfo[id].title}
                  disabled={proxy.busy && activePane !== id}
                  className='shadow-none'
                  icon={<AccountSettingsIcon name={paneInfo[id].icon} />}
                  trailing={settings.dirtyPanes[id]
                    ? <span className='acct-dlg-dirty-dot' aria-label={t('已调整设置')} />
                    : undefined}
                  onClick={() => selectPane(id)}
                >
                  {paneInfo[id].title}
                </NavItem>
              ))}
            </NavList>
            <div className='acct-dlg-rail-context'>
              <strong>
                <AccountSettingsIcon name={isCustom ? 'agent' : 'accounts'} size={14} />
                {isCustom ? (customLabel || t('自定义 API')) : providerLabel}
              </strong>
              <p>{isCustom && customMode === 'create' ? t('创建后会继续提示获取模型') : t('所有设置会在保存时一并生效')}</p>
            </div>
          </Sidebar>
          <div className='acct-dlg-main'>
            <div className='acct-dlg-pane-head'>
              <div>
                <h3>{paneInfo[activePane].title}</h3>
                <p>{paneInfo[activePane].description}</p>
              </div>
              <span className='acct-dlg-scope'>
                <AccountSettingsIcon name='sliders' size={13} />
                {activePane === 'basic' ? t('必填项与凭证') : t('账号初始设置')}
              </span>
            </div>
            <div ref={paneScroll} className='acct-dlg-scroll' aria-busy={proxy.busy}>
              <div className='acct-dlg-pane'>
                {/* 首栏常驻 DOM（只用 hidden 切换）：保证登录引擎事件与草稿 ID 不因切栏丢失 */}
                <div hidden={activePane !== 'basic'}>
                  <AddAccountForms
                    forms={state.forms}
                    provider={state.provider}
                    open={state.step === 'form'}
                    version={version}
                    availability={availability}
                    footerHost={formFootHost}
                  />
                </div>
                {activePane === 'forwarding' ? (
                  <AddAccountForwardingPane concDefault={concDefault} uaEditable={uaEditable} busy={proxy.busy} />
                ) : null}
                {activePane === 'query' && supportsUsage ? (
                  <AddAccountQueryPane supportsUsage={supportsUsage} balanceScheme={balanceScheme} busy={proxy.busy} />
                ) : null}
                {activePane === 'limiter' ? (
                  <AddAccountLimiterPane provider={effectiveProvider} allowBalance={supportsUsage} busy={proxy.busy} />
                ) : null}
                {activePane === 'network' ? (
                  <AddAccountNetworkPane idPrefix='add-account-proxy' busy={proxy.busy} allowRotate={allowRotate} />
                ) : null}
              </div>
            </div>
          </div>
        </div>
      </DialogBody>
      {/* 表单动作始终挂在固定底栏宿主，在任何分栏下都可直接提交或返回 */}
      <DialogFooter className='acct-add-foot'>
        <Button variant='ghost' className='acct-add-back' onClick={backToPick}>
          <svg width='16' height='16' viewBox='0 0 24 24' fill='none' stroke='currentColor' strokeWidth='1.8' aria-hidden='true'><path d='m15 5-7 7 7 7' /></svg>
          {state.direct ? t('取消') : t('更换提供商')}
        </Button>
        {isCustom ? (
          <div className='acct-add-foot-host'>
            <CustomFootActions mode={customMode} presetKey={state.presetKey} providerId={state.providerHint} />
          </div>
        ) : (
          <div
            className='acct-add-foot-mount'
            ref={node => {
              if (node && formFootHost.parentElement !== node) node.append(formFootHost)
            }}
          />
        )}
      </DialogFooter>
    </DialogContent>
  )
}

function AddAccountModal({
  onClose,
  onReady,
  initialProvider,
}: {
  onClose: () => void
  onReady: (handle: IslandHandle | null) => void
  initialProvider?: string
}): React.ReactElement {
  const [state, setState] = React.useState<IslandState>(() => initialProvider
    ? providerFormState(initialProvider, INITIAL, true) : INITIAL)
  const { version, availability, refreshCustom } = useAddAccountCatalog()
  const verifying = React.useSyncExternalStore(subscribeManagedCaptchaBusy, managedCaptchaBusy)
  /**
   * 表单层底栏的宿主节点：**在这里 createElement 出来**，而不是 ref + useState。
   * 后者首帧拿到的是 null，段落会先把按钮渲染在原位、下一帧再 portal 进底栏 ——
   * 节点一换，引擎在 `create()` 时绑上的 click 就丢了（sms-login.js /
   * autoclaw-oauth.js 都在 create() 时按 id 绑按钮）。用这个身份稳定的节点，
   * 首帧按钮就已经在底栏里，之后不再搬家。
   */
  const [formFootHost] = React.useState(() => {
    const node = document.createElement('div')
    node.className = 'acct-add-foot-host'
    return node
  })
  const reset = React.useCallback(() => {
    setState(previous => ({ ...previous, step: 'pick', direct: false, pickerRevision: previous.pickerRevision + 1 }))
  }, [])

  const openNewCustomForm = React.useCallback(() => {
    setState(previous => providerFormState(NEW_PROVIDER_CARD_ID, previous))
  }, [])

  const openProvider = React.useCallback((provider: string) => {
    const id = provider.trim()
    if (!id || !requireAccountEntry(id)) return
    setState(previous => providerFormState(id, previous, true))
  }, [])

  React.useEffect(() => {
    onReady({ reset, openNewCustomForm, openProvider })
    return () => onReady(null)
  }, [onReady, reset, openNewCustomForm, openProvider])

  // 打开时以主进程真实状态复位按钮（旧 openModal 里那一次 refresh）：
  // 上次若在等待中被关窗，这里会重新可用
  React.useEffect(() => {
    void shared().wbWebLogin?.refresh?.()
  }, [])

  React.useEffect(() => {
    if (state.step === 'form' && state.provider === CUSTOM_PROVIDER) void refreshCustom()
  }, [state.step, state.provider, state.showToken, refreshCustom])

  /** 第 1 步点一张卡：三种特殊取值分别落到自定义块的哪种模式（见 pickProvider 的旧注释） */
  const pick = React.useCallback((id: string) => {
    if (id && requireAccountEntry(id)) setState(previous => providerFormState(id, previous))
  }, [])

  const backToPick = React.useCallback(() => {
    if (state.direct) onClose()
    else setState(prev => ({ ...prev, step: 'pick' }))
  }, [state.direct, onClose])

  const form = state.forms.length ? (
    // 保留表单节点，避免返回列表后丢失短信验证码与登录引擎状态。
    <Dialog open={state.step === 'form'} modal={!verifying} onOpenChange={(next, details) => {
      if (!next && verifying && ['outside-press', 'focus-out'].includes(details.reason)) { details.cancel(); return }
      if (!next) backToPick()
    }}>
      <AddAccountProxyProvider>
        <AddAccountSettingsProvider contextKey={`${state.provider}:${state.providerHint}:${state.presetKey}`}>
          <AddAccountFormContent
            state={state}
            version={version}
            availability={availability}
            formFootHost={formFootHost}
            backToPick={backToPick}
          />
        </AddAccountSettingsProvider>
      </AddAccountProxyProvider>
    </Dialog>
  ) : null

  if (state.direct && form) return form

  return (
    <Dialog open modal={!verifying} onOpenChange={(next, details) => {
      if (!next && verifying && ['outside-press', 'focus-out'].includes(details.reason)) { details.cancel(); return }
      if (!next) onClose()
    }}>
      <DialogContent className='acct-add-dialog w-[min(1100px,calc(100vw-48px))]'>
        <DialogHeader>
          <DialogTitle>{t('添加账号')}</DialogTitle>
        </DialogHeader>
        <AddAccountPicker key={state.pickerRevision} version={version} onPick={pick} />
        {/* 表单必须位于列表的 DialogContent 内，才能保留嵌套弹窗的遮罩与焦点顺序。 */}
        {form}
      </DialogContent>
    </Dialog>
  )
}

/* ─── 命令式外壳：与旧实现的 window 接口一致 ─── */

let root: ReturnType<typeof createRoot> | null = null
let host: HTMLElement | null = null
let handle: IslandHandle | null = null
/** 挂载后才执行的那一个动作（open / openNewCustomForm 在同一 tick 里被调到时用） */
let pendingAction: (() => void) | null = null

function unmountModal(): void {
  if (root) {
    root.unmount()
    root = null
  }
  if (host) {
    host.remove()
    host = null
  }
  handle = null
  pendingAction = null
}

function mountModal(initialProvider?: string): void {
  host = document.createElement('div')
  document.body.append(host)
  root = createRoot(host)
  root.render(
    <AddAccountModal
      onClose={closeModal}
      initialProvider={initialProvider}
      onReady={next => {
        handle = next
        if (!next) return
        const action = pendingAction
        pendingAction = null
        action?.()
      }}
    />,
  )
}

/**
 * 关闭弹窗：两层一起关（整个岛卸载），并放弃等待中的登录。
 *
 * 关窗即放弃等待：通知主进程中止后端轮询，否则按钮会一直卡在禁用态。
 * 不传 provider = 「谁在等待就取消谁」：多家共用同一个弹窗，这里无需区分。
 * AutoClaw 国际版的 OAuth 另有一套等待（先跑验证码、再等登录窗口），与上面那条
 * 链的取消语义不同（它还要作废本地那次验证码等待），因此单独通知一次 ——
 * 没有发起过时它什么都不做。
 */
function closeModal(): void {
  void shared().wbWebLogin?.cancelIfActive?.('')
  shared().wbAutoclawOauth?.cancel?.()
  unmountModal()
}

/** 打开弹窗（已开着时等价于「重画卡片列表并复位到第 1 步」） */
function openModal(): void {
  if (root) {
    handle?.reset()
    return
  }
  pendingAction = null
  mountModal()
}

function openProvider(provider: string): void {
  const id = provider.trim()
  if (!id || !requireAccountEntry(id)) return
  if (root) {
    if (handle) handle.openProvider(id)
    else pendingAction = () => handle?.openProvider(id)
    return
  }
  pendingAction = null
  mountModal(id)
}

/** 直达「新建自定义提供商」表单：打开弹窗、复位，再走「新建」那张卡的同一路径 */
function openNewCustomForm(): void {
  if (root) {
    handle?.openNewCustomForm()
    return
  }
  pendingAction = () => handle?.openNewCustomForm()
  mountModal()
}

// 账号页的「添加账号」按钮（报表页那个已随会话状态卡片删除，id 保留同一约定）。
// 岛在 index.html 的 islands/ui.js 里加载，位置在那两个按钮之后，DOM 必已解析。
for (const id of ['btn-add-account', 'btn-add-account-2']) {
  document.getElementById(id)?.addEventListener('click', openModal)
}

// 对外接口用「窄类型 + 转型」写回（SharedWindow 里已声明这两个属性），
// 不用 declare global 往 Window 上加属性 —— 并行迁移时同名属性类型不一致会撞 TS2717。
const bridgeWindow = window as unknown as SharedWindow
bridgeWindow.wbAddAccountModal = { open: openModal, openProvider, close: closeModal }
bridgeWindow.wbAccountAddForms = {
  /** 账号页的「添加账号」按钮：按 providers 摘要重建卡片并复位到 WorkBuddy */
  syncAddProvider: () => {
    if (root) handle?.reset()
    else openModal()
  },
  openNewCustomForm,
}