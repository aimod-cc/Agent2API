import * as React from 'react'
import { flushSync } from 'react-dom'
import { createRoot, type Root } from 'react-dom/client'
import { t } from '../i18n'

/**
 * Agent2API · 请求日志「重试 / 敏感词」两枚标签的悬停面板（React 岛）。
 *
 * 替换 ui/request-hover.js（旧文件由迁移负责人删除）。对外接口与原实现**完全一致**：
 *   window.wbRequestHover = { bind, close, hasProcessFacts, beforeListRedraw, afterListRedraw }
 * 调用点只有 ui/requests-panel.js（bind 在加载期把委托挂到列表上、hasProcessFacts 在 retryCell
 * 决定那枚「重试」标签显不显示、before/afterListRedraw 在 paintList 通知整表重绘），一行都不用改。
 *
 * ── 这两枚标签承载的「逐请求事实」（文案与分组口径的唯一去处）──────
 * 改造前，一次转发的过程事实散在运行日志里（换号顺延、退避重试、401 刷新、限额降级、上游报错、
 * 代理回退各自一行，有的按请求刷屏），请求日志这边只看得到「换了几次号」与最后一次的结果。现在
 * 这些事实全部收进尝试明细（`attemptDetails` 的 `account` / `retries` / `notice`），由本模块渲染
 * —— 排障时**只看这一处**就够：谁承载了每一轮（提供商 + 账号）、每轮成没成与失败原因、每轮内部
 * 退避重试的次数 / 原因 / 等待时长、出口有没有降级（代理不可用 → 直连，落在提示行 notice 上）。
 * 所以这里的**文案、分组与上限**（MAX_TERM_ROWS 等）一律照抄旧实现：它是排障的唯一口径，多一套
 * 说法就等于有两个事实源。
 *
 * ── 形态：常驻的委托（命令式）+ 按需的浮层（React）────────────────
 *   · 常驻部分本次**不动**：`bind(list)` 把事件委托挂在列表**容器**上。宿主是 legacy 渲染的表格
 *     DOM（requests-panel.js 默认 1 秒一拍、每次整表重绘 `innerHTML`），委托挂在容器上不随重绘
 *     失效；而 React 管不到那些节点，契约 `bind({host, entryOf})` 收的本来就是一个现成元素 ——
 *     这一半留在命令式一侧是必然的。
 *   · 浮层部分（本次换掉）：内容改由 React 渲染。打开时就地建宿主 div 挂到 body、createRoot 渲染，
 *     量完尺寸落位；收起时 `root.unmount()` + `host.remove()`，不留常驻节点（旧实现是一个常驻
 *     body 的 div + innerHTML 拼内容）。
 *
 * ── 为什么外壳自绘（Tailwind）而不是用组件库的浮层件 ──────────────
 * ui-kit 里没有 Popover / 非模态浮层件：Dialog 与 AlertDialog 都是**模态**（遮罩、焦点陷阱、滚动
 * 锁定），而这里要的恰好相反 —— 跟着鼠标、不抢焦点、连指针事件都不接收（旧 `.req-hover` 是
 * `pointer-events: none`）的只读说明。所以外壳按旧 CSS 的取值映射组件库令牌自绘（见 SHELL_CLASS），
 * 并把「缺一个非模态 Popover」反馈给 ui-kit。面板内容是纯文本结构（分组 / 列表 / 键值对），没有
 * 按钮也没有徽章 —— 因此这一版**没有**用到 Button / Badge：把「× N」这类计数换成徽章会改掉旧设计
 * 刻意的「词左次数右、次数竖着能比大小」两列排布，属于无谓的观感回归。
 *
 * ── 定位与整表重绘的锚点迁移（两条不能丢的行为）─────────────────
 * 定位照抄旧算法（不引入浮层库 / 拖拽库）：面板 `position: fixed` 挂 body（`.panel` 是
 * overflow:hidden，absolute 的浮层会被卡片裁掉），用 max-content 量宽 → 夹进下限与两个上限 →
 * 默认贴锚点下方、下方放不下且上方更宽裕时翻到上方 → 贴边夹进视口。关键在时序：渲染、量尺寸、
 * 落位必须**在同一个任务内**完成，否则会闪出一帧未定位的面板（旧实现是 innerHTML + place() 同
 * 任务）。React 19 的 root.render 默认并发调度（提交可能落在下一个宏任务），所以这里用 `flushSync`
 * 把提交压成同步 —— 这是与旧实现等价的时序保证，不是可选项。
 *
 * 锚点迁移：本页 1 秒重绘一次、重绘换掉全部标签节点，而打开有 150ms 延迟 —— 重绘落在延迟窗口里时
 * 面板会拿**游离节点**定位（rect 全 0 → 面板落在视口左上角），且游离节点收不到 pointerout、开了就
 * 不会自己关（实测的 bug）。宿主在替换 innerHTML 前后各通知一次，本模块按「标签种类 + 行身份键」
 * 把锚点迁到新节点上；迁不走（行被挤出当前页 / 反查不到数据）一律收起，于是不会留下死锚点。
 *
 * ── 与旧实现的两处显式差异 ───────────────────────────────────
 *   ① 全局监听（click 捕获 / scroll 捕获 / resize）只在面板开着时绑、收起即解 —— 旧实现是加载期
 *      常驻三条。行为等价（关着的时候它们本来就什么都不做），但不留悬挂监听。
 *   ② 深色样式走组件库的 `dark:` 变体（跟 data-theme 走），旧实现的
 *      `@media (prefers-color-scheme: dark)` 不认 data-theme，会在「显式浅色主题 + 深色系统」下
 *      把面板刷成深色。
 */

