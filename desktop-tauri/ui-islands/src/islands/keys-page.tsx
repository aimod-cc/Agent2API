import * as React from 'react'
import { createRoot } from 'react-dom/client'
import {
  Badge,
  Button,
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogSection,
  DialogTitle,
  Input,
  Label,
  MultiSelect,
  Switch,
  type MultiSelectOption,
} from '@ui'
import { TableFooter, useClientPaging } from './table-shell'
import { t } from '../i18n'

/**
 * Agent2API · 网关 Key 页（列表 / 新建 / 启停 / 删除 / 可用范围）—— React 岛。
 *
 * 替换 ui/keys-panel.js（那份用 innerHTML 拼 .models-table 的行、事件走容器委托）。
 * 对外接口与原实现**完全一致**：`window.wbKeysPanel = { load, render, visibleColumns }`
 * —— app.js:151 切到本页时调 load()，table-columns.js 的列宽层按 visibleColumns()
 * 算「当前可见列」（覆盖值落到哪个 <col>、末列不给把手），调用点一行都不用改。
 *
 * ── 数据口径（照旧，别改）──────────────────────────────────
 * 数据来自 `GET /api/keys`（`{keys, authRequired, providers, modelsByProvider}`，
 * keys 带明文 key 与掩码）。写接口都返回最新列表，就地替换后重绘；列表默认显示掩码，
 * 每行可单独「显示」明文并复制（复制走 clipboard.js 的 data-copy 委托）。
 * 「可用提供商 / 可用模型」两个白名单**空数组 = 不限制**（见后端 core::api_keys）。
 *
 * ── 两个多选走组件库的 MultiSelect ───────────────────────────
 * 「可用提供商 / 可用模型」是**受控**的 React state（不再渲染原生 `<select multiple>`
 * 再让 select.js 增强）：候选、勾选、联动全在这一层算 —— 联动规则见 modelOptions，
 * 提交给后端的取值口径见 pickedFromOptions。
 *
 * ── 弹窗走组件库的 Dialog（与 request-clear-modal / conc-dialog 同一手法）──
 * 旧的 `#key-modal`（.modal-mask 一族）不再使用：Esc / 点遮罩关闭、焦点陷阱、滚动
 * 锁定都由 Dialog 内建。MultiSelect 的浮层是 Base UI 自己的浮层（portal 到 body，
 * z-35 高于弹窗的 z-30），与模态共用同一套 outside-press 判定，不需要额外放行。
 *
 * ── 列设置：为什么在 layout effect 里注册，而不是模块顶层 ──────
 * ① 本岛的模块体比 table-col-settings.tsx 先执行（import.meta.glob 按文件名字典序），
 *    模块顶层那一刻 `window.wbColSettings` 还不存在；
 * ② 「列设置」按钮要插进**本岛渲染出来的** .panel-head .head-actions，而 React 的首次
 *    渲染排在后面的任务里 —— 顶层 querySelector 拿到的是 index.html 的静态骨架
 *    （马上会被 replaceChildren 清掉），按钮会插进一个即将消失的节点。
 * 表头同步跟着注册一起做，并靠 syncStaticHead 内部的 `wbTableColumns.repaint('keys')`
 * 补上把手与列宽：table-columns.js 在本脚本之后加载，谁先跑都有可能 —— 它先跑时找
 * 不到表（当时 React 还没渲染），这次 repaint 补上；这次是空转时，它加载期自己会找。
 * <colgroup> / <thead> 由本文件渲染但 `data-col` 一个不少，React 从不动这几棵静态
 * 子树（虚拟 DOM 不变），所以命令式的重排 / 摘除是安全的。
 */

/* ─── 类型 ─────────────────────────────────── */

/** 一把 Key（`GET /api/keys` 的 keys[]，对应后端 `api_keys::ApiKeyEntry::public_json`） */
type KeyEntry = {
  id: string
  name?: string
  /** 明文：本机管理界面要能随时复制给客户端（掩码只用于列表折叠展示） */
  key?: string
  masked?: string
  enabled?: boolean
  createdAt?: number
  /** 白名单，**空数组 = 不限制** */
  allowedProviders?: string[]
  allowedModels?: string[]
}

/**
 * 「可用提供商」的候选项：后端拼好的整张表 —— 注册表（内置家）+ 已建的自定义家
 * （项目禁止维护第二份 provider 清单；自定义家是运行期数据，只有后端拿得到）。
 */
type ProviderOption = { id: string; label?: string }

/** 四个接口的响应；`created` 只在 POST 的响应里（新建后要立刻展开它） */
type KeysPayload = {
  keys?: KeyEntry[]
  authRequired?: boolean
  providers?: ProviderOption[]
  /** 每家 → 对外名清单，模型候选的**唯一**数据源（按当前勾选的提供商取并集） */
  modelsByProvider?: Record<string, string[]>
  created?: KeyEntry
}

