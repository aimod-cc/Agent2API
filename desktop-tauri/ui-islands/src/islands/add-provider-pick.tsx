/**
 * Agent2API · 「添加账号」弹窗第 1 步：选提供商（账号类型分段 + 搜索 + 卡片网格）。
 *
 * 替换旧 add-provider-forms.js 的 providerCards / cardHtml / logoHtml / cardMeta /
 * renderProviderCards / newCardHtml 与 mountAddProviderUi 里注入的那段骨架。
 *
 * 卡片网格与卡片的类名（.add-provider-grid / .add-provider-card / .add-provider-logo…）
 * 原样保留：那是布局性的成套样式（四列网格 + 固定高度滚动 + 卡片三行排版），
 * 组件库没有对应件，重写成 Tailwind 只会把一份已经调好的像素预算再抄一遍。
 * 控件（分段、搜索框）走组件库。
 *
 * ── 「全部」段 ──────────────────────────────────────────────
 * 分段栏最前面一档「全部」（也是打开弹窗的默认段）：它把四个分类的卡片按
 * 分组带拼在同一屏里，搜索一次跨全部分类。解决的痛点很具体 —— 搜索框只过滤
 * 当前分段，用户在「API 模板」段搜 Cline、搜 Qoder 只会看到「没有匹配」，
 * 而要找的那家就在隔壁那段里。单分类段照旧保留：知道找哪一类时它才是最短
 * 路径，「全部」段每组的「只看这一类」就是把用户收回那条路径的入口。
 */

import * as React from 'react'
import { Alert, InputGroup, InputGroupAddon, InputGroupInput, SegmentedControl, Button, Item } from '@ui'

import { isAddAccountProviderHidden } from '../../../ui-shared/provider-availability'
import { accountCountOf, shared } from './add-account-bridge'
import { presetOfProvider, providerPresetIcon, providerSiteLabel } from './add-provider-metadata'
import { WORKBUDDY_ENTRY_LABEL, WORKBUDDY_PROVIDER } from './add-account-configs'
import { DIALOG_SEG_CLASS } from './dialog-segmented'
import { AddGroup } from './add-account-kit'
import { t } from '../i18n'
import { Empty } from '@ui'

/** 第 1 步的账号类型：全部 / Agent / Web / API 模板 / 已建 API / 导入 */
export type AccountType = 'all' | 'proxy' | 'web' | 'preset' | 'custom' | 'import'

export const TYPE_ALL: AccountType = 'all'
export const TYPE_PROXY: AccountType = 'proxy'
export const TYPE_WEB: AccountType = 'web'
export const TYPE_PRESET: AccountType = 'preset'
export const TYPE_CUSTOM: AccountType = 'custom'
export const TYPE_IMPORT: AccountType = 'import'

/**
 * 「全部」段里按顺序列出的分类：与分段栏一一对应（Agent / Web / API 模板 /
 * 已建 API），顺序也照抄分段栏 —— 两处顺序不一致的话，用户看到的「第几类」
 * 就对不上，读起来像两套分类。
 */
const GROUPED_TYPES: AccountType[] = [TYPE_PROXY, TYPE_WEB, TYPE_PRESET, TYPE_CUSTOM]

/**
 * 「导入」分段是否露出。当前 **false**：这一屏还没做完整，先从界面上收起来。
 *
 * 收的是入口、不是实现 —— 导入面板、后端 `/api/import/cc-switch` 与配套 CSS
 * 全部原样留着（见 add-provider-import.tsx），等这一屏补齐把这里改回 true 即可。
 * 关掉之后：分段不生成 → 到不了 TYPE_IMPORT → 面板不挂载、底部「导入所选」
 * 也不会被点亮，走的就是「用户从没点过这个分段」那条路径。
 */
export const IMPORT_SEGMENT_ENABLED = false