/* ─── 常量 ───────────────────────────────────── */
/** 悬停进入 / 离开的延迟（ms）：与 tooltip.js 同一组值，手感一致 */
const SHOW_DELAY = 150
const HIDE_DELAY = 80
/** 距视口边缘的安全距离，以及浮层与锚点的间距 */
const EDGE = 8
const GAP = 8
/**
 * 面板宽度的下限（px）：内容再短也至少这么宽。重试链面板最长的一行是「尝试 N · 提供商
 * 账号 → 失败（状态码）：错误摘要」，头部加常见长度的摘要就有 400–600px —— 下限给足，
 * 常见场景整行显示。敏感词面板是「词 × 次数」列表，内容不长，基准宽度给小的（铺太宽会
 * 让词与次数隔得老远，反而难读）。
 */
const MIN_PANEL_WIDTH = 520
const MIN_PANEL_WIDTH_SENSITIVE = 220
/**
 * 敏感词面板在量出宽度上的宽裕系数（1.1 = 放宽一成）：量出的 max-content 恰恰好「放得下
 * 最长一行」，系统缩放非整数倍时（Windows 125% / 150%）文字实际排布会宽零点几像素，长敏感词
 * 就在最后一个词处折行，而面板右边还空着一截。重试链面板不加：那几行是长文本流，宽窄只影响
 * 换行位置。
 */
const SENSITIVE_WIDTH_SLACK = 1.1
/** 面板宽度的上限（px，还要再夹进视口可用宽度）：铺满整屏的一行 12px 字很难读，超出的交给内部换行 */
const MAX_PANEL_WIDTH = 900
/** 命中词列表最多显示几行：这一块是「命中了什么」的快照，不是词表编辑器 */
const MAX_TERM_ROWS = 12
/** 面板节点 id：锚点用 aria-describedby 指过来（与旧实现同一个 id） */
const PANEL_ID = 'req-hover-panel'

/**
 * 面板外壳的类名。定位那几条（position / left / top / width / max-width）**不在这里**：它们要
 * 量完尺寸才知道，由 place() 写成内联样式 —— 也不该由 React 管，否则每次重渲染都可能把落位
 * 结果推回原样。
 *
 * 逐条对应旧 `.req-hover`（ui/css/page-requests.css），取值映射到组件库令牌：z-index 35 /
 * max-height min(360px,60vh) / padding 10px 12px / overflow / 12px + 1.6 行高 /
 * pointer-events:none / word-break:normal；radius --r-md ↔ rounded-md（两边都由 --radius: 1rem
 * 派生，值相同）、bg --surface 96% ↔ bg-surface-2（旧值是把 --surface 压暗 4% 的抬升面，
 * 组件库最接近的是 surface-2；深色下旧值就是 --surface-3，两边同名同值）、color --text ↔
 * text-foreground、--shadow-3 ↔ shadow-3；深色下 --surface-3 + --border-strong 描边。
 *
 * `pointer-events-none` 是行为的一部分：面板不接收指针事件，指针落到它上面时命中的仍是下面的
 * 标签/行 —— 于是「移到面板上」等价于「移开标签」，按 HIDE_DELAY 收起，这正是旧实现「面板不可
 * 交互」的语义（也因此这里没有任何可点控件）。
 */
const SHELL_CLASS = [
  'fixed z-[35] pointer-events-none',
  'max-h-[min(360px,60vh)] overflow-x-hidden overflow-y-auto',
  'rounded-md p-[10px_12px] text-[12px] leading-[1.6] break-normal',
  'bg-surface-2 text-foreground shadow-3',
  'border-0 dark:border dark:border-border-strong dark:bg-surface-3',
].join(' ')

/** 淡色说明行的类（旧 `.rh-row` + `.rh-dim` 的组合，出现多处，口径只写一处） */
const DIM_ROW = 'rh-row rh-dim min-w-0 break-words text-muted-foreground'
/** 尝试行 / 重试子行共用的「文本流连排」底（旧 `.rh-row`，见 page-requests.css 的说明） */
const FLOW_ROW = 'rh-row min-w-0 break-words'

/* ─── 类型 ───────────────────────────────────── */
/**
 * 请求日志条目：本文件只用下面这几个字段，值一律按 unknown 收（后端可能给数字 / 字符串 /
 * null），读取处各自归一 —— 与旧实现 `Number(x) || 1` 的手法一致。
 */
type RequestEntry = {
  attempts?: unknown
  provider?: unknown
  status?: unknown
  error?: unknown
  attemptDetails?: unknown
  sensitiveHits?: unknown
}

/** 一次尝试的明细（后端 AttemptDetail；字段名是后端契约） */
type AttemptDetail = {
  provider?: unknown
  account?: unknown
  status?: unknown
  error?: unknown
  notice?: unknown
  retries?: unknown
}

/** 一轮内部的一次退避重试 */
type AttemptRetry = { reason?: unknown; status?: unknown; delayMs?: unknown }

/** 敏感词命中明细（后端 SensitiveHit） */
type SensitiveHit = { word?: unknown; count?: unknown }

