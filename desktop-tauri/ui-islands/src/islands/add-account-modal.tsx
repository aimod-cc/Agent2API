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
 * 内建，嵌套弹窗的 Esc 只收最上面那层（旧实现靠 document 上的 keydown +
 * closeFormStep 手工分流）。**一处行为差异**：关掉表单层时旧实现只是把它的 DOM
 * 藏起来（表单内容与登录引擎的闭包都还在），这里会随之卸载 —— 输入框内容由
 * 模块级草稿（add-account-bridge 的 draftProps）保住，手机验证码那一步的
 * deviceId 会丢（用户回到同一家重填验证码即可，链路上的服务端行为不变）。
 *
 * ── 对外接口（调用点逐个 grep 确认过，见最终报告）────────────
 *   window.wbAddAccountModal  = { open, close }   —— 跨模块打开这张弹窗的唯一入口
 *   window.wbAccountAddForms  = { syncAddProvider, openNewCustomForm }
 *     · syncAddProvider   账号页（account-panel.js）在「添加账号」按钮上同步调用
 *     · openNewCustomForm 模型管理页左栏的「＋ 新建自定义提供商」直达新建表单
 * 旧接口里其余成员（closeFormStep / registerAddForm / bindSeg / segValueOf /
 * setSegValue / SEG_EVENT）只被本子系统内部用，随旧文件一起消失。
 */

import * as React from 'react'
import { createRoot } from 'react-dom/client'
import { Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle } from '@ui'

import { shared, type CustomProviderRecord, type SharedWindow } from './add-account-bridge'
import {
  BUILTIN_CONFIGS,
  CUSTOM_PROVIDER,
  WORKBUDDY_ENTRY_LABEL,
  WORKBUDDY_PROVIDER,
  configOf,
} from './add-account-configs'
import { CustomFootActions, CustomProviderBlock, type CustomMode } from './add-custom-provider'
import { ImportFootActions, ImportPanel } from './add-provider-import'
import {
  IMPORT_SEGMENT_ENABLED,
  NEW_PROVIDER_CARD_ID,
  PRESET_CARD_PREFIX,
  PROVIDER_ICONS,
  PickStep,
  TYPE_ALL,
  TYPE_IMPORT,
  type AccountType,
} from './add-provider-pick'
import { ProviderBlock, WorkBuddyBlock } from './add-provider-blocks'
import { DynamicProviderBlock } from './add-dynamic-provider'
import { DeepseekAccountBlock } from './add-provider-deepseek-block'
import '../styles/add-account-dialog.css'
import { t } from '../i18n'

/**
 * 表单层头部那枚提供商标识：收录过图标的家出真图标，其余用首字母徽章
 * （与第 1 步卡片同一份图标映射，别处不要照抄）。
 */
function ProviderMark({ provider, label }: { provider: string; label: string }): React.ReactElement {
  const icon = PROVIDER_ICONS[provider]
  if (icon) return <span className='acct-add-head-mark'><img src={icon} alt='' /></span>
  const initial = String(label || '?').trim().slice(0, 1).toUpperCase() || '?'
  return <span className='acct-add-head-mark' aria-hidden='true'>{initial}</span>
}

type Step = 'pick' | 'form'

type IslandState = {
  step: Step
  accountType: AccountType
  search: string
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
  accountType: TYPE_ALL,
  search: '',
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
}

const customList = (): CustomProviderRecord[] => shared().wbProviders?.customList?.() || []