/** 本岛用到的后端桥（见 bridge.rs 的「网关 Key」那一段） */
type KeysBridge = {
  getKeys(): Promise<KeysPayload | null | undefined>
  createKey(payload: {
    name: string
    /** 留空 = 后端自动生成 */
    key?: string
    allowedProviders: string[]
    allowedModels: string[]
  }): Promise<KeysPayload | null | undefined>
  updateKey(
    id: string,
    patch: { enabled?: boolean; allowedProviders?: string[]; allowedModels?: string[] },
  ): Promise<KeysPayload | null | undefined>
  deleteKey(id: string): Promise<KeysPayload | null | undefined>
}

/**
 * window 上由其它脚本 / 其它岛挂载的共享桥。
 *
 * 刻意用「局部窄类型 + 转型」而不是 declare global 往 Window 上加属性：
 * workbuddyDesktop / wbApp / wbColSettings / wbProviders 是多个岛共用的桥，若每个岛
 * 各 declare 一份，接口合并会因同名属性类型不一致直接报 TS2717。本文件只 declare
 * 自己独占的 wbKeysPanel（见文件末尾）。
 */
type SharedWindow = {
  workbuddyDesktop?: KeysBridge
  wbApp?: {
    toast?: (message: string, kind?: 'err' | 'ok') => void
    /** 时间戳 → 本地时间串（app.js 的 formatTime；createdAt 列用它） */
    formatTime?: (value: unknown) => string
    /** 顶栏状态区重画：本页徽标是顶栏那枚的镜像（按 id 读文案与 data-tone） */
    renderTopbarStatus?: () => void
    /** 当前页标识：首屏自持加载只在用户正看着本页时打后端 */
    readonly currentPage?: string
  }
  wbConfirm?: {
    ask?: (options: {
      title?: string
      /** 正文，允许 <strong> 等少量标记；内容由调用方负责转义 */
      html?: string
      okText?: string
      /** danger = 不可恢复的危险操作（确认键走红） */
      okClass?: string
    }) => Promise<boolean>
  }
  wbColSettings?: {
    register(spec: {
      id: string
      label?: string
      columns: readonly ColumnDecl[]
      mount?: () => Element | null
      onChange?: () => void
    }): ColSettingsHandle
    syncStaticHead(id: string, table: Element | null | undefined): void
  }
  /** 提供商显示名（注册表 + 自定义家的查找链，注册表里没有的 id 回落原样） */
  wbProviders?: { labelOf?: (id: string) => string | undefined }
}

function shared(): SharedWindow {
  return window as unknown as SharedWindow
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

/** toast 的统一出口（运行期读 wbApp，不在模块顶层解构） */
function toast(message: string, kind?: 'err' | 'ok'): void {
  shared().wbApp?.toast?.(message, kind)
}

/** 确认框正文是 HTML 串，插值一律先转义（不借 wbApp.esc：那是 app.js 的私有函数） */
function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/g, ch => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  }[ch] ?? ch))
}

/* ─── 常量与列声明 ───────────────────────────── */

type Align = 'left' | 'center' | 'right'

/** 列声明：key 与 index.html 里既有的 `data-col`、<col> 的 data-col 三处同名 */
type ColumnDecl = { key: string; label: string; align?: Align }

const COLUMNS: readonly ColumnDecl[] = [
  { key: 'name', label: t('名称') },
  // 「Key」是产品固定叫法（无中文），不进词典
  { key: 'key', label: 'Key' },
  { key: 'time', label: t('创建时间') },
  { key: 'state', label: t('启用') },
  { key: 'act', label: t('操作'), align: 'right' },
]

/** 每个单元格自己的类名（`state` / `r` 是既有 CSS 的钩子，见 page-gateway.css） */
const CELL_CLASS: Record<string, string> = {
  name: 'cell-name',
  key: 'cell-key',
  time: 'cell-time',
  state: 'cell-state state',
  act: 'cell-act r',
}

/** 页面区块（React root 直接建在它上面，见文件头） */
const SECTION = '.page[data-page="keys"]'

/* ─── 列设置：能力层（register / apply / syncStaticHead）─── */

type ColSettingsHandle = {
  apply<C extends { key: string }>(columns: C[]): (C & { align: Align })[]
  config(): unknown
}

let colSettings: ColSettingsHandle | null = null

/** 列设置改动后的重画入口：组件挂载后登记（onChange 从 React 之外回调进来） */
let onColumnsChanged: (() => void) | null = null

/**
 * 该表当前可见的列（顺序即配置顺序；列设置未就绪时退回全部列）。
 * 导出给 table-columns.js：列宽那一层要按当前可见列算（覆盖值落到哪个 <col>、
 * 末列不给把手），两边读同一份配置才不会各算一个样。
 */
function visibleColumns(): (ColumnDecl & { align: Align })[] {
  if (colSettings) return colSettings.apply([...COLUMNS])
  // 列设置没就绪（脚本加载失败等）：退回声明顺序，对齐取列上声明的默认值
  return COLUMNS.map(column => ({ ...column, align: column.align ?? 'left' }))
}

/** 静态表头就地重排：顺序 / 显隐 / 对齐（末尾顺带让列宽层重对一遍把手） */
function syncHead(): void {
  shared().wbColSettings?.syncStaticHead?.('keys', document.querySelector('table.keys-table'))
}