/**
 * 面板类型：取自标签的 `data-req-hover` —— 只有 'sensitive' 是特例，其余（含空值）
 * 一律按重试链面板渲染（与旧实现 `=== 'sensitive' ? sensitive : chain` 同判据）。
 */
type PanelKind = 'chain' | 'sensitive'

/** 重绘期间的锚点处置意图；`null` = 没有重绘在进行 */
type Remap = { action: 'move'; kind: string; key: string } | { action: 'close' }

/** bind 的参数：宿主列表容器 + 「标签 → 数据」的反查函数（见 bind 的说明） */
type BindOptions = {
  host?: HTMLElement | null
  entryOf?: (tag: HTMLElement) => unknown
}

/**
 * window 上由其它脚本挂载的共享桥。
 *
 * 刻意用「局部窄类型 + 转型」而不是 declare global：wbApp / wbProviders 是多个岛共用的桥，
 * 各岛各 declare 一份会因同名属性类型不一致直接报 TS2717。本文件只用 declare global 声明自己
 * 独占的 wbRequestHover（见文件末尾）。
 */
type SharedWindow = {
  /** 提供商展示名目录（providerLabel 的兜底链：目录 → 原样回显 id） */
  wbProviders?: { labelOf?: (id: string) => string }
  /** 旧实现从 wbApp 取 `esc` 拼 innerHTML，本岛**不使用**（React 的文本节点天然转义，再 esc
   *  一次会把 `&` 显示成 `&amp;`）。列出来只为把依赖面写清楚。 */
  wbApp?: { esc?: (value: unknown) => string }
}

function shared(): SharedWindow {
  return window as unknown as SharedWindow
}

/* ─── 数据读取与判据（与后端字段名一一对应）───── */
/** 普通对象 / 数组都按对象读（旧实现是 `if (!entry) return`），其余给 null */
function asEntry(value: unknown): RequestEntry | null {
  return typeof value === 'object' && value !== null ? (value as RequestEntry) : null
}

/** 一次尝试的明细数组（后端字段是 attemptDetails；旧行没有该键 → 空表） */
const detailsOf = (entry: RequestEntry | null | undefined): AttemptDetail[] =>
  Array.isArray(entry?.attemptDetails) ? (entry.attemptDetails as AttemptDetail[]) : []

/** 一次尝试内部的退避重试数组（旧明细没有该键 → 空表） */
const retriesOf = (item: AttemptDetail | null | undefined): AttemptRetry[] =>
  Array.isArray(item?.retries) ? (item.retries as AttemptRetry[]) : []

/** 敏感词命中明细（后端字段是 sensitiveHits；旧行没有该键 → 空表） */
const hitsOf = (entry: RequestEntry | null | undefined): SensitiveHit[] =>
  Array.isArray(entry?.sensitiveHits) ? (entry.sensitiveHits as SensitiveHit[]) : []

/**
 * provider id → 展示名，查不到就原样回显 id。
 *
 * ⚠️ 旧实现的注释说这里是「后端 label → 前端 providers 目录 → 原样回显 id」三级兜底、
 * 与请求日志的「提供商」列（requests-panel.js 的 targetCell）逐字同源，但**代码只实现了
 * 两级**（读 `wbProviders.labelOf` + 回显，没读明细里的 providerLabel）。本次迁移照抄
 * 代码行为、不改口径；要不要补第一级请迁移负责人定（补法：这里先读 item.providerLabel）。
 */
function providerLabel(id: unknown): string {
  const key = String(id ?? '').trim()
  if (!key) return ''
  return shared().wbProviders?.labelOf?.(key) || key
}

/**
 * 这条请求是否有**值得展示的过程事实**（决定那枚「重试」标签显不显示）。
 *
 * 判据为什么不是 `attempts > 1`：`attempts` 只数**账号轮换**（口径见后端
 * `TelemetrySnapshot::attempts`），同账号内的退避重试（11-128 敏感词拦截、瞬时 5xx、传输层
 * 失败、401 刷新）不计入它 —— 一次被 11-128 拦下、重试 3 次后成功的请求 `attempts` 仍是 1，
 * 旧口径下这类请求在列表里连标签都不出现。所以判据是「换过号 **或** 重试过 **或** 有过提示」。
 *
 * 这是唯一一个**给宿主用的判断**（requests-panel.js 的 retryCell 调它）：判据要读明细内部的
 * 字段，让宿主自己拼一份就会有两份判据。敏感词那枚标签不在这里判 —— 它是同一列里另一枚标签
 *（`sensitiveHits`）的事，两者各自独立出现。
 */
function hasProcessFacts(entry: unknown): boolean {
  const row = asEntry(entry)
  if ((Number(row?.attempts) || 1) > 1) return true
  return detailsOf(row).some(item => retriesOf(item).length > 0 || Boolean(item?.notice))
}

/** 标签 → 面板类型（见 PanelKind 的说明） */
function kindOfTag(tag: HTMLElement): PanelKind {
  return tag.dataset.reqHover === 'sensitive' ? 'sensitive' : 'chain'
}

/* ─── 面板内容（纯展示，与旧内容构造函数逐字对应）── */
/**
 * 切换路径行：`A → B → C`。**只在真实发生过 ≥2 次尝试时显示**：只有一次尝试时那串
 * 箭头就是「A → 成功」，没有信息量；而这一列的标签本来就只在有过程事实时出现，所以
 * 这条判据主要是防「明细比 attempts 短」的边界（截断、或部分尝试没采到）。
 */