function AddAccountModal({
  onClose,
  onReady,
}: {
  onClose: () => void
  onReady: (handle: IslandHandle | null) => void
}): React.ReactElement {
  const [state, setState] = React.useState<IslandState>(INITIAL)
  /** 摘要 / 自定义目录异步到位后的重画信号（值本身不参与渲染） */
  const [version, setVersion] = React.useState(0)
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
  const bump = React.useCallback(() => setVersion(value => value + 1), [])

  const reset = React.useCallback(() => {
    setState(INITIAL)
    const providers = shared().wbProviders
    // 摘要还没到（首次打开弹窗早于首屏那次 refresh）时补拉一次再重画：
    // 否则卡片上会清一色写「还没有账号」，而账号其实早就有了
    if (!(providers?.all?.() || []).length) void providers?.load?.().then(bump)
    // 自定义目录同理：切到「自定义」段时已建的家要显示出来
    void providers?.refreshCustom?.().then(bump)
  }, [bump])

  const openNewCustomForm = React.useCallback(() => {
    reset()
    setState({ ...INITIAL, step: 'form', provider: CUSTOM_PROVIDER })
  }, [reset])

  React.useEffect(() => {
    onReady({ reset, openNewCustomForm })
    return () => onReady(null)
  }, [onReady, reset, openNewCustomForm])

  // 打开时以主进程真实状态复位按钮（旧 openModal 里那一次 refresh）：
  // 上次若在等待中被关窗，这里会重新可用
  React.useEffect(() => {
    void shared().wbWebLogin?.refresh?.()
  }, [])

  // 自定义目录提前拉一次：第一次打开这一屏时下拉就有数据（旧实现加载期就拉）
  React.useEffect(() => {
    void shared().wbProviders?.refreshCustom?.().then(bump)
  }, [bump])

  // 每次进入自定义块都重读目录（旧 onShow 里那次 refreshCustom）
  React.useEffect(() => {
    if (state.step !== 'form' || state.provider !== CUSTOM_PROVIDER) return
    void shared().wbProviders?.refreshCustom?.().then(bump)
  }, [state.step, state.provider, state.showToken, bump])

  const config = configOf(state.provider)
  const isCustom = state.provider === CUSTOM_PROVIDER
  const isDeepseek = state.provider === 'deepseek-web'
  const hasBlock = state.provider === WORKBUDDY_PROVIDER || isCustom || isDeepseek || Boolean(config)
  const customMode: CustomMode = state.providerHint ? 'existing' : 'create'
  const customLabel = state.providerHint
    ? customList().find(item => item.id === state.providerHint)?.name || ''
    : state.presetKey
      ? shared().wbPresetProviders?.presetOf?.(state.presetKey)?.name || ''
      : ''
  // WorkBuddy 用中性品牌名（不带地区）：这个块内部有「账号版本」分段，标题写着
  // 「国内版」而用户当场切到国际版就自相矛盾了（与第 1 步那张卡同一口径，名字
  // 见 add-account-configs 的 WORKBUDDY_ENTRY_LABEL）。别家用注册表名，
  // 注册名要过一遍词典（「AutoClaw 国内版」这类在非中文界面得翻）。
  const providerLabel = state.provider === WORKBUDDY_PROVIDER
    ? WORKBUDDY_ENTRY_LABEL
    : t(shared().wbProviders?.labelOf?.(state.provider) || state.provider)
  // 标题写在表单弹窗的头部：列表弹窗的标题始终是「添加账号」，不跟着步骤变
  const heading = isCustom
    ? (state.providerHint
      ? t('登录 / 添加 {name} 账号', { name: customLabel || t('自定义提供商') })
      : state.presetKey
        ? t('使用 {name} 模板新建提供商', { name: customLabel })
        : t('新建自定义提供商'))
    : isDeepseek ? t('添加 DeepSeek 账号') : t('登录 / 添加 {name} 账号', { name: providerLabel })

  /** 第 1 步点一张卡：三种特殊取值分别落到自定义块的哪种模式（见 pickProvider 的旧注释） */
  function pick(id: string): void {
    if (!id) return
    const entry = { step: 'form' as Step, showToken: state.showToken + 1 }
    if (id === NEW_PROVIDER_CARD_ID) {
      setState(prev => ({ ...prev, ...entry, provider: CUSTOM_PROVIDER, providerHint: '', presetKey: '' }))
    } else if (id.startsWith(PRESET_CARD_PREFIX)) {
      const presetKey = id.slice(PRESET_CARD_PREFIX.length)
      setState(prev => ({
        ...prev,
        ...entry,
        provider: CUSTOM_PROVIDER,
        providerHint: '',
        presetKey,
      }))
    } else if (customList().some(item => item.id === id)) {
      setState(prev => ({ ...prev, ...entry, provider: CUSTOM_PROVIDER, providerHint: id, presetKey: '' }))
    } else {
      setState(prev => ({ ...prev, ...entry, provider: id, providerHint: '', presetKey: '' }))
    }
  }

  const backToPick = (): void => setState(prev => ({ ...prev, step: 'pick' }))
  const importing = state.accountType === TYPE_IMPORT

  return (
    // 第 1 步：选提供商。受控 open（恒为 true）：本岛是「打开时建、关闭即卸」，
    // 关窗一律由 onClose 收口（Esc / 点遮罩 / ✕ 都由 Base UI 汇到 onOpenChange）。
    //
    // 第 2 步（表单层）写在 **DialogContent 的 children 里**（见下面那段 Dialog）。
    <Dialog open onOpenChange={next => { if (!next) onClose() }}>
      <DialogContent className='acct-add-dialog w-[min(1100px,calc(100vw-48px))]'>
        <DialogHeader>
          <DialogTitle>{t('添加账号')}</DialogTitle>
        </DialogHeader>
        <DialogBody>
          <PickStep
            accountType={state.accountType}
            onAccountTypeChange={value => setState(prev => ({ ...prev, accountType: value }))}
            search={state.search}
            onSearchChange={value => setState(prev => ({ ...prev, search: value }))}
            onPick={pick}
            version={version}
            importing={importing}
            importPanel={
              IMPORT_SEGMENT_ENABLED ? <ImportPanel segmentOn={importing} /> : undefined
            }
          />
        </DialogBody>
        {/* 列表弹窗这条操作条只服务「留在第 1 步完成」的动作（导入段的「导入所选」）；
            第 2 步的主按钮走下面那条，两条各归各的层级 */}
        {IMPORT_SEGMENT_ENABLED && importing ? (
          <DialogFooter>
            <ImportFootActions />
          </DialogFooter>
        ) : null}

        {/* 第 2 步：选方式 + 填凭证。叠在列表弹窗之上的一层，宽度窄一档
            （表单输入框横跨 800px 只是把字拉散；窄一档顺带强化层叠感）。

            位置是这一层能不能「叠在上面」的关键，两条都要满足：
              · React 父子关系 —— Base UI 据此认「嵌套弹窗」（见 useDialogRoot 的
                parentStore），内层开着时外层的 Esc 与遮罩按压才不生效；
              · 挂在**本 DialogContent 的 children** 里 —— 内层 Dialog 渲染在外层
                Popup 的子树中，它自己的 Portal 才会落进外层 Portal 节点、排在 Popup
                之后（Base UI 的嵌套弹窗就是靠这个先后来分层的）。写成与 DialogContent
                平级时内层 Portal 会挂到 body 上，两层同为 z-30，谁在 DOM 里靠后谁在
                上面 —— 于是表单层被压在提供商列表下面。

            keepMounted：这一层收起时**不卸载** —— 里面的登录引擎把状态放在自己的闭包里
            （短信那一步的 deviceId、发码冷却），节点一卸就等于把「刚发出去的验证码绑在
            哪个 deviceId 上」丢掉，用户「发码 → 退回列表 → 再进来填码」会拿到作废的 id。
            隐藏时 Base UI 会把内容标记为 inert，键盘与读屏不会跑进去。

            overlayForceRender：这一层要有自己的遮罩（压暗下面的列表层 + 点空白处
            退回列表，与旧实现 #add-form-modal 那层遮罩同义）。Base UI 对嵌套弹窗默认
            不画子层遮罩，遮罩同时是「点空白处」的落点（useDialogRoot 的 outsidePress
            认遮罩本身），所以这一档不能省。 */}
        <Dialog open={state.step === 'form'} onOpenChange={next => { if (!next) backToPick() }}>
          <DialogContent keepMounted overlayForceRender className={isDeepseek
            ? 'compact-login-dialog acct-add-form-dialog w-[min(540px,calc(100vw-48px))]'
            : 'acct-add-form-dialog w-[min(620px,calc(100vw-48px))]'}>
            <DialogHeader>
              <ProviderMark provider={state.provider} label={providerLabel} />
              <DialogTitle>{heading}</DialogTitle>
            </DialogHeader>
            <DialogBody>
              {/* 各家的块整块常驻、只切显隐：块内的登录引擎把状态放在自己的闭包里，
                  节点被卸载就等于把「刚发出去的验证码绑在哪个 deviceId 上」丢掉 */}
              <WorkBuddyBlock active={state.provider === WORKBUDDY_PROVIDER} footerHost={formFootHost} />
              {BUILTIN_CONFIGS.filter(item => item.provider !== 'deepseek-web').map(item => (
                <ProviderBlock
                  key={item.provider}
                  config={item}
                  active={state.provider === item.provider}
                  footerHost={formFootHost}
                />
              ))}
              <div
                className='add-provider-block'
                style={isCustom ? undefined : { display: 'none' }}
              >
                <CustomProviderBlock
                  mode={customMode}
                  providerHint={state.providerHint}
                  presetKey={state.presetKey}
                  showToken={state.showToken}
                  version={version}
                />
              </div>
              <DeepseekAccountBlock active={isDeepseek && state.step === 'form'}
                footerHost={formFootHost} onClose={backToPick} />
              {hasBlock ? null : (
                <DynamicProviderBlock
                  key={state.provider}
                  provider={state.provider}
                  label={providerLabel}
                  active={state.step === 'form'}
                  version={version}
                  footerHost={formFootHost}
                />
              )}
            </DialogBody>
            {/* 这一层统一一条底栏：各家段落把主按钮与说明 portal 进来（见 add-account-kit
                的 AddFoot），自定义块本来就把按钮渲染在这里。宿主为空时整条收起 ——
                判据是样式表里的 `:has()`，不需要额外状态。 */}
            <DialogFooter className='acct-add-foot'>
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
        </Dialog>
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
}

function mountModal(): void {
  host = document.createElement('div')
  document.body.append(host)
  root = createRoot(host)
  root.render(
    <AddAccountModal
      onClose={closeModal}
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
  pendingAction = () => handle?.reset()
  mountModal()
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
bridgeWindow.wbAddAccountModal = { open: openModal, close: closeModal }
bridgeWindow.wbAccountAddForms = {
  /** 账号页的「添加账号」按钮：按 providers 摘要重建卡片并复位到 WorkBuddy */
  syncAddProvider: () => {
    if (root) handle?.reset()
    else openModal()
  },
  openNewCustomForm,
}