/** 登记列设置（只做一次）并同步表头；必须在 React 提交之后调用（见文件头） */
function setupColumns(): void {
  if (colSettings) return
  const handle = shared().wbColSettings?.register({
    id: 'keys',
    label: t('网关 Key 表'),
    columns: COLUMNS,
    mount: () => document.querySelector('.page[data-page="keys"] .panel-head .head-actions'),
    onChange: () => {
      syncHead()
      onColumnsChanged?.()
    },
  })
  if (!handle) return
  colSettings = handle
  syncHead()
}

/* ─── 纯函数：文案与候选项 ───────────────────── */

function keysOf(payload: KeysPayload | null): KeyEntry[] {
  return Array.isArray(payload?.keys) ? payload.keys : []
}

/** 后端下发的提供商摘要（注册表顺序：workbuddy → raccoon → …，自定义家附在末尾） */
function providersOf(payload: KeysPayload | null): ProviderOption[] {
  return Array.isArray(payload?.providers) ? payload.providers : []
}

function modelsByProviderOf(payload: KeysPayload | null): Record<string, string[]> {
  const map = payload?.modelsByProvider
  return map && typeof map === 'object' ? map : {}
}

/** 提供商显示名：注册表里没有的 id 回落原样（旧数据里可能有已下线的家） */
function providerLabel(id: string): string {
  return shared().wbProviders?.labelOf?.(id) || id
}

/**
 * 按**当前勾选的提供商**取对外名并集，铺成多选选项。
 * `selected` 里的名字即使不在并集里也照样保留（铺成已勾选状态）—— 用户取消勾选某家
 * 之后，那家独有的模型仍要看得见、能自己取消，否则「保存范围」会变成一次静默的
 * 数据修改。一家都没勾时返回**空数组**（没有约束范围就没有候选）。
 */
function modelOptions(
  table: Record<string, string[]>,
  selectedProviders: string[],
  selected: string[],
): MultiSelectOption[] {
  const names = new Map<string, string>() // 小写 → 原始名（先到先得，保住后端给的大小写）
  const put = (value: unknown) => {
    const text = String(value ?? '').trim()
    if (!text) return
    const key = text.toLowerCase()
    if (!names.has(key)) names.set(key, text)
  }
  selectedProviders.forEach(id => {
    const list = table[id] ?? table[String(id).toLowerCase()]
    if (Array.isArray(list)) list.forEach(put)
  })
  // 已勾选的模型无论是否还在并集里都要铺出来（见函数说明）
  selected.forEach(put)
  // 选项**值**是模型名（提交给后端的身份）不进词典；展示 label 走 t()
  const out = [...names.values()].map(value => ({ value, label: t(value) }))
  out.sort((a, b) => (a.value.toLowerCase() < b.value.toLowerCase() ? -1 : 1))
  return out
}

/**
 * 一行 Key 名下的**限制摘要**。为什么要显示而不是留空：限制是**看不见的** —— 列表上
 * 不写，用户就只记得「我配过点什么」，客户端 404 时会去查模型、查账号，最后才想到是
 * Key 的限制。无限制时显示「不限制」而不是省略：省略与「没读到」长得一样。
 */
function restrictionText(k: KeyEntry): string {
  const providers = Array.isArray(k.allowedProviders) ? k.allowedProviders : []
  const models = Array.isArray(k.allowedModels) ? k.allowedModels : []
  if (!providers.length && !models.length) return t('不限制')
  const parts: string[] = []
  // providerLabel 的输出是展示名（providers.js 侧已处理多语言），原样透出
  if (providers.length) parts.push(providers.map(providerLabel).join(t('、')))
  // 模型那半边只报个数：一屏 Row 里塞不下十几个模型名，悬停由 title 给全量
  if (models.length) parts.push(t('{n} 个模型', { n: models.length }))
  return t('限制：{text}', { text: parts.join(' / ') })
}

/** 限制摘要的完整说明（悬停 title 用；列不宽，详情只能挂这里） */
function restrictionTitle(k: KeyEntry): string {
  const providers = Array.isArray(k.allowedProviders) ? k.allowedProviders : []
  const models = Array.isArray(k.allowedModels) ? k.allowedModels : []
  if (!providers.length && !models.length) {
    return t('这把 Key 不限制提供商与模型（可用全部上游与全部对外模型）')
  }
  const lines: string[] = []
  if (providers.length) lines.push(t('可用提供商：{names}', { names: providers.map(providerLabel).join(t('、')) }))
  if (models.length) lines.push(t('可用模型：{names}', { names: models.join(t('、')) }))
  return lines.join('\n')
}

/**
 * 弹窗里「当前选的摘要」。为什么要有它：限制是**看不见的** —— 弹窗一关，列表上只剩
 * 「限制：…」一行；而多选的触发器只显示连接后的一行文案，清单长了会被省略号收掉。
 * 用户点完「保存范围」就看不到弹窗了，勾了哪几家 / 哪些模型得在这儿给他核对一遍。
 */