function ChainLine({ details }: { details: AttemptDetail[] }) {
  if (details.length < 2) return null
  const names = details.map(item => providerLabel(item.provider) || t('未知'))
  return (
    <div className='rh-chain mb-1.5 border-b border-border pb-1.5 break-words'>
      <span className='rh-chain-k font-semibold text-muted-foreground'>{t('切换路径：')}</span>
      <span className='rh-chain-v font-semibold text-foreground'>{names.join(' → ')}</span>
    </div>
  )
}

/**
 * 一次尝试内部的退避重试子行：`↻ 重试 N 次` + 逐条原因。
 *
 * 挂在所属的那一行尝试下面（缩进 + 左侧竖线），而不是作为并列的尝试行 —— 它们是
 * **同一轮账号内**的重发（换的是时间不是账号），并列会让「切换路径」那串箭头里混进一串
 * 同名项，把真正的换号链埋掉。逐条文案：`原因（HTTP 状态码，无则省略），X秒后重试`。
 */
function RetryRows({ item }: { item: AttemptDetail }) {
  const retries = retriesOf(item)
  if (!retries.length) return null
  return (
    <div className='rh-retry mt-0.5 mb-1 ml-2.5 border-l-2 border-border-strong pl-2'>
      <div className='rh-retry-head text-[11.5px] font-semibold text-warning'>
        {t('↻ 重试 {n} 次', { n: retries.length })}
      </div>
      {retries.map((retry, index) => {
        const reason = String(retry?.reason ?? '').trim()
        const status = Number(retry?.status)
        const hasStatus = retry?.status !== null && retry?.status !== undefined && Number.isFinite(status)
        const delayMs = Number(retry?.delayMs)
        const delay = Number.isFinite(delayMs) && delayMs > 0
          ? (delayMs >= 1000
              ? t('，{n}秒后重试', { n: Math.round(delayMs / 1000) })
              : t('，{n}毫秒后重试', { n: Math.round(delayMs) }))
          : ''
        return (
          <div key={index} className='rh-retry-row min-w-0 break-words'>
            <span className='rh-retry-why mr-1 text-subtle [overflow-wrap:anywhere]'>
              {reason || t('未知原因')}
            </span>
            {hasStatus ? (
              <span className='rh-retry-status mr-1 text-muted-foreground tabular-nums whitespace-nowrap'>
                {`HTTP ${String(status)}`}
              </span>
            ) : null}
            {delay ? <span className='rh-dim text-muted-foreground'>{delay}</span> : null}
          </div>
        )
      })}
    </div>
  )
}

/**
 * 单次尝试的一行：`尝试 N · 提供商（账号）→ 成功(200) / 失败(500)：错误摘要`。
 *
 * 三种结局的判据与颜色：
 *   · `error` 非空         → 失败（红），带状态码（可能没有：传输层失败）
 *   · `status` 有值、无错误 → 成功（绿），带状态码
 *   · 两者都无             → 未定论（淡灰）。两种来源：这一轮还在飞（`inFlight`，进行中行的最后
 *     一条明细 —— 转发一开始就在途回写，所以这是常态），以及被手工改过的库。前者写「进行中…」，
 *     后者才写「无结果记录」：对一条正在跑的请求说「无结果」会被读成它已经失败。
 *
 * 账号名挂在提供商名后面：一家可以有多个账号，「WorkBuddy / aibjchat001@gmail.com」比只有家名
 * 更能定位到那一轮（这些信息改造前只在运行日志里）。
 */
function AttemptRow({ item, index, inFlight }: { item: AttemptDetail; index: number; inFlight: boolean }) {
  const name = providerLabel(item.provider)
  const account = String(item.account ?? '').trim()
  const status = Number(item.status)
  const hasStatus = item.status !== null && item.status !== undefined && Number.isFinite(status)
  const error = item.error ? String(item.error) : ''
  const notice = String(item.notice ?? '').trim()
  return (
    <>
      <div className={FLOW_ROW}>
        {/* 序号从 1 起（后端明细数组本身就是发生顺序，不需要另存 attempt_no） */}
        <span className='rh-no mr-1 text-muted-foreground'>{t('尝试 {n} ·', { n: index + 1 })}</span>
        <span className='rh-who mr-1 font-semibold text-foreground'>{name || t('未知')}</span>
        {account ? (
          <span className='rh-account mr-1 text-muted-foreground [overflow-wrap:anywhere]'>{account}</span>
        ) : null}
        <span className='rh-arrow mr-1 text-muted-foreground'>→</span>
        {error ? (
          <span className='rh-bad font-medium text-destructive [overflow-wrap:anywhere]'>
            {hasStatus
              ? t('失败（{status}）：{error}', { status: String(status), error })
              : t('失败：{error}', { error })}
          </span>
        ) : hasStatus ? (
          <span className='rh-ok font-medium text-success'>{t('成功（{status}）', { status: String(status) })}</span>
        ) : inFlight ? (
          <span className='rh-dim text-muted-foreground'>{t('进行中…')}</span>
        ) : (
          <span className='rh-dim text-muted-foreground'>{t('无结果记录')}</span>
        )}
      </div>
      {/* 提示行（代理回退直连等）：非失败、但值得记一笔 */}
      {notice ? (
        <div className='rh-notice mt-0.5 mb-1 ml-2.5 border-l-2 border-border-strong pl-2 text-warning [overflow-wrap:anywhere]'>
          {`⚠️ ${notice}`}
        </div>
      ) : null}
      <RetryRows item={item} />
    </>
  )
}