/** 预置家卡片的取值前缀（不是 provider id，只是卡片自己的标记） */
export const PRESET_CARD_PREFIX = 'preset:'
/** 「新建自定义提供商」那张卡片的取值（不是 provider id，只是卡片自己的标记） */
export const NEW_PROVIDER_CARD_ID = '__new__'

/** 分段值归一：未知取值按「Agent」处理。 */
export function typeValueOf(value: string): AccountType {
  if (value === TYPE_ALL) return TYPE_ALL
  if (value === TYPE_PRESET || value === TYPE_CUSTOM || value === TYPE_WEB) return value
  if (IMPORT_SEGMENT_ENABLED && value === TYPE_IMPORT) return TYPE_IMPORT
  return TYPE_PROXY
}

/**
 * 分类的展示名：分段栏的分段与「全部」段的分组带**共用这一份**。
 * 两处必须是同一个词 —— 带子上写着「API 模板」而分段里是别的叫法，用户就
 * 对不上「这是刚才那一档」。Agent / Web 是客户端自己的叫法（专名），不进词典。
 */
function typeLabelOf(type: AccountType): string {
  if (type === TYPE_ALL) return t('全部')
  if (type === TYPE_WEB) return 'Web'
  if (type === TYPE_PRESET) return t('API 模板')
  if (type === TYPE_CUSTOM) return t('已建 API')
  if (type === TYPE_IMPORT) return t('导入')
  return 'Agent'
}

/**
 * 内置家的真实图标：`id → assets/providers/<file>.png`，图取自各客户端安装目录
 * 内嵌的图标（与系统里显示的为同一张；AutoClaw / WorkBuddy 各自两地、Cline 两种
 * 账号、Accio / ZCode 两地共用一张 —— 它们本来就是同一个客户端）。
 * 自定义家与没收录图标的家回落到首字母徽章。
 * 导出共用：账号页的分组带（accounts-panels 的 GroupBandRow）、模型管理页
 * （models-page）与签到中心的提供商行（checkin-page）用同一份映射，别处不要照抄。
 */