function restrictionSummary(providers: readonly string[], models: readonly string[]): string {
  if (!providers.length && !models.length) {
    return t('当前不限制：这把 Key 可以用全部提供商与全部对外模型')
  }
  const parts: string[] = []
  if (providers.length) parts.push(t('提供商：{names}', { names: providers.map(providerLabel).join(t('、')) }))
  if (models.length) parts.push(t('模型：{names}', { names: models.join(t('、')) }))
  // 只限制了模型、没限制提供商（旧数据里可能存在这种组合）：模型候选此刻只剩已勾的
  // 那几个（没有提供商就没有并集可铺），要说清怎么把候选拿回来 —— 否则用户会以为
  // 「模型清单坏了，加不了新的」
  if (!providers.length) {
    parts.push(t('（模型候选需先选提供商；不选则沿用当前这几项，保存后仍按模型白名单生效）'))
  }
  return parts.join('　')
}

/* ─── 多选的选项与取值 ───────────────────────── */

/** 白名单字段是后端给的，可能缺失 / 形状不对（非数组一律当空，与旧实现同口径） */
function stringList(value: unknown): string[] {
  return Array.isArray(value) ? value.map(item => String(item)) : []
}

/**
 * 把 state 里的勾选值按**候选表**归一：只留候选里存在的项、顺序照候选表、写法以候选
 * 为准（模型名的大小写可能与清单不一致，旧实现就是按小写比对后把候选的写法写回 DOM）。
 * 这就是旧实现 `[...select.selectedOptions].map(option => option.value)` 的口径 ——
 * 那时「勾选」只存在于渲染出来的 option 上，候选里没有的 id 落不到 DOM 里，提交时自然
 * 被丢掉。**提交给后端的两个数组必须逐字保持这个口径**。
 */
function pickedFromOptions(values: readonly string[], options: readonly MultiSelectOption[]): string[] {
  const canonical = new Map(options.map(option => [option.value.toLowerCase(), option.value]))
  const chosen = new Set<string>()
  values.forEach(value => {
    const hit = canonical.get(String(value).toLowerCase())
    if (hit) chosen.add(hit)
  })
  return options.filter(option => chosen.has(option.value)).map(option => option.value)
}

/* ─── 弹窗（新建 / 改可用范围共用）──────────────── */

type KeyModalProps = {
  /** 编辑形态的 Key；null = 新建 */
  target: KeyEntry | null
  providers: ProviderOption[]
  modelsByProvider: Record<string, string[]>
  onClose: () => void
  /** 写接口返回的最新列表（与旧实现的 accept 同义）；revealId = 新建后要展开明文的那把 */
  onSaved: (next: KeysPayload | null | undefined, revealId?: string) => void
}