/**
 * 重试面板的内容。两种形态：
 *   ① 有明细：切换路径 + 每次尝试。
 *   ② 没有明细（旧数据 / 转发前就失败）：给一句「共 N 次尝试，最终由 X 承载」—— 明细确实拿不到，
 *      但已有的两个读数（次数、最终承载者）仍然值得显示。**不编造中间过程**：那会是猜。
 * 进行中行也走①：转发一开始就在途回写，最后一条的 status / error 都为空 —— 那是「这一轮还在飞」，
 * 由 AttemptRow 的 inFlight 渲染成「进行中…」。
 */
function ChainPanel({ entry }: { entry: RequestEntry }) {
  const attempts = Number(entry.attempts) || 1
  const details = detailsOf(entry)
  if (!details.length) {
    const finalProvider = providerLabel(entry.provider)
    const sentence = finalProvider
      ? t('共 {n} 次尝试，最终由 {provider} 承载', { n: attempts, provider: finalProvider })
      : t('共 {n} 次尝试', { n: attempts })
    return (
      <>
        <div className={DIM_ROW}>{sentence}</div>
        <div className={DIM_ROW}>
          {t('这条记录的尝试明细未采集（该字段上线前的旧数据，或请求在转发前就失败）')}
        </div>
      </>
    )
  }
  // 明细被体积闸截断时如实说明（保头：留下的是最早那几轮）。判据是「明细比 attempts
  // 少」，与后端的 MAX_ATTEMPT_DETAILS 无关 —— 上限值改了这条提示仍然成立。
  // 「还在飞」只可能是最后一条：尝试是严格串行的，前面的轮次一旦定局就不再变化。
  const running = (Number(entry.status) || 0) === 0 && !entry.error
  return (
    <>
      <ChainLine details={details} />
      {details.map((item, index) => (
        <AttemptRow
          key={index}
          item={item}
          index={index}
          inFlight={running && index === details.length - 1}
        />
      ))}
      {details.length < attempts ? (
        <div className={DIM_ROW}>
          {t('另有 {missing} 次尝试未记录明细（只保留最早的 {kept} 条）', {
            missing: attempts - details.length, kept: details.length,
          })}
        </div>
      ) : null}
    </>
  )
}

/**
 * 敏感词面板的内容：标题 + 「词 × 次数」列表。没有命中明细（只有布尔事实的旧数据）时给一句如实
 * 说明 —— 标签本身由「命中表非空」驱动，这条分支在正常数据下走不到；它兜住的是「有人只改了
 * attempts 之外的字段」那种手工改动。
 */
function SensitivePanel({ entry }: { entry: RequestEntry }) {
  const hits = hitsOf(entry)
  if (!hits.length) {
    return <div className={DIM_ROW}>{t('这条记录命中了敏感词，但没有留下命中明细')}</div>
  }
  // 后端已按次数降序给出，这里不重排（顺序定义只有一处）
  return (
    <>
      <div className='rh-title mb-1 text-[11.5px] font-semibold tracking-[.02em] text-muted-foreground'>
        {t('命中的敏感词')}
      </div>
      {hits.slice(0, MAX_TERM_ROWS).map((hit, index) => (
        <div key={index} className='rh-row rh-hit flex items-baseline justify-between gap-2.5'>
          <span className='rh-hit-w break-words text-foreground'>{String(hit?.word ?? '')}</span>
          <span className='rh-hit-c text-muted-foreground tabular-nums whitespace-nowrap'>
            {`× ${String(Number(hit?.count) || 0)}`}
          </span>
        </div>
      ))}
      {hits.length > MAX_TERM_ROWS ? (
        <div className={DIM_ROW}>{t('另有 {n} 个词命中', { n: hits.length - MAX_TERM_ROWS })}</div>
      ) : null}
    </>
  )
}

/**
 * 面板根：外壳（外观 + 内容）在这里，内容按 kind 分派。`panelRef` 由命令式外壳在挂载时现建：
 * place() 要拿真实节点量尺寸，而节点由 React 创建 —— 一个 ref 就够，不必 portal 或全局查询。
 */
function HoverPanel({
  kind,
  entry,
  panelRef,
}: {
  kind: PanelKind
  entry: RequestEntry
  panelRef: React.RefObject<HTMLDivElement | null>
}) {
  return (
    <div ref={panelRef} id={PANEL_ID} role='tooltip' data-kind={kind} className={SHELL_CLASS}>
      {kind === 'sensitive' ? <SensitivePanel entry={entry} /> : <ChainPanel entry={entry} />}
    </div>
  )
}

/* ─── 模块状态 ───────────────────────────────── */
/** 当前挂着的标签 */
let anchor: HTMLElement | null = null
let showTimer = 0
let hideTimer = 0
/**
 * 延迟窗口里排着打开的那枚标签（`showTimer` 的排队目标）。需要单独记：定时器排上时 `anchor` 还是
 * null（面板还没开），而 pointerout 的取消判据此前只看 `anchor` —— 于是「悬一下就走」照样会弹出，
 * 列表重绘把标签删掉时浏览器补发的 pointerout 也拦不住这个定时器。
 */
let pendingTag: HTMLElement | null = null

/** 宿主列表与「标签 → 数据」的反查函数（bind 时记下；整表重绘后找回锚点要用） */
let listHost: HTMLElement | null = null
let entryOfFn: ((tag: HTMLElement) => unknown) | null = null