export const PROVIDER_ICONS: Record<string, string> = {
  workbuddy: 'assets/providers/workbuddy.png',
  'workbuddy-intl': 'assets/providers/workbuddy.png',
  raccoon: 'assets/providers/raccoon.png',
  catpaw: 'assets/providers/catpaw.png',
  autoclaw: 'assets/providers/autoclaw.png',
  'autoclaw-intl': 'assets/providers/autoclaw.png',
  qoder: 'assets/providers/qoder.png',
  'qoder-intl': 'assets/providers/qoder.png',
  'cline-free': 'assets/providers/cline.png',
  'cline-pass': 'assets/providers/cline.png',
  accio: 'assets/providers/accio.png',
  'accio-cn': 'assets/providers/accio.png',
  zcode: 'assets/providers/zcode.png',
  'zcode-intl': 'assets/providers/zcode.png',
  codearts: 'assets/providers/codearts.png',
  trae: 'assets/providers/trae.png',
  // Loomy：取自安装包 `resources/app.asar` 的 Windows 图标集
  // （`build/icons/favicon-228.png`，与系统里显示的应用图标为同一张；
  // 绿色圆底上的品牌形象）
  loomy: 'assets/providers/loomy.png',
  // KukuAI：取自客户端 `GenFlowPro.exe` 的 RT_ICON 资源（256×256 那张，
  // 与系统里显示的应用图标为同一张）
  kuku: 'assets/providers/kuku.png',
  // MonkeyCode：取自客户端 asar 内嵌的应用图标（`electron/icon.png`，与系统里
  // 显示的应用图标为同一张；国内 / 国际版共用这一张，同 AutoClaw 的形态）
  monkeycode: 'assets/providers/monkeycode.png',
  'monkeycode-intl': 'assets/providers/monkeycode.png',
  // Command Code / Antigravity：这两家没有可解的本地安装包，图取自官方资产 ——
  // Command Code 用官网 `apple-touch-icon.png`（黑底 + 白结的应用图标形态；官方
  // brand 页那种「白描边框 + 满幅黑底」的变体在 13px 徽章里会糊），Antigravity
  // 用官方 press 页的「Icon - Full Color」（antigravity.google/press）
  commandcode: 'assets/providers/commandcode.png',
  antigravity: 'assets/providers/antigravity.png',
  // OfficeAce（华为云果办）：取自官网产品页的产品图标
  // （`res-static.hc-cdn.cn/.../OfficeAce/新图标.png`，176×176 RGBA，
  // 归一成 256×256 入库 —— 与系统里显示的应用图标为同一形象）
  officeace: 'assets/providers/officeace.png',
  // OrcaTerm（腾讯云遨驰终端桌面客户端）：取自官方 PWA 图标
  // （`cloudcache.tencent-cloud.com/qcloud/orcaterm/pwa/icon/orcaterm_192x192.png`，
  // 与客户端 / 官网显示的应用图标为同一形象）
  orcaterm: 'assets/providers/orcaterm.png',
// 智谱清言：取自客户端安装包 `resources/app.asar` 内的应用图标
  // （`assets/icon.png`，512×512，与系统里显示的应用图标为同一形象；
  // 归一成 256×256 入库 —— 与 officeace 同一处置）
  chatglm: 'assets/providers/chatglm.png',
  'deepseek-web': 'assets/providers/deepseek.png',
  // Tabbit 浏览器：取自国内站 `web.tabbit.com/favicon.ico`（实际是 160×160 PNG），
  // 两站共用这一张（同一产品形象）。
  tabbit: 'assets/providers/tabbit.png',
  'tabbit-intl': 'assets/providers/tabbit.png',
  // ModelScope（魔搭）：取自官网 favicon（`https://modelscope.cn/favicon.ico`，
  // 内含 128×128 的一帧，归一成 256×256 入库 —— 与站点标签页上显示的是同一形象；
  // 国内 / 国际版共用这一张，同 AutoClaw / MonkeyCode 的形态）
  modelscope: 'assets/providers/modelscope.png',
  'modelscope-intl': 'assets/providers/modelscope.png',
  // AtomCode（AtomGit / GitCode）：取自官网首页的产品图标
  // （`https://cdn-news.gitcode.com/news/atomcode-icon1.png`，524×524 RGBA，
  // 归一成 256×256 入库 —— 与官网首页 / 客户端显示的应用图标为同一形象）。
  // ⚠️ 官网的 `<link rel="icon">` 指向的是 GitCode **平台**的 favicon
  // （`cdn-static.gitcode.host/static/images/logo-favicon.png`），不是本产品的形象，
  // 所以没有采用它。
  atomcode: 'assets/providers/atomcode.png',
  // LobsterAI（网易有道「有道龙虾」）：取自官方开源仓库的应用图标
  // （`build/icons/png/256x256.png`，原尺寸直接入库 —— 红底白龙虾，与官网
  // `lobsterai.youdao.com/icon-192.png`、README 的 `public/logo.png` 同一形象，
  // 也就是系统里显示的客户端图标）。⚠️ 早前用的是 sibling 仓库
  // cpa-jethub-plugins 里那枚 24×24 通用小怪物 SVG，与产品实际形象不符，已换掉。
  lobsterai: 'assets/providers/lobsterai.png',
  doubaowork: 'assets/providers/doubaowork.png',
  cursor: 'assets/providers/cursor.svg',
  kiro: 'assets/providers/kiro.svg',
  // AStudio（科大讯飞 Astron Studio）：取自客户端图标资源（紫底白 N + 星芒，
  // 与系统里显示的应用图标同一形象；原始 32×32 归一成 256×256 入库）
  astudio: 'assets/providers/astudio.png',
  devin: 'assets/providers/devin.svg',
  factory: 'assets/providers/factory.svg',
  zed: 'assets/providers/zed.svg',
  grok: 'assets/providers/grok.svg',
  'minimax-code': 'assets/providers/minimax-code.svg',
  'minimax-code-global': 'assets/providers/minimax-code.svg',
  qwenwork: 'assets/providers/qwenwork.svg',
  'qwenwork-intl': 'assets/providers/qwenwork.svg',
  'qwenwork-enterprise': 'assets/providers/qwenwork.svg',
  'kimi-work': 'assets/providers/kimi-work.svg',
  opendesign: 'assets/providers/opendesign.svg',
  'nous-portal': 'assets/providers/nous-portal.svg',
  'claude-subscription': 'assets/providers/anthropic.png',
  'codex-subscription': 'assets/providers/openai.png',
  'chatgpt-subscription': 'assets/providers/openai.png',
  'copilot-subscription': 'assets/providers/copilot.svg',
  'gemini-subscription': 'assets/providers/gemini-cli.svg',
}