function KeyModal({ target, providers, modelsByProvider, onClose, onSaved }: KeyModalProps) {
  const editingId = target?.id ?? null
  const editing = Boolean(editingId)

  const [name, setName] = React.useState(target?.name ?? '')
  const [keyValue, setKeyValue] = React.useState('')
  const [status, setStatus] = React.useState('')
  const [saving, setSaving] = React.useState(false)
  /** 在途守卫：命令式的关闭判定（Esc / 点遮罩）必须能**同步**读到它 */
  const savingRef = React.useRef(false)

  /** 两个白名单的勾选（受控；**空数组 = 不限制**）。初值照抄后端给的数组，不在这里删改 */
  const [pickedProviders, setPickedProviders] = React.useState<string[]>(
    () => stringList(target?.allowedProviders),
  )
  const [pickedModels, setPickedModels] = React.useState<string[]>(
    () => stringList(target?.allowedModels),
  )

  /**
   * 提供商候选 = 后端下发的整张表（内置家 + 已建的自定义家）。自定义家必须在这一层
   * 就进候选：`pickedFromOptions` 只认候选里有的 id，缺了它，已存的 `custom-xxx`
   * 会在保存时被静默丢掉（见后端 `keys_api` 里 `providers` 候选表的说明）。
   */
  const providerOptions = React.useMemo<MultiSelectOption[]>(
    // 值是提供商 id（提交给后端的身份）不包；展示 label 走 t()（后端下发的展示名）
    () => providers.map(item => ({ value: String(item.id), label: t(String(item.label ?? item.id)) })),
    [providers],
  )

  /**
   * 勾选值按候选表归一后的结果 —— 它是**唯一对外**的东西：MultiSelect 的受控值、摘要、
   * 提交给后端的数组都用它，与旧实现从 `<select>` 的 selectedOptions 读值同一口径
   * （见 pickedFromOptions 的说明）。
   */
  const allowedProviders = React.useMemo(
    () => pickedFromOptions(pickedProviders, providerOptions),
    [pickedProviders, providerOptions],
  )

  /**
   * 模型候选跟着「已勾选的提供商」重建：勾一家立刻多出这家的对外名，取消一家则收回去。
   * 但**已勾选的模型一律留在候选里**（见 modelOptions）—— 用户没动过模型那栏，
   * 保存就不该悄悄少几项。
   */
  const modelCandidates = React.useMemo(
    () => modelOptions(modelsByProvider, allowedProviders, pickedModels),
    [modelsByProvider, allowedProviders, pickedModels],
  )

  const allowedModels = React.useMemo(
    () => pickedFromOptions(pickedModels, modelCandidates),
    [pickedModels, modelCandidates],
  )

  /**
   * 空候选的说明文案分两种：没勾提供商时是「先选提供商」，勾了却是空才是「这几家
   * 现在没有可用模型」（后端没给该家的清单 = 没登录态）。有候选时不写这句 ——
   * 那时浮层里空只可能是搜索没匹配上。
   */
  const modelEmptyHint = modelCandidates.length
    ? t('没有匹配的选项')
    : allowedProviders.length
      ? t('这几家当前没有可用模型（账号未登录或清单为空）')
      : t('请先在上面选择可用提供商')

  /** 收尾：解除在途守卫（写两处，避免两边漂移） */
  function stopSaving(): void {
    savingRef.current = false
    setSaving(false)
  }

  async function save(): Promise<void> {
    if (savingRef.current) return
    // 两个白名单直接用归一后的勾选（见 pickedFromOptions），字段名与取值口径都与旧实现一致
    savingRef.current = true
    setSaving(true)
    setStatus(t('保存中…'))
    try {
      const api = shared().workbuddyDesktop
      if (!api) throw new Error(t('后端桥不可用'))
      if (editingId) {
        // 只提交两个白名单：别名与启停都不动（部分更新语义，见后端 api_keys::update）
        const next = await api.updateKey(editingId, { allowedProviders, allowedModels })
        // 先解除守卫再关窗（旧实现同序：saving = false 在 closeModal 之前）
        stopSaving()
        onSaved(next)
        onClose()
        toast(t('✅ 可用范围已保存'))
        return
      }
      const trimmedKey = keyValue.trim()
      if (trimmedKey && trimmedKey.length < 8) {
        setStatus(t('Key 至少需要 8 个字符'))
        return
      }
      const next = await api.createKey({
        name: name.trim(), key: trimmedKey || undefined, allowedProviders, allowedModels,
      })
      stopSaving()
      onSaved(next, next?.created?.id)
      onClose()
      toast(t('✅ Key 已创建，记得复制给客户端'))
    } catch (error) {
      setStatus(t('保存失败：{reason}', { reason: errorMessage(error) }))
    } finally {
      stopSaving()
    }
  }

  return (
    <Dialog
      open
      onOpenChange={(next, eventDetails) => {
        // 关闭请求（Esc / 点遮罩 / 右上角 ✕）全部汇到这里。保存中拒绝关闭必须走
        // eventDetails.cancel()：光「不更新 open prop」拦不住 Base UI 的 store。
        if (next) return
        if (savingRef.current) {
          eventDetails.cancel()
          return
        }
        onClose()
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>
            {editing
              ? t('可用范围 · {name}', { name: target?.name || t('未命名') })
              : t('新建 API Key')}
          </DialogTitle>
        </DialogHeader>
        <DialogBody>
          <DialogSection>
            <div className='flex flex-wrap items-center gap-2.5'>
              <Label htmlFor='key-name' className='text-[12.5px] whitespace-nowrap text-subtle'>{t('名称')}</Label>
              <Input id='key-name' type='text' maxLength={60} placeholder={t('例如 Cursor / 公司电脑')}
                autoComplete='off' value={name} disabled={editing}
                onChange={event => setName(event.currentTarget.value)} />
            </div>
            {/* 编辑形态**不渲染** Key 输入行：那一行在改范围时没有意义（Key 值不可改）。
                这里既没有常驻需求也没有状态要保，条件渲染最省事 —— 属性式显隐留给
                节点必须常驻的场合（组件库 globals.css 的 [hidden][hidden] 已给它兜底）。 */}
            {!editing && (
              <div className='flex flex-wrap items-center gap-2.5'>
                <Label htmlFor='key-value' className='text-[12.5px] whitespace-nowrap text-subtle'>Key</Label>
                <Input id='key-value' type='text' placeholder={t('留空自动生成；手填至少 8 个字符')}
                  autoComplete='off' spellCheck={false} value={keyValue}
                  onChange={event => setKeyValue(event.currentTarget.value)}
                  // 回车 = 提交（旧实现只绑在这一个输入框上）
                  onKeyDown={event => { if (event.key === 'Enter') void save() }} />
              </div>
            )}
            {/* 两个多选走组件库的 MultiSelect（受控）。宽度规则（min 180 / max 260）原先是
                page-gateway.css 按 `#key-allowed-providers` / `#key-allowed-models` 给的，
                而 MultiSelect 的触发器不吃 id（组件库不转发）—— 那条规则成了死规则，
                等价的宽度锚只能自己带：flex-auto 就是旧 CSS 里的 `flex: 1 1 auto`。
                触发器上没有可见 label 与之关联（同样没有 id 可给 htmlFor），所以 aria-label
                必须给，否则读屏只念到一串连接起来的选项名。 */}
            <div className='flex flex-wrap items-center gap-2.5'>
              <Label className='text-[12.5px] whitespace-nowrap text-subtle'>{t('可用提供商')}</Label>
              <MultiSelect
                value={allowedProviders}
                onValueChange={setPickedProviders}
                options={providerOptions}
                placeholder={t('留空 = 不限制')}
                searchPlaceholder={t('搜索提供商…')}
                aria-label={t('可用提供商')}
                className='flex-auto min-w-[180px] max-w-[260px]'
              />
            </div>
            <div className='flex flex-wrap items-center gap-2.5'>
              <Label className='text-[12.5px] whitespace-nowrap text-subtle'>{t('可用模型')}</Label>
              <MultiSelect
                value={allowedModels}
                onValueChange={setPickedModels}
                options={modelCandidates}
                placeholder={t('留空 = 不限制')}
                emptyHint={modelEmptyHint}
                searchPlaceholder={t('搜索模型…')}
                aria-label={t('可用模型')}
                className='flex-auto min-w-[180px] max-w-[260px]'
              />
            </div>
            <p>
              {t('留空表示不限制；同时设置时请求需同时满足两个条件（模型在白名单内且路由到允许的提供商）。 「可用模型」的候选跟着上面勾选的提供商走：没勾提供商时它是空的（还没有约束范围）， 勾了几家就列出这几家能收的全部对外名。')}
            </p>
            {/* 当前选的摘要：多选的触发器上只显示「连接后的一行文案」，清单长了会被省略号
                收掉 —— 勾了哪几家 / 哪些模型要在这儿摊开。id 沿用旧实现的：page-gateway.css
                按它给这行加了上边距（它是「当前选择」而不是「使用说明」）。 */}
            <p id='key-restrict-summary'>{restrictionSummary(allowedProviders, allowedModels)}</p>
          </DialogSection>
          <div className='min-h-[18px] text-[11.5px] text-muted-foreground'>{status}</div>
        </DialogBody>
        <DialogFooter>
          <div className='mr-auto' />
          <Button variant='outline' onClick={onClose} disabled={saving}>
            {t('取消')}
          </Button>
          <Button variant='default' disabled={saving} onClick={() => void save()}>
            {editing ? t('保存范围') : t('创建')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

/* ─── 页面本体 ───────────────────────────────── */

/** 组件挂载后登记的入口：对外契约的 load / render 都经它转发 */
type PageHandle = {
  load(): Promise<void>
  /** 旧实现的重绘入口（当前无外部调用点，保留契约） */
  render(): void
}

let handle: PageHandle | null = null

/** app.js 切到本页时调它拉一次（挂载前的调用见文件末尾的说明） */
async function load(): Promise<void> {
  await handle?.load()
}

/** 保留旧实现的能力：按当前数据重绘一次 */
function render(): void {
  handle?.render()
}

function KeysPage() {
  const [data, setData] = React.useState<KeysPayload | null>(null)
  /** 已切到明文显示的 key id */
  const [revealed, setRevealed] = React.useState<ReadonlySet<string>>(() => new Set())
  /** 在途操作的行（按钮与开关禁用）；同步守卫读下面的 ref */
  const [pending, setPending] = React.useState<ReadonlySet<string>>(() => new Set())
  /** 弹窗：null = 关着；{ key: null } = 新建 */
  const [modal, setModal] = React.useState<{ key: KeyEntry | null } | null>(null)
  /** 列设置改了 / 契约 render() 被调 → 强制重画（数据没变但可见列变了） */
  const [, setVersion] = React.useState(0)

  const pendingRef = React.useRef<Set<string>>(new Set())
  const loadingRef = React.useRef(false)

  const applyData = React.useCallback((next: KeysPayload | null) => {
    setData(next)
  }, [])

  /** 写接口返回的最新列表：形状不对（读失败）时保持原样，与旧实现的 accept 同口径 */
  const accept = React.useCallback((next: KeysPayload | null | undefined) => {
    if (next && Array.isArray(next.keys)) applyData(next)
  }, [applyData])

  const loadPanel = React.useCallback(async (): Promise<void> => {
    if (loadingRef.current) return
    loadingRef.current = true
    try {
      const api = shared().workbuddyDesktop
      if (!api) throw new Error(t('后端桥不可用'))
      applyData((await api.getKeys()) ?? null)
    } catch (error) {
      toast(t('读取 Key 列表失败：{reason}', { reason: errorMessage(error) }), 'err')
    } finally {
      loadingRef.current = false
    }
  }, [applyData])

  /** 行内异步操作的统一外壳：置忙 → 跑 → 用返回值刷新 → 收忙（失败只 toast） */
  async function runRowAction(
    id: string,
    run: () => Promise<KeysPayload | null | undefined>,
    doneText?: string,
  ): Promise<void> {
    if (pendingRef.current.has(id)) return
    pendingRef.current.add(id)
    setPending(new Set(pendingRef.current))
    try {
      accept(await run())
      if (doneText) toast(doneText)
    } catch (error) {
      toast(t('操作失败：{reason}', { reason: errorMessage(error) }), 'err')
    } finally {
      pendingRef.current.delete(id)
      setPending(new Set(pendingRef.current))
    }
  }

  function toggleReveal(id: string): void {
    setRevealed(prev => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }

  /** 删除：原生 confirm 在 Tauri 的 WebView 里不弹窗、直接放行（等于没有确认） */
  async function removeKey(k: KeyEntry): Promise<void> {
    const api = shared().workbuddyDesktop
    const ask = shared().wbConfirm?.ask
    if (!api || !ask) return
    const label = escapeHtml(k.name || k.masked || k.id)
    const confirmed = await ask({
      title: t('删除网关 Key'),
      html: t('确定删除 Key「<strong>{name}</strong>」？使用它的客户端会立刻无法访问。', { name: label }),
      okText: t('删除'),
      okClass: 'danger',
    })
    if (!confirmed) return
    void runRowAction(k.id, () => api.deleteKey(k.id), t('Key 已删除'))
  }

  /** 开关：写接口回的是最新列表，就地替换（与旧实现的 runRowAction 同路） */
  function toggleEnabled(k: KeyEntry, next: boolean): void {
    const api = shared().workbuddyDesktop
    if (!api) return
    void runRowAction(
      k.id,
      () => api.updateKey(k.id, { enabled: next }),
      next ? t('Key 已启用') : t('Key 已停用'),
    )
  }

  /* ─── 挂载期的两件事：契约登记 + 列设置（见文件头）────── */

  React.useLayoutEffect(() => {
    handle = {
      load: loadPanel,
      render: () => setVersion(version => version + 1),
    }
    onColumnsChanged = () => setVersion(version => version + 1)
    setupColumns()
    // 注册完必须再画一次：首帧的 visibleColumns() 还是「列设置未就绪」的回退值
    // （全部列、声明顺序），用户藏过列的话表头与表体会对不上。layout effect 里的
    // setState 是同步重画，发生在浏览器绘制之前，看不到这一帧。
    setVersion(version => version + 1)
    return () => {
      handle = null
      onColumnsChanged = null
    }
  }, [loadPanel])

  /**
   * 首屏自持加载：app.js 的 showPage 在本脚本加载前就执行过（那时 window.wbKeysPanel
   * 还不存在，切页那次调用落空），用户上次若停在本页，这里补一次 —— 与旧实现文件
   * 末尾的 `if (wbApp.currentPage === 'keys') load()` 等价。
   */
  React.useEffect(() => {
    if (shared().wbApp?.currentPage === 'keys') void loadPanel()
  }, [loadPanel])

  /** 顶栏那枚是本页徽标的镜像（app.js 的 renderTopbarStatus 按 id 读文案与 data-tone）：
   *  数据一变就让它跟上，否则要等下一次主状态轮询（20 秒）才同步 */
  React.useEffect(() => {
    shared().wbApp?.renderTopbarStatus?.()
  }, [data])

  /* ─── 渲染 ─────────────────────────────── */

  const keys = keysOf(data)
  /** 客户端分页（通用表格外壳）：Key 通常只有几把，但口径与其它四张表保持一致 */
  const paging = useClientPaging(keys.length, 'keys')
  const list = paging.paged ? paging.slice(keys) : keys
  const columns = visibleColumns()
  const authRequired = data?.authRequired === true
  const enabledCount = list.filter(item => item.enabled).length
  const badgeText = authRequired
    ? t('已启用鉴权 · {n} 把 Key 生效', { n: enabledCount })
    : t('未启用鉴权')

  /** 一个单元格的内容（不含 <td> 外壳）；「某一列长什么样」只有这一处实现 */
  function cell(columnKey: string, k: KeyEntry, busyRow: boolean): React.ReactNode {
    switch (columnKey) {
      case 'name':
        return (
          <>
            <div className='mid'><span className='t'>{k.name || t('未命名')}</span></div>
            <div className='mname' title={restrictionTitle(k)}>{restrictionText(k)}</div>
          </>
        )
      case 'key': {
        const shown = revealed.has(k.id)
        return (
          <div className='keycell'>
            <code className='kv'>{shown ? k.key : k.masked}</code>
            <Button size='sm' variant='ghost' onClick={() => toggleReveal(k.id)}>
              {shown ? t('隐藏') : t('显示')}
            </Button>
            {/* data-copy 是 clipboard.js 的委托钩子 */}
            <Button size='sm' variant='ghost' data-copy={k.key} title={t('复制 Key')}>{t('复制')}</Button>
          </div>
        )
      }
      case 'time':
        return (
          <span className='muted'>{k.createdAt ? shared().wbApp?.formatTime?.(k.createdAt) : '—'}</span>
        )
      case 'state':
        return (
          <Switch checked={k.enabled === true} disabled={busyRow}
            aria-label={t('启用「{name}」', { name: k.name || t('未命名') })}
            onCheckedChange={next => toggleEnabled(k, next)} />
        )
      case 'act':
        return (
          <div className='row-actions'>
            <Button size='sm' variant='ghost' disabled={busyRow} onClick={() => setModal({ key: k })}>
              {t('可用范围')}
            </Button>
            <Button size='sm' variant='destructive' disabled={busyRow}
              onClick={() => void removeKey(k)}>
              {t('删除')}
            </Button>
          </div>
        )
      default:
        return null
    }
  }

  return (
    <section className='panel'>
      <div className='panel-head'>
        <h2>{t('API Key 列表')}</h2>
        {/* id 与 data-tone 保留：app.js 的 renderTopbarStatus 按 id 镜像这枚徽标的文案
            与配色（data-tone 有值走它，不去拆组件库 Badge 那串 Tailwind 类名） */}
        <Badge id='keys-status' variant={authRequired ? 'success' : 'warning'}
          data-tone={authRequired ? 'ok' : 'warn'}>
          {badgeText}
        </Badge>
        <div className='head-actions'>
          {/* 列设置的触发按钮由 wbColSettings.register 插进这个容器的最前面（命令式，
              与模型管理页同一手法：插入位置由那边决定，本岛只留容器） */}
          <Button variant='default' onClick={() => setModal({ key: null })}>
            {t('＋ 新建 Key')}
          </Button>
        </div>
      </div>

      <div className='models-table-wrap'>
        <table className='models-table keys-table'>
          {/* colgroup / thead 由本文件渲染，但 data-col 一个不少：列设置的就地重排
              （syncStaticHead）与列宽层（table-columns.js）都按它定位列 */}
          <colgroup>
            <col className='k-name' data-col='name' />
            <col className='k-key' data-col='key' />
            <col className='k-time' data-col='time' />
            <col className='k-state' data-col='state' />
            <col className='k-act' data-col='act' />
          </colgroup>
          <thead>
            <tr>
              <th data-col='name'>{t('名称')}</th>
              {/* 「Key」是产品固定叫法（无中文），不进词典 */}
              <th data-col='key'>Key</th>
              <th data-col='time'>{t('创建时间')}</th>
              <th data-col='state'>{t('启用')}</th>
              <th className='r' data-col='act'>{t('操作')}</th>
            </tr>
          </thead>
          <tbody>
            {list.length ? list.map(k => {
              const busyRow = pending.has(k.id)
              return (
                <tr key={k.id} className={k.enabled ? '' : 'off'} data-id={k.id}>
                  {columns.map(column => (
                    <td key={column.key} className={`${CELL_CLASS[column.key] ?? ''} ta-${column.align}`}>
                      {cell(column.key, k, busyRow)}
                    </td>
                  ))}
                </tr>
              )
            }) : (
              // 空态的 colspan 跟着可见列数走：写死 5 之后藏起两列，这一格会比表体宽出
              // 两格，把整张表顶出横向滚动
              <tr>
                <td colSpan={columns.length} className='empty'>
                  {data ? t('还没有 Key，当前不鉴权') : t('加载中…')}
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>

      <TableFooter
        leading={(
          // 行内 <code> / <b> 拆片段保住（与 docs-page 脚注同一手法）；
          // 片段里的空格是 JSX 折行产生的原文空隙，照抄
          <span>
            {t('客户端请求需带 ')}
            <code>{'Authorization: Bearer <key>'}</code>
            {t(' 或 ')}
            <code>{'x-api-key: <key>'}</code>
            {t('； 修改后立即生效，本程序自身会自动使用第一把启用的 Key。每把 Key 可单独限制')}
            <b>{t('可用提供商')}</b>
            {t('与')}
            <b>{t('可用模型')}</b>
            {t('（行内「可用范围」）：留空 = 不限制，两个都设时 按交集生效 —— 被限制的模型对这把 Key 表现为「不存在」（拉 /v1/models 也看不到它）， 提供它的家不在可用列表里时请求同样被拒。')}
          </span>
        )}
        total={keys.length}
        range={paging.paged ? { start: paging.rangeStart, end: paging.rangeEnd } : null}
        page={paging.page}
        pageCount={paging.pageCount}
        size={paging.size}
        onSizeChange={paging.setSize}
        onPageChange={paging.goto}
      />

      {modal ? (
        <KeyModal
          target={modal.key}
          providers={providersOf(data)}
          modelsByProvider={modelsByProviderOf(data)}
          onClose={() => setModal(null)}
          onSaved={(next, revealId) => {
            if (revealId) setRevealed(prev => new Set(prev).add(revealId))
            accept(next)
          }}
        />
      ) : null}
    </section>
  )
}

/* ─── 挂载：接管 index.html 里既有的页面区块 ─────── */

let mounted = false

/**
 * 把 React root 直接建在页面区块上（不套宿主 div：页面 CSS 用 `.page > *` 这组直接
 * 子选择器分配高度，中间插一层会打断它）。先清掉骨架里的静态子节点 —— 下面按同样的
 * 类名重新渲染，留着会与 React 打架。
 */
function mount(): void {
  if (mounted) return
  const section = document.querySelector<HTMLElement>(SECTION)
  if (!section) return
  mounted = true
  section.replaceChildren()
  createRoot(section).render(<KeysPage />)
}

// 脚本排在页面骨架之后（index.html 里 islands/ui.js 在各 section 之后），正常直接挂；
// 万一将来被挪到前面，退化成等 DOM 解析完再挂。
if (document.querySelector(SECTION)) mount()
else document.addEventListener('DOMContentLoaded', mount, { once: true })

declare global {
  interface Window {
    /** 网关 Key 面板（替换 ui/keys-panel.js，接口与原实现一致） */
    wbKeysPanel?: {
      /** 切到本页时拉一次（app.js:151） */
      load(): Promise<void>
      /** 按当前数据重绘（旧实现的能力，保留） */
      render(): void
      /** 当前可见的列（table-columns.js 的列宽层按它算覆盖值与末列把手） */
      visibleColumns(): (ColumnDecl & { align: Align })[]
    }
  }
}

window.wbKeysPanel = { load, render, visibleColumns }