/** 重绘期间的锚点处置意图（见 beforeListRedraw / afterListRedraw） */
let remap: Remap | null = null

/** 浮层的宿主 div / React root / 面板节点引用 / 当前面板类型（收起时全部清空） */
let panelHost: HTMLElement | null = null
let panelRoot: Root | null = null
let panelRef: React.RefObject<HTMLDivElement | null> | null = null
let panelKind: PanelKind = 'chain'

function cancelTimers(): void {
  window.clearTimeout(showTimer)
  window.clearTimeout(hideTimer)
  showTimer = 0
  hideTimer = 0
  pendingTag = null
}

/* ─── 定位 ───────────────────────────────────── */
/**
 * 按锚点位置摆面板：默认下方，下方放不下且上方更宽裕时翻到上方。
 *
 * 宽度按内容自适应再夹进视口：切换路径那行可能很长（三家的中文名 + 箭头），错误摘要更长。用
 * max-content 量出理想宽度，再夹到下限（MIN_PANEL_WIDTH）与两个上限（视口可用宽度、
 * MAX_PANEL_WIDTH），放不下的交给面板内部的换行与滚动；敏感词面板另外放宽一成（见
 * SENSITIVE_WIDTH_SLACK）并把宽度**直接定到**上限 —— max-width 只是上限，内容比它窄时面板仍
 * 会缩回 max-content，放宽的一成就落不到实处。
 *
 * 三步（量自然宽 → 定宽 → 量高定坐标）在同一任务内跑完，浏览器只绘制最终结果，不会闪出「先落在
 * 上一处」的一帧（与 tooltip.js 同法）。夹进视口后仍放不下（面板比可用高度还高）时贴边显示，
 * 超出部分由 max-height + overflow 接管（见 SHELL_CLASS）。
 */
function place(): void {
  const panel = panelRef?.current
  const tag = anchor
  if (!panel || !tag) return

  const rect = tag.getBoundingClientRect()
  const avail = window.innerWidth - EDGE * 2

  panel.style.maxWidth = 'none'
  panel.style.width = 'max-content'
  const natural = panel.offsetWidth
  const sensitive = panelKind === 'sensitive'
  const minWidth = sensitive ? MIN_PANEL_WIDTH_SENSITIVE : MIN_PANEL_WIDTH
  const base = Math.max(minWidth, natural) * (sensitive ? SENSITIVE_WIDTH_SLACK : 1)
  const cap = Math.min(base, avail, MAX_PANEL_WIDTH)
  panel.style.maxWidth = `${cap}px`
  panel.style.width = sensitive ? `${cap}px` : 'auto'

  const width = panel.offsetWidth
  const height = panel.offsetHeight
  const below = window.innerHeight - rect.bottom - GAP - EDGE
  const above = rect.top - GAP - EDGE
  const flip = below < height && above > below

  const centerX = rect.left + rect.width / 2
  const left = Math.max(EDGE, Math.min(centerX - width / 2, window.innerWidth - EDGE - width))
  const top = flip ? rect.top - GAP - height : rect.bottom + GAP

  panel.style.left = `${Math.round(left)}px`
  panel.style.top = `${Math.round(Math.max(EDGE, Math.min(top, window.innerHeight - EDGE - height)))}px`
}

/* ─── 全局监听（按需绑 / 收起即解）────────────── */
/** 点标签之外的任何地方收起（捕获阶段：先于业务自己的 click，保证「关面板」不影响页面本来的点击） */
function onDocumentClick(event: MouseEvent): void {
  if (!anchor) return
  const target = event.target
  if (!(target instanceof Node)) return
  // 点在锚点自身或它内部不算「点外面」（标签是 button，点它不该把面板关掉）
  if (target === anchor || anchor.contains(target)) return
  closePanel()
}

/**
 * 滚动 / 缩放时**收起**而不是重新定位（与 tooltip.js 相反）：面板挂在列表内的行上，而列表自身可滚
 *（`.log-list`），滚动时行在动、面板是 fixed 的，追随会让面板在屏幕上拖着走、干扰正在滚动的手。
 * 收起之后指针还停在标签上，轻微一动就会重新弹出。
 */
function onScroll(): void {
  closePanel()
}

function onResize(): void {
  closePanel()
}

let globalsBound = false

function bindGlobals(): void {
  if (globalsBound) return
  globalsBound = true
  document.addEventListener('click', onDocumentClick, true)
  window.addEventListener('scroll', onScroll, true)
  window.addEventListener('resize', onResize)
}

function unbindGlobals(): void {
  if (!globalsBound) return
  globalsBound = false
  document.removeEventListener('click', onDocumentClick, true)
  window.removeEventListener('scroll', onScroll, true)
  window.removeEventListener('resize', onResize)
}

/* ─── 面板生命周期（按需建、收起即卸）────────── */
/** 拆掉宿主与 root（收起时唯一的出口，保证不留宿主 div、不留全局监听） */
function unmountPanel(): void {
  unbindGlobals()
  if (panelRoot) {
    panelRoot.unmount()
    panelRoot = null
  }
  if (panelHost) {
    panelHost.remove()
    panelHost = null
  }
  panelRef = null
}