/**
 * Agent 段**不露出添加入口**的内置家（精确匹配 provider id）。目前只有
 * `cline-pass`：
 *   ① 它支持 API Key 形态（与 OAuth token 走同一个 Bearer 头），添加入口已迁到
 *      「预置 API」段的 Cline Pass 卡（`preset-providers.js` 的 `clinepass`）；
 *   ② 后端 Provider 与存量账号完全不动 —— 存量账号照常转发 / 刷新 / 删除，
 *      账号页的分组带仍会列出 cline-pass（那段走 accounts-domain 的
 *      providerSummaries 直读 providers 摘要，不经过本文件）；
 *   ③ 这是刻意的（与 `workbuddy-intl` 那条同类：入口只留一处），**不要当成
 *      漏掉的 bug 把它恢复**。
 * 注意只按 id 精确匹配：`cline-free`（免费池）不受影响，仍在 Agent 段。
 */
const HIDDEN_AGENT_PROVIDERS = new Set(['cline-pass'])

type CardItem = {
  id: string
  label: string
  count: number
  preset?: boolean
  custom?: boolean
  icon?: string
  site?: string
  template?: string
}

/** 模板卡只负责新建；已建 API 按提供商 ID 列出，不按名称或模板类型合并。 */
function providerCards(accountType: AccountType): CardItem[] {
  const providers = shared().wbProviders
  if (accountType === TYPE_PRESET) {
    return (shared().wbPresetProviders?.list || []).map(preset => ({
      id: PRESET_CARD_PREFIX + preset.key,
      label: preset.name,
      count: 0,
      preset: true,
    }))
  }
  if (accountType === TYPE_CUSTOM) {
    return (providers?.customList?.() || []).map(provider => ({
      id: provider.id,
      label: provider.name || provider.id,
      count: accountCountOf(provider.id),
      custom: true,
      icon: providerPresetIcon(provider),
      site: providerSiteLabel(provider.baseUrl),
      template: presetOfProvider(provider)?.name,
    }))
  }
  // Cline Pass 的添加入口不在这一段（判据与缘由见 HIDDEN_AGENT_PROVIDERS）：
  // 在映射成卡片之前就把摘要里那条剔掉，后面的名字处理都不必再想它。
  // 过滤只落在**本函数**（PickStep 的列表构造）上：账号页分组带 / 模型管理页
  // 读的是同一份 providers 摘要，但各自有自己的列表构造（accounts-domain 的
  // providerSummaries、models-page 的左栏），不经过这里，所以存量 cline-pass
  // 账号仍照常成组显示。
  const list = (providers?.all?.() || [])
    .filter(item => !isAddAccountProviderHidden(String(item?.id || '')))
    .filter(item => (item.category === 'web') === (accountType === TYPE_WEB))
    .filter(item => accountType === TYPE_WEB || !HIDDEN_AGENT_PROVIDERS.has(String(item?.id || '')))
  // WorkBuddy 这张卡与它的弹窗标题用**中性品牌名**（不带地区）而不是注册名：
  // 这个入口内部有「账号版本」分段，两版都从这一张卡进（名字见
  // `WORKBUDDY_ENTRY_LABEL` 的说明）。与下面那条「国际版不进卡片列表」是同一件事
  // 的两半：入口一处、版本在块里选。
  // 注册名（「AutoClaw 国内版」这类）过一遍词典：卡片名与弹窗标题都由它出，
  // 不查表的话非中文界面会在这里印中文
  const labelOf = (id: string, fallback: string): string => {
    if (id === 'minimax-code') return t('MiniMax Code 国内版')
    if (id === 'minimax-code-global') return t('MiniMax Code 国际版')
    // 千问办公两版已由「提供商增强」插件实现，名字与插件注册表的 meta 逐字一致
    // （`千问办公 国内版` / `千问办公 国际版`）。这里仍然要覆盖一次：插件给的
    // label 是中文源串，只有走 t() 才会被词典翻成其余五种语言。
    if (id === 'qwenwork') return t('千问办公 国内版')
    if (id === 'qwenwork-intl') return t('千问办公 国际版')
    if (id === 'qwenwork-enterprise') return t('QwenWork 千问办公企业版')
    return id === WORKBUDDY_PROVIDER ? WORKBUDDY_ENTRY_LABEL : t(shared().wbProviders?.labelOf?.(id) || fallback)
  }
  // 摘要为空**不再兜底放 WorkBuddy 一张**（2026-10 改）：
  // 以前那张兜底卡的理由是「弹窗不能因为一次状态未就绪就空着」，但本版本
  // 转发与账号实现全在插件里，没装插件时它点了必然 503 —— 兜底一张点不动的卡
  // 比空着更糟（用户以为功能坏了）。空列表的处置交给渲染处：显示
  // 「先去安装提供商插件包」，并给一个跳转入口。
  //
  // 「摘要还没到」那一瞬也走空态：`add-account-modal` 的 reset 已经会在
  // 列表为空时补拉一次再重画（见那里的 `providers?.load?.().then(bump)`），
  // 所以空态最多闪一下。这个代价比留一张必然失败的卡小得多。
  const cards: CardItem[] = list.map(item => {
    const id = String(item.id || '')
    return {
      id,
      label: labelOf(id, String(item.label || item.id || '')),
      count: Number(item.count) || 0,
    }
  })
  // WorkBuddy 国际版**不进这张卡列表**：它已经由国内版那张卡里的「账号版本」
  // 分段覆盖（同一个 WorkBuddyBlock 的两个选项），两处入口会让用户以为要走两条
  // 流程，而落到国际版卡片时那个块根本不会渲染（`add-account-modal` 按
  // `provider === 'workbuddy'` 分派，症状是一张空表单）。
  // 拆家后它是独立 provider，但**添加入口**仍然是同一个块 —— 与 AutoClaw /
  // Accio / ZCode 那三家（各自一块、没有地区分段）的形态不同，这是刻意的：
  // WorkBuddy 两地的登录页与凭证形态完全一致，合成一块对用户更省事。
  const visible = cards.filter(item => item.id !== 'workbuddy-intl')
  // 展示顺序微调：AutoClaw 与 Qoder 的两个版本都要挨着（两列网格里同处一行）
  // 且**国内版在前** —— 摘要给的是注册表顺序，Qoder 国际版注册在 Qoder 后面
  // 本就相邻（拆家后无需再挪）；AutoClaw 国际版排在 Cline 之后，仍要前移。
  const from = visible.findIndex(item => item.id === 'autoclaw-intl')
  const to = visible.findIndex(item => item.id === 'autoclaw')
  if (to >= 0 && from > to) visible.splice(to + 1, 0, visible.splice(from, 1)[0])
  return visible
}

/** 卡片图标：收录过的家出真实图标（预置家问预置目录要），其余用首字母徽章 */
function Logo({ item }: { item: CardItem }): React.ReactElement {
  const icon = item.id.startsWith(PRESET_CARD_PREFIX)
    ? shared().wbPresetProviders?.iconOf?.(item.id.slice(PRESET_CARD_PREFIX.length)) || ''
    : item.icon || PROVIDER_ICONS[item.id]
  if (icon) {
    return (
      <span className='add-provider-logo has-icon'>
        <img src={icon} alt='' loading='lazy' />
      </span>
    )
  }
  const initial = String(item.label || '?').trim().slice(0, 1).toUpperCase() || '?'
  return <span className='add-provider-logo'>{initial}</span>
}

function cardMeta(item: CardItem): string {
  if (item.preset) return t('使用模板新建提供商')
  if (item.custom) return item.count ? t('{n} 个账号，点击添加', { n: item.count }) : t('点击添加账号')
  return item.count ? t('{n} 个账号', { n: item.count }) : t('还没有账号')
}

function ProviderCard({ item, onPick }: { item: CardItem; onPick: (id: string) => void }): React.ReactElement {
  return (
    <Item render={<Button variant='outline' />} variant='outline'
      className='add-provider-card h-auto whitespace-normal text-left'
      data-provider={item.id}
      title={[item.label, item.site, item.template].filter(Boolean).join(' · ')}
      role='option'
      onClick={() => onPick(item.id)}
    >
      <Logo item={item} />
      <span className='add-provider-info'>
        <span className='add-provider-name'>{item.label}</span>
        {item.site ? <span className='add-provider-meta'>{[item.site, item.template].filter(Boolean).join(' · ')}</span> : null}
        <span className='add-provider-meta'>{cardMeta(item)}</span>
      </span>
      <span className='add-provider-go'>›</span>
    </Item>
  )
}