/**
 * 把面板内容渲染出来并立刻落位。
 *
 * `flushSync` 是时序的关键：React 19 的 `root.render` 默认并发调度，提交可能落在下一个宏任务 ——
 * 那样「渲染 → 量尺寸 → 落位」就跨了任务，中间会闪出一帧未定位的面板。旧实现是 `innerHTML` +
 * place() 同任务，这里必须等价，所以把提交压成同步。
 *
 * 复用时直接重渲染（锚点迁移路径：内容一并重算，而不是沿用上一拍的旧内容 —— 进行中的行每秒都在变），
 * 宿主与 root 留着；首次打开才建宿主 div 挂到 body（同旧实现）。
 */
function renderPanel(kind: PanelKind, entry: RequestEntry): void {
  panelKind = kind
  if (!panelHost || !panelRoot || !panelRef) {
    panelHost = document.createElement('div')
    document.body.append(panelHost)
    panelRef = React.createRef<HTMLDivElement>()
    panelRoot = createRoot(panelHost)
  }
  const root = panelRoot
  const ref = panelRef
  flushSync(() => {
    root.render(<HoverPanel kind={kind} entry={entry} panelRef={ref} />)
  })
  bindGlobals()
  place()
}

/** 展开锚点的面板（数据由 entryOf 反查；反查不到就什么都不显示） */
function openPanel(el: HTMLElement): void {
  // 锚点必须还在文档里：整表重绘会把标签换成新节点，而 150ms 延迟窗口里排下的这次打开
  // 拿到的可能正是被删掉的旧节点 —— 游离节点 rect 全 0（面板会落在视口左上角），且它收不到
  // pointerout、开了就不会自己关。作废这次打开即可：指针还在标签上的话，补发的 pointerover
  // 会重新排一次。
  if (!el.isConnected) return
  const entry = asEntry(entryOfFn?.(el))
  // 反查不到数据（列表在打开前恰好刷新过）：不显示（旧实现返回空 HTML 时直接早退）
  if (!entry) return
  cancelTimers()
  if (anchor && anchor !== el) closePanel()
  anchor = el
  renderPanel(kindOfTag(el), entry)
  el.classList.add('active')
  el.setAttribute('aria-describedby', PANEL_ID)
}

function closePanel(): void {
  cancelTimers()
  if (anchor) {
    anchor.classList.remove('active')
    anchor.removeAttribute('aria-describedby')
    anchor = null
  }
  unmountPanel()
}

/* ─── 整表重绘时的锚点迁移 ───────────────────── */
/**
 * 列表整表重绘**前**调用（宿主在替换 `innerHTML` 之前）：定下锚点的处置意图。
 *
 * 只有「指针正悬着」的锚点才迁移（鼠标用户在看面板，面板该跟着新一屏走），身份用「标签种类 + 行
 * 身份键」（`data-req-hover` 与 `data-req-id`）记下 —— 用属性而不是节点引用：整表重绘必然换节点，
 * 引用一定会失效。其余情况一律记为收起，理由各是一条独立边界：
 *   · `hideTimer` 排着 = 指针已移开、正等 HIDE_DELAY 收起 —— 迁移后那个定时器的判据
 *     （`anchor === tag`）已不成立，会变成「该关没关」；
 *   · 指针不在而焦点在标签上 = 键盘打开 —— 重绘会删掉焦点元素、焦点回落到 body，面板跟过去就成了
 *     「焦点不在标签上、面板却开着」的错位；
 *   · 身份键缺失 = 这枚标签不是按常规渲染出来的，无从找回。
 */
function beforeListRedraw(): void {
  if (!anchor) return
  const kind = String(anchor.dataset.reqHover || '')
  const key = String(anchor.dataset.reqId || '')
  const movable = Boolean(kind && key) && anchor.matches(':hover') && !hideTimer
  remap = movable ? { action: 'move', kind, key } : { action: 'close' }
}

/**
 * 列表整表重绘**后**调用（宿主在替换 `innerHTML` 之后）：按处置意图收尾 —— 迁移则换锚点、重算内容、
 * 重摆位置（用户看不到闪动）；找不回新节点（行被挤出当前页 / 列表变空 / 出错态 / 反查不到数据）或
 * 意图本就是收起则 closePanel()。
 *
 * 为什么不「重绘即收起」：本页默认 1 秒重绘一次，正盯着面板看的人会被每秒关一次、还要动一下鼠标才
 * 重开。顺带这也是「游离锚点」的彻底解法 —— 迁不走的（找不到新节点的）一律收起。
 */
function afterListRedraw(): void {
  const pending = remap
  remap = null
  if (!pending || !anchor) return
  if (pending.action === 'close') {
    closePanel()
    return
  }
  const next = listHost ? findTag(listHost, pending.kind, pending.key) : null
  // 找不回新节点（行被挤出当前页 / 列表变空 / 出错态）就不再反查数据
  const entry = next ? asEntry(entryOfFn?.(next)) : null
  if (!next || !entry) {
    closePanel()
    return
  }
  if (next === anchor) return // 整表重绘必然换节点；这条兜住宿主改成局部更新后误伤
  anchor.classList.remove('active')
  anchor.removeAttribute('aria-describedby')
  anchor = next
  renderPanel(kindOfTag(next), entry)
  next.classList.add('active')
  next.setAttribute('aria-describedby', PANEL_ID)
}