/**
 * 「新建自定义提供商」卡：落在自定义块上，由那个块切到「新建」模式。
 * 说明文案短到一行（与其它卡片同宽）：「OpenAI / Anthropic 兼容」那层约束留给表单页去讲。
 */
function NewProviderCard({ onPick }: { onPick: (id: string) => void }): React.ReactElement {
  return (
    <Item render={<Button variant='dashed' />} variant='outline'
      className='add-provider-card is-new h-auto whitespace-normal text-left'
      data-provider={NEW_PROVIDER_CARD_ID}
      role='option'
      onClick={() => onPick(NEW_PROVIDER_CARD_ID)}
    >
      <span className='add-provider-logo is-new'>＋</span>
      <span className='add-provider-info'>
        <span className='add-provider-name'>{t('新建自定义提供商')}</span>
        <span className='add-provider-meta'>{t('接入一个兼容上游')}</span>
      </span>
      <span className='add-provider-go'>›</span>
    </Item>
  )
}

export type PickStepProps = {
  accountType: AccountType
  onAccountTypeChange: (value: AccountType) => void
  search: string
  onSearchChange: (value: string) => void
  onPick: (id: string) => void
  /** 摘要 / 自定义目录异步到位后的重画信号（值本身不参与渲染） */
  version: number
  /** 导入段：卡片网格与搜索整块收起（导入面板占同一块区域） */
  importing: boolean
  /** 导入面板（分段收起时不存在）：与卡片网格互斥显隐，占同一块位置 */
  importPanel?: React.ReactNode
}

export function PickStep({
  accountType,
  onAccountTypeChange,
  search,
  onSearchChange,
  onPick,
  version,
  importing,
  importPanel,
}: PickStepProps): React.ReactElement {
  const hasAgentCards = React.useMemo(() => providerCards(TYPE_PROXY).length > 0, [version])
  const hasWebCards = React.useMemo(() => providerCards(TYPE_WEB).length > 0, [version])
  const hiddenType = (accountType === TYPE_PROXY && !hasAgentCards)
    || (accountType === TYPE_WEB && !hasWebCards)
  const effectiveType = hiddenType ? (hasWebCards ? TYPE_WEB : TYPE_PRESET) : accountType
  const keyword = search.trim().toLowerCase()
  const hitOf = (item: CardItem): boolean => !keyword
    || [item.label, item.site, item.template].filter(Boolean).join(' ').toLowerCase().includes(keyword)
  // 有关键词时「新建」那张卡收起来 —— 用户在找的是一家。
  // 「全部」段里它住在「已建 API」组里当第一张（与单分类段的位置一致）
  const newCard = !keyword && (effectiveType === TYPE_CUSTOM || effectiveType === TYPE_ALL)
  const grouped = effectiveType === TYPE_ALL
  /** 这一屏的分组：单分类段只有自己那一组（不带分组带），「全部」段四组按分段栏的顺序排 */
  const groups = (grouped ? GROUPED_TYPES : [effectiveType]).map(type => ({
    type,
    label: typeLabelOf(type),
    items: providerCards(type).filter(hitOf),
  }))
  // 「全部」段：空组整组不出（连分组带一起收），否则找一类得先翻过几条空带子。
  // 唯一例外是「已建 API」—— 「新建自定义提供商」那张卡住在里面，它永远可达
  const shownGroups = groups.filter(group => group.items.length > 0
    || (group.type === TYPE_CUSTOM && newCard))
  const hit = shownGroups.flatMap(group => group.items)
  // 四个分类合起来命中多少家：「全部」那一档的计数徽标就是它（没搜索时即总家数）。
  // 徽标不只是装饰 —— 在「API 模板」段搜 Cline 时它显示 1，等于替「全部」喊了一声
  const allHitCount = GROUPED_TYPES
    .reduce((sum, type) => sum + providerCards(type).filter(hitOf).length, 0)
  // 当前分段里一家没有、别的分类里有：这正是「全部」段要解决的那件事，
  // 所以给一条提示把人送过去，而不是指望他自己想到去点「全部」。
  // 此分支下当前分段命中为 0，所以全量命中数就是「其它分类」的命中数
  const otherHitCount = keyword && !grouped && !hit.length ? allHitCount : 0
  // Agent 与 Web 只列出已加载且未暂停的提供商。

  return (
    // 分段与列表同一块：两者是同一个问题的两面（「给什么形态的上游加账号」→
    // 「给哪一家加」），分两块带边框会让人以为是两个独立步骤。
    // 分组名走 AddGroup 的 label（挂在方框外面），框内第一行是分段 + 搜索。
    <AddGroup label={t('选择提供商')}>
      {/* 分段与搜索同一行：左边选形态，右边是这一屏的过滤器 */}
      <div className='add-pick-row' style={importing ? { display: 'none' } : undefined}>
        <SegmentedControl
          aria-label={t('账号类型')}
          className={DIALOG_SEG_CLASS}
          options={[
            // 默认落在「全部」：搜索一次跨全部分类，不必先猜自己该进哪一段
            { value: TYPE_ALL, label: typeLabelOf(TYPE_ALL), count: allHitCount },
            // Agent 段只在内置家真的存在时出现（判据见 hasAgentCards）
            ...(hasAgentCards ? [{ value: TYPE_PROXY, label: typeLabelOf(TYPE_PROXY) }] : []),
            ...(hasWebCards ? [{ value: TYPE_WEB, label: typeLabelOf(TYPE_WEB) }] : []),
            { value: TYPE_PRESET, label: typeLabelOf(TYPE_PRESET) },
            { value: TYPE_CUSTOM, label: typeLabelOf(TYPE_CUSTOM) },
            // 「导入」分段暂时收起（见 IMPORT_SEGMENT_ENABLED）：整段不生成
            ...(IMPORT_SEGMENT_ENABLED ? [{ value: TYPE_IMPORT, label: typeLabelOf(TYPE_IMPORT) }] : []),
          ]}
          value={effectiveType}
          onValueChange={value => onAccountTypeChange(typeValueOf(value))}
        />
        <span id='add-search-wrap' className='add-provider-search ml-auto max-w-[340px] min-w-0 flex-[1_1_200px]'>
          <InputGroup>
            <InputGroupAddon>⌕</InputGroupAddon>
            <InputGroupInput
              id='add-provider-search'
              type='search'
              placeholder={t('搜索提供商…')}
              autoComplete='off'
              aria-label={t('搜索提供商')}
              value={search}
              onChange={event => onSearchChange(event.currentTarget.value)}
            />
          </InputGroup>
        </span>
      </div>
      {/* 跨分类命中提示：拦在卡片网格上方（不进滚动区），点一下直接切到「全部」 */}
      {otherHitCount > 0 ? (
        <Alert className='flex items-center gap-3'>
          <span>
            {t('当前分类里没有「{keyword}」，其它分类有 {n} 个匹配', {
              keyword: search.trim(),
              n: otherHitCount,
            })}
          </span>
          <Button size='sm' variant='outline' type='button' className='add-provider-hint-go'
            onClick={() => onAccountTypeChange(TYPE_ALL)}>
            {t('看全部匹配')}
          </Button>
        </Alert>
      ) : null}
      {/* 导入面板插在卡片网格的位置上（与网格互斥显隐），放在网格之前 ——
          两屏内容同一块区域，滚动位置也就跟着换，不会串 */}
      {importPanel}
      <div
        className='add-provider-grid'
        id='add-provider-grid'
        role='listbox'
        aria-label={t('选择要添加账号的提供商')}
        style={importing ? { display: 'none' } : undefined}
      >
        {shownGroups.map(group => (
          // 分组带是通栏的一格（grid-column: 1/-1），卡片仍是同级网格项 ——
          // 包一层 Fragment 而不是 wrapper，列宽才不会被多包的一层打乱
          <React.Fragment key={group.type}>
            {grouped ? (
              <div className='add-provider-band' role='group' aria-label={group.label}>
                <span className='add-provider-band-name'>{group.label}</span>
                {group.items.length ? (
                  // 复用语料里现成的「{count} 家」（插件页的包内家数）：同一个读数
                  // 在两处必须是同一句话，新造一条键只会多一份要翻译的空值
                  <span className='add-provider-band-stat'>{t('{count} 家', { count: group.items.length })}</span>
                ) : null}
                <span className='add-provider-band-fill' />
                <Button size='xs' variant='ghost' type='button' className='add-provider-band-only'
                  onClick={() => onAccountTypeChange(group.type)}>
                  {t('只看这一类')}
                </Button>
              </div>
            ) : null}
            {newCard && group.type === TYPE_CUSTOM ? <NewProviderCard onPick={onPick} /> : null}
            {group.items.map(item => <ProviderCard key={item.id} item={item} onPick={onPick} />)}
          </React.Fragment>
        ))}
        {!hit.length && keyword ? (
          <Empty className='add-provider-empty'>{t('没有匹配「{keyword}」的提供商', { keyword: search.trim() })}</Empty>
        ) : null}
        {/* 一个都没有、也没在搜索、也没有「新建自定义提供商」= 还没装插件包。这是
            **正常状态**（本版本的转发与账号实现都在插件里），所以文案要指路而不是
            报错；给一个直达插件页的按钮，省得用户自己去左侧菜单找。
            「全部」段同样走这一条：四类都空而插件没装时，那一屏什么都没有。 */}
        {!hit.length && !newCard && !keyword
          && (effectiveType === TYPE_PROXY || effectiveType === TYPE_ALL) ? (
            <Empty className='add-provider-empty'>
              <p className='mb-2'>{t('请安装并启用对应的提供商插件；列表为空时请刷新后重试。')}</p>
              <Button size='default' variant='ghost'
                type='button'
                className='text-primary underline underline-offset-2'
                onClick={() => shared().wbApp?.showPage?.('plugins')}
              >
                {t('去安装提供商插件包')}
              </Button>
            </Empty>
          ) : null}
      </div>
    </AddGroup>
  )
}