/** 在新 DOM 里按「标签种类 + 行身份键」找回锚点；没有就是这一行不在本屏了 */
function findTag(host: HTMLElement, kind: string, key: string): HTMLElement | null {
  // 用遍历而不是拼属性选择器：身份键来自后端（id 或时间戳），不必让它参与选择器解析
  // —— 转义漏一个字符就是一个静默查不到。一行最多两枚标签，50 行的遍历对每 1 秒一次
  // 的重绘完全无感。
  for (const el of host.querySelectorAll<HTMLElement>('[data-req-hover]')) {
    if (String(el.dataset.reqHover || '') === kind && String(el.dataset.reqId || '') === key) return el
  }
  return null
}

/* ─── 事件委托（常驻部分，宿主是 legacy 列表 DOM）── */
function tagFromEvent(event: Event): HTMLElement | null {
  const target = event.target
  if (!(target instanceof Element)) return null
  return target.closest<HTMLElement>('[data-req-hover]')
}

function onPointerOver(event: PointerEvent): void {
  const tag = tagFromEvent(event)
  if (!tag || !listHost?.contains(tag)) return
  cancelTimers()
  if (anchor === tag && panelRoot) return
  // 已经开着别的面板时立即切换（用户在连着看），否则等满延迟防误触
  const delay = anchor ? 0 : SHOW_DELAY
  if (anchor) closePanel()
  pendingTag = tag
  showTimer = window.setTimeout(() => {
    showTimer = 0
    pendingTag = null
    openPanel(tag)
  }, delay)
}

function onPointerOut(event: PointerEvent): void {
  const tag = tagFromEvent(event)
  if (!tag) return
  // ① 还在延迟窗口里（面板没开、anchor 为 null）：取消排着的那次打开 ——
  //    「悬一下就走」不该弹出。列表重绘把标签删掉时浏览器补发的 pointerout 也走这里：
  //    不取消的话，定时器到点会拿游离节点去 openPanel（见那里的判据）
  if (pendingTag === tag) {
    window.clearTimeout(showTimer)
    showTimer = 0
    pendingTag = null
    return
  }
  // ② 面板开着且锚的就是它：按 HIDE_DELAY 收起
  if (anchor !== tag) return
  window.clearTimeout(showTimer)
  showTimer = 0
  hideTimer = window.setTimeout(() => {
    hideTimer = 0
    if (anchor === tag) closePanel()
  }, HIDE_DELAY)
}

function onFocusIn(event: FocusEvent): void {
  // 键盘可达：标签是 button（见 requests-panel.js 的 retryCell），聚焦即展开
  const tag = tagFromEvent(event)
  if (tag) openPanel(tag)
}

function onFocusOut(event: FocusEvent): void {
  const tag = tagFromEvent(event)
  if (tag && anchor === tag) closePanel()
}

function onKeyDown(event: KeyboardEvent): void {
  // Esc 收起（不拦冒泡：弹窗自己也要用 Esc）。与旧实现同处 —— 挂在宿主列表上，
  // 所以生效范围是「焦点在列表里」（聚焦标签即打开，此时按 Esc 一定命中这里）。
  if (event.key !== 'Escape' || !anchor) return
  closePanel()
}

/**
 * 绑定宿主列表：一次委托覆盖所有行，整表重绘后监听仍然有效。`entryOf(el)` 由宿主提供：收一个标签
 * 元素，返回它所属的那条请求日志。反查而不是把数据塞进 `data-*`：一次尝试明细可达 24 条、错误摘要
 * 200 字符，每页 50 行就是几百 KB 的属性文本 —— 那会拖慢整表重绘，而数据本来就在宿主的 `entries`
 * 里（按行号反查是常数时间，且列表重绘时标签与数据同生共死，不存在「标签还在、数据换了」的错位）。
 */
function bind(options?: BindOptions): void {
  const host = options?.host
  if (!host) return
  // 记下宿主与反查函数：整表重绘后按身份找回锚点要用（见 afterListRedraw）
  listHost = host
  entryOfFn = options?.entryOf ?? null
  host.addEventListener('pointerover', onPointerOver)
  host.addEventListener('pointerout', onPointerOut)
  host.addEventListener('focusin', onFocusIn)
  host.addEventListener('focusout', onFocusOut)
  host.addEventListener('keydown', onKeyDown)
}

/* ─── 注册 ───────────────────────────────────── */
/**
 * 对外契约（替换 ui/request-hover.js，接口与原实现一致）。只导出宿主需要的入口；内容构造函数保持
 * 私有 —— 宿主要显示什么，由标签上的 `data-req-hover` 决定，不必也不该由它拼 HTML。
 */
type RequestHoverApi = {
  /** 把委托挂到列表容器上（宿主加载期调一次；后续整表重绘不影响它） */
  bind(options?: BindOptions): void
  /** 收起面板并拆掉宿主 */
  close(): void
  /** 「重试」标签该不该出现 —— 判据在数据明细里，由本模块回答（见 hasProcessFacts） */
  hasProcessFacts(entry: unknown): boolean
  /** 整表重绘前：定下锚点的处置意图（迁移 / 收起） */
  beforeListRedraw(): void
  /** 整表重绘后：迁移锚点并重算内容，迁不走就收起 */
  afterListRedraw(): void
}

declare global {
  interface Window {
    /** 请求日志两枚标签的悬停面板（替换 ui/request-hover.js，接口与原实现一致） */
    wbRequestHover?: RequestHoverApi
  }
}

window.wbRequestHover = { bind, close: closePanel, hasProcessFacts, beforeListRedraw, afterListRedraw }
