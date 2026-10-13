/**
 * Agent2API · 「测试模型」两层弹窗 —— React 岛。
 *
 * 入口在**模型管理页操作列**那颗「测试」（models-page.tsx 的 `case 'act'`）。入口只开这一处：
 * 账号页一行是登录态、不携带模型清单，从那边发起测试要再造一个模型选择器，而账号页操作列已经
 * 是六项；账号维度由本弹窗的「测试账号」多选覆盖 —— 那是同一个动作的另一个参数，不是另一件事。
 *
 * ── 两层：参数在下、结果在上 ──────────────────────────────────
 *   1. **参数层**（外层 Dialog）：目标行 + 系统提示词 / 用户提示词 / 思考等级 / 流式 / 测试账号。
 *      默认值刻意取「最小请求」——用户提示词「你好」、流式开、思考等级跟随映射；
 *   2. **结果层**（内层 Dialog，按账号分块）：测试中 / 一成一败 / 全部失败三态都住在这一层。
 *
 * 内层是**嵌在外层 DialogContent 的 children 里**的（照 add-account-modal.tsx 的双层形态）：
 * React 父子关系是 Base UI 认「嵌套弹窗」的判据，内层开着时外层的 Esc 与遮罩按压才不生效；
 * 位置也必须是外层内容的子树，内层 Portal 才会排在父层 Popup 之后、落在上面。内层要给
 * `overlayForceRender` —— 它要压暗下层（含参数弹窗），而「点空白处关掉这一层」认的正是遮罩本身。
 * 关掉结果层 = 回到参数层继续改（参数不重填）；关掉参数层才整个收口。
 *
 * ── 口径：走真实转发链路，按账号钉住 ──────────────────────────
 * 每勾一个账号就并发发一次 `POST /api/models/test`（一个账号一条请求），后端把这一家与这一个
 * 账号**钉住**再走 `UpstreamService::forward` —— token 刷新、出站脱敏、提示词模式、思考等级注入、
 * 请求日志与调试报文全都覆盖到，所以结论与生产一致。也因此**会消耗少量额度**，且每次测试都带
 * 「测试」来源标记记入请求日志（报表统计里排除，见后端 `is_test`）——这句压成一行常驻在参数弹窗
 * 底部（额度必须看得见），其余解释各归其位：目标行 / 流式 / 账号的口径进 ⓘ（tip-q + Tooltip），
 * 「留空则…」进 placeholder，页脚旁白与标题里重复的模型 ID 一律不摆。文案精简的对照与去向见
 * prototype/model-test-lite.html。
 *
 * ── 中止：没有 abort 桥，靠同一个关联 id ──────────────────────
 * 桌面壳的桥（`invoke('api_request')`）**没有 abort**：一次已经在跑的调用取消不掉。所以 id 由
 * **前端**生成（`test_id`）随请求一起发上去，中止时用同一个 id 去调既有的「终止请求」
 * （`terminateStatsRequest` → `POST /api/stats/requests/terminate?id=`）置位取消令牌，转发链当场
 * 断开上游。三处对齐：结果块、请求日志里那一行、中止用的把手，都是这一个 id。
 * 中止后回到参数层（`slots` 清空、seq 自增让在途回调作废），不把半截结果留在屏幕上。
 *
 * ── 两处刻意的「不显示」─────────────────────────────────────
 *   · **不摆原始报文**：发出去与收回来的原文在请求日志里能看能搜，堆进结果弹窗只会把「成没成、
 *     为什么不成」淹掉；
 *   · **不解释网关的换号口径**：测试按指定账号发、不顺延，那是测试的语义，不是转发策略的说明。
 */

import * as React from 'react'
import {
  Badge, Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle,
  Label, MultiSelect, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, Spinner,
  Switch, Textarea, Tooltip, TooltipContent, TooltipTrigger,
} from '@ui'
import {
  DEFAULT_PROVIDER_ID, byPriorityOrder, displayNameOf, isRateLimited, positionMap, providerFeatures,
} from './accounts-domain'
import type { AccountRecord } from './accounts-shared'
import * as customSource from './models-custom-source'
import type { ManageModel } from './models-custom-source'
import { levels as reasoningLevels } from './models-reasoning'
import {
  bindingsOf, errorMessage, getSnapshot, levelOf, modelRowOf, providerLabelOf, toast,
} from './models-panel-state'
import { t } from '../i18n'

/* ─── 类型 ─────────────────────────────────── */

/** 被测试的那一行：只带 `(provider, id)` 两个定位键（照 CapabilityContext 的形状） */
export type ModelTestTarget = { provider: string; id: string }

/** 一次测试的请求体（字段名与后端 `api::model_test` 逐字对齐，snake_case） */
type TestModelRequest = {
  provider: string
  model: string
  account_id?: string
  prompt?: string
  system_prompt?: string
  /** 本次指定的思考等级；空 = 不写进请求体（映射上绑的等级因此照常生效） */
  reasoning?: string
  stream?: boolean
  /** 前端生成的关联 id（请求日志那一行的 id，也是中止用的把手） */
  test_id?: string
}

/** 一次测试的响应（POST /api/models/test 永远 2xx，失败结论在 `status` / `error` 里） */
type TestModelResponse = {
  success?: boolean
  status?: number
  error?: string | null
  reply?: string
  reasoning?: string
  provider?: string
  model?: string
  account_id?: string | null
  account_name?: string | null
  upstream_model?: string | null
  upstream_reasoning?: string | null
  duration_ms?: number
  ttfb_ms?: number | null
  attempts?: number
  prompt_tokens?: number
  completion_tokens?: number
  total_tokens?: number
}

/**
 * 一个账号一条的测试槽位。
 *
 * `label` / `position` 是**发起时**的快照：结果回来时那一条账号可能已被改名或删掉，而用户要看的
 * 是「我刚才点的那一条」的结论 —— 改用结果里的 `account_name` 会让块标题中途变脸。
 */
type Slot = {
  accountId: string
  label: string
  position: number
  testId: string
  startedAt: number
  state: 'running' | 'done'
  result?: TestModelResponse
  /** 桥接层失败（本机接口不可达 / 超时）：与「上游给出的失败结论」分开表达 */
  transport?: string
}

/**
 * window 上由其它脚本 / 其它岛挂载的共享桥。
 *
 * 刻意用「局部窄类型 + 转型读取」而不是 declare global 往 Window 上加属性：workbuddyDesktop /
 * wbApp 是多个岛共用的桥，各岛各 declare 一份会因同名属性类型不一致直接报 TS2717 —— 并行迁移时
 * 必然互相撞车。本文件只声明自己用到的那几个成员（其余桥走 models-panel-state / accounts-domain
 * 里已有的那份）。
 */
type WindowBridge = {
  workbuddyDesktop?: {
    testModel(payload: TestModelRequest): Promise<TestModelResponse | null | undefined>
    /** 「终止请求」：按关联 id 置位取消令牌（测试的中止走它，见模块头） */
    terminateStatsRequest?(id: string): Promise<unknown>
  }
  wbApp?: {
    getState?: () => { accounts?: { accounts?: AccountRecord[] } } | null | undefined
  }
}

function bridge(): WindowBridge {
  return window as unknown as WindowBridge
}

/* ─── 常量 ─────────────────────────────────── */

/**
 * 用户提示词留空时后端用的默认值（后端 `api::model_test::DEFAULT_TEST_PROMPT`）。
 * 两处必须是同一句：取「你好」而不是一句有信息量的话 —— 测试要的是**最小请求**，越短越省额度、
 * 越少触发上游的内容策略，也越容易看出「通不通」这件事本身。
 */
const DEFAULT_PROMPT = '你好'

/** 提示词长度上限（与后端 `MAX_PROMPT_CHARS` 同值；超了后端会截断，这里先挡一道） */
const MAX_PROMPT_CHARS = 4000

/** 模型行的「来源」徽章（与模型表同一套口径；自定义家的 source 是空串，见 ManageModel） */
const SOURCE_LABEL: Record<string, string> = {
  remote: t('远程目录'),
  builtin: t('内置清单'),
  manual: t('手动登记'),
}

/**
 * 失败原因 → 下一步。**只按 HTTP 状态分类**，不猜上游的文案：429 与 401 指向完全不同的处置
 * （一个等一会儿自己会好，一个换账号也救不回来），5xx / 404 又是另外两件事。
 * 后端给的 `error` 原样展示在上一行，这里只补「接下来做什么」。
 */
const STATUS_HINT: Array<{ test: (status: number) => boolean; hint: string }> = [
  {
    test: status => status === 429,
    hint: t('该账号对这个模型正在限额冷却，等一会儿自己会好；也可以先测别的账号。'),
  },
  {
    test: status => status === 401 || status === 403,
    hint: t('上游拒绝了这条登录态：重新登录一次，或把这条账号删掉。'),
  },
  {
    test: status => status === 404,
    hint: t('上游不认识这个模型名：先点「获取模型」刷新清单，或核对默认绑定指向的上游模型。'),
  },
  {
    test: status => status === 504,
    hint: t('上游没在预算内给完回答。可以只勾一个账号再测一次，看是普遍慢还是某一个账号慢。'),
  },
  {
    test: status => status >= 500,
    hint: t('多半是上游自己的问题，过一会儿再试。同一个错误出现在所有账号上时，先查模型目录与映射。'),
  },
]

/* ─── 纯函数工具 ─────────────────────────────── */

/** 全部账号（主状态快照；账号页与本弹窗读的是同一份） */
function allAccounts(): AccountRecord[] {
  return bridge().wbApp?.getState?.()?.accounts?.accounts || []
}

/**
 * 该家可用于测试的账号：过滤 = 启用 + 有凭证（后端口径，与「模型来源」下拉同源），
 * 排序用账号页同一条 `byPriorityOrder`（全局一条队列，本弹窗只取这一家的子集）。
 */
function usableAccounts(provider: string): AccountRecord[] {
  return allAccounts()
    .filter(account => (account.provider || DEFAULT_PROVIDER_ID) === provider
      && account.available !== false && account.enabled !== false)
    .sort(byPriorityOrder)
}

/**
 * 账号的展示名：与「模型来源」下拉（models-fetch-modal.tsx 的 accountLabel）同一口径 ——
 * 以邮箱报名字的家（Qoder / AutoClaw 国际版）用邮箱，其余用账号页的展示名。
 */
function accountLabel(account: AccountRecord): string {
  const email = String(account.email || '').trim()
  if (email && providerFeatures(account.provider).emailAsName) return email
  return displayNameOf(account) || email || t('未命名账号')
}

/** 关联 id：优先 `crypto.randomUUID`（WebView2 与 localhost 都是安全上下文），退化到自拼的 v4 形态 */
function newTestId(): string {
  const uuid = globalThis.crypto?.randomUUID?.()
  if (uuid) return uuid
  const hex = (count: number): string =>
    Array.from({ length: count }, () => Math.floor(Math.random() * 16).toString(16)).join('')
  return `${hex(8)}-${hex(4)}-4${hex(3)}-a${hex(3)}-${hex(12)}`
}

/** 毫秒读数：1 秒以内给整数毫秒，再长给秒（两位小数；十秒以上一位就够） */
function formatMs(value: unknown): string {
  const time = Number(value)
  if (!Number.isFinite(time) || time <= 0) return '—'
  return time < 1000 ? `${Math.round(time)} ms` : `${(time / 1000).toFixed(time < 10000 ? 2 : 1)} s`
}

/**
 * 「测试」那颗按钮的门禁：该家**没有可用账号**（启用 + 凭证完整）→ 一行都发不出去，
 * 这是唯一还成立的置灰理由。返回空串 = 可以测；否则返回要挂在按钮 title 上的原因。
 *
 * ── **未启用的行不再置灰**（曾经置灰，别改回去）────────────────
 * 这颗按钮的用法本来就是「先测通、再决定要不要启用」—— 被测的行往往就是关着的，
 * 置灰等于把按钮的唯一用途挡在门外。后端为此给测试开了直达跳
 * （`ForwardRequest::ignore_model_gate`）：候选直接取被钉住的那家，跳过
 * 「模型已在网关中关闭」的生产门禁；关闭的默认绑定解析不出改写目标，名字原样
 * 直发、映射上的思考等级不注入 —— 测的就是这个模型在这家上游的**真实形态**。
 */
export function testBlockReason(provider: string, model: ManageModel): string {
  if (!usableAccounts(provider).length) {
    return t('该提供商没有可用账号（要在账号页启用一个、且凭证完整），一行都发不出去')
  }
  return ''
}

/* ─── 小件 ─────────────────────────────────── */

/** 「i + 一句话」提示（圈的用法与页面上其它提示一致）。box 是结果区的处置建议块；
 *  plain 是参数弹窗底部的一行常驻口径 —— 精简文案后弹窗里唯一「必须看见」的说明（会消耗额度） */
function NoteBlock({ children, plain = false }: { children: React.ReactNode; plain?: boolean }) {
  return (
    <p className={plain
      ? 'flex items-center gap-2 text-xs leading-[1.6] text-subtle'
      : 'flex gap-2 rounded-md border border-border bg-surface-2 px-3 py-2.5 text-xs leading-[1.65] text-subtle'}>
      <span aria-hidden='true'
        className={plain
          ? 'size-4 flex-none rounded-full border border-border-strong text-center text-[10px] leading-[14px]'
          : 'mt-px size-4 flex-none rounded-full border border-border-strong text-center text-[10px] leading-[14px]'}>i</span>
      <span>{children}</span>
    </p>
  )
}

/**
 * 一个可折叠块（思考过程）。头是组件库的 ghost 按钮而不是自绘 button ——
 * tokens.css 只给 button 做了 `font / color: inherit`，没清浏览器默认的描边与底色。
 * 左对齐靠「末件 ml-auto」而不是 `justify-start`：那是同一个属性的两张工具类，
 * 谁赢由 Tailwind 的产出顺序决定（center 在后），不能指望覆盖。
 */
function Fold({ open, label, text, onToggle }: {
  open: boolean; label: string; text: string; onToggle: () => void
}) {
  return (
    <div className='overflow-hidden rounded-sm border border-border'>
      <Button variant='ghost' size='xs' className='w-full' aria-expanded={open} onClick={onToggle}>
        <span className='truncate'>{label}</span>
        <span aria-hidden='true' className='ml-auto text-[10px]'>{open ? '▴' : '▾'}</span>
      </Button>
      {open ? (
        <div className='border-t border-hairline bg-surface px-2.5 py-2'>
          <pre className='whitespace-pre-wrap font-mono text-[11.5px] leading-[1.65] text-subtle'>{text}</pre>
        </div>
      ) : null}
    </div>
  )
}

/** 「已等待 1.4s」的读秒：只在跑着的时候开定时器（停表后不再空转） */
function useClock(active: boolean): number {
  const [now, setNow] = React.useState(() => Date.now())
  React.useEffect(() => {
    if (!active) return
    setNow(Date.now())
    const timer = window.setInterval(() => setNow(Date.now()), 400)
    return () => window.clearInterval(timer)
  }, [active])
  return now
}

/** 成功判据：没有错误结论、且状态码是 2xx（后端把「上游失败」也放在 2xx 的响应体里） */
function isOk(slot: Slot): boolean {
  if (slot.transport || !slot.result) return false
  if (slot.result.error) return false
  const status = Number(slot.result.status) || 0
  return status >= 200 && status < 300
}

/* ─── 弹窗本体 ───────────────────────────────── */

export function ModelTestDialog({ target, onClose }: { target: ModelTestTarget; onClose: () => void }) {
  const provider = target.provider
  const model = modelRowOf(provider, target.id)

  const usable = usableAccounts(provider)
  const positions = positionMap(allAccounts())

  const [systemPrompt, setSystemPrompt] = React.useState('')
  const [prompt, setPrompt] = React.useState(DEFAULT_PROMPT)
  const [reasoning, setReasoning] = React.useState('')
  const [stream, setStream] = React.useState(true)
  /**
   * 默认只勾**队列里最靠前的那个可测账号**（真实链路的第一个候选）。
   * 对当前模型正在冷却的账号靠后站：拿它测只会第一跳就拿回 429，而用户要的是「这个模型通不通」。
   */
  const [picked, setPicked] = React.useState<string[]>(() => {
    const first = usable.find(account => !isRateLimited(account, target.id)) || usable[0]
    return first ? [first.id] : []
  })
  const [slots, setSlots] = React.useState<Slot[]>([])
  const [folds, setFolds] = React.useState<ReadonlySet<string>>(() => new Set())

  /** 本次运行的序号：中止 / 重测会让在途回调作废（迟到的结果不许写进新一轮） */
  const seq = React.useRef(0)
  const slotsRef = React.useRef<Slot[]>([])
  slotsRef.current = slots

  const running = slots.some(slot => slot.state === 'running')
  const now = useClock(running)

  /** 中止在途请求：seq 自增（作废回调）+ 逐个置位取消令牌，然后清空槽位回到参数层 */
  const abort = React.useCallback((quiet = false) => {
    const inflight = slotsRef.current.filter(slot => slot.state === 'running')
    seq.current += 1
    for (const slot of inflight) {
      void bridge().workbuddyDesktop?.terminateStatsRequest?.(slot.testId)?.catch(() => {})
    }
    setSlots([])
    if (inflight.length && !quiet) toast(t('已中止在途测试（{n} 个账号）', { n: inflight.length }))
  }, [])

  // 弹窗被卸掉（用户关掉参数层 / 模型行在目录刷新后消失）时补一刀：已经放弃的在途请求不该继续
  // 占上游额度。清理里不再动状态（组件已经没了），所以走 quiet。
  React.useEffect(() => () => { abort(true) }, [abort])

  /** 发一次测试：结果按 testId 对号入座；整轮的序号变了就直接丢弃（中止 / 重测） */
  async function send(slot: Slot, runId: number): Promise<void> {
    const body: TestModelRequest = {
      provider,
      model: target.id,
      account_id: slot.accountId,
      prompt: prompt.trim(),
      system_prompt: systemPrompt.trim(),
      reasoning,
      stream,
      test_id: slot.testId,
    }
    const settle = (patch: Partial<Slot>): void => {
      if (seq.current !== runId) return
      setSlots(previous => previous.map(item => (item.testId === slot.testId ? { ...item, ...patch } : item)))
    }
    try {
      const result = await bridge().workbuddyDesktop?.testModel(body)
      if (!result) {
        settle({ state: 'done', transport: t('桥接调用返回空（本机网关没有响应这次测试）') })
        return
      }
      settle({ state: 'done', result })
    } catch (error) {
      settle({ state: 'done', transport: errorMessage(error) })
    }
  }

  /** 发起一轮：勾了几个账号就并发发几次（一个账号一条请求，互不影响） */
  function run(): void {
    const chosen = usable.filter(account => picked.includes(account.id))
    if (!chosen.length) {
      toast(t('至少选一个测试账号'), 'err')
      return
    }
    const runId = seq.current + 1
    seq.current = runId
    const startedAt = Date.now()
    const next: Slot[] = chosen.map(account => ({
      accountId: account.id,
      label: accountLabel(account),
      position: positions.get(account.id)?.position || 0,
      testId: newTestId(),
      startedAt,
      state: 'running',
    }))
    setFolds(new Set())
    setSlots(next)
    for (const slot of next) void send(slot, runId)
  }

  /** 结果层关掉：跑着的时候 = 中止（参数留着，回下层改完就能重测） */
  function closeResults(): void {
    if (running) abort()
    else setSlots([])
  }

  /** 顶栏那个模型 ID 小字（两层标题共用） */
  const modelTag = <span className='ml-2 font-mono text-[12px] font-normal text-subtle'>{target.id}</span>

  /** 目标行不在清单里了（模型被移除，或目录刷新后上游不再提供它）——与「模型能力」同一处置 */
  if (!model) {
    return (
      <Dialog open onOpenChange={next => { if (!next) onClose() }}>
        <DialogContent>
          <DialogHeader><DialogTitle>{t('测试模型')}</DialogTitle></DialogHeader>
          <DialogBody>
            <p className='text-sm leading-[1.7] text-subtle'>
              {t('这一行已不在当前清单里（模型被移除、或目录刷新后上游不再提供它）。关闭后刷新列表再试。')}
            </p>
          </DialogBody>
          <DialogFooter>
            <div className='mr-auto' />
            <Button variant='outline' onClick={onClose}>{t('关闭')}</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    )
  }

  // 本名直发时生效的思考等级只认「名字不变的那条按家映射」（见 model_rules::reasoning 第 4 条），
  // 而那正是这一行的默认绑定 —— 与表格里默认 chip 上显示的是同一个值。
  // 该行未启用时映射整体不生效（生产路由也放不过去），等级自然不参与本次。
  const defaultClosed = bindingsOf(model).find(binding => binding.isDefault)?.enabled === false
  const boundLevel = levelOf(model.id, model.id, provider)
  const aliases = bindingsOf(model)
    .filter(binding => !binding.isDefault)
    .map(binding => binding.alias)
  // 「关闭思考」两档（off / none）不给：它们在**映射**上的含义是「不注入」，而写进**请求体**
  // 就是一个上游不认识的档位 —— CatPaw 的 resolve_effort 对表外值当场 400。要「不注入」，
  // 选「跟随映射」这一项（映射上没绑等级时它本来就不注入）
  const levelOptions = reasoningLevels(getSnapshot().data)
    .filter(level => level !== 'off' && level !== 'none')
  const followLabel = defaultClosed
    ? t('跟随映射（该行未启用，本次不注入）')
    : boundLevel ? t('跟随映射（当前 {level}）', { level: boundLevel }) : t('跟随映射（未绑定等级）')
  /** 目标行 ⓘ 的口径：多数行没有别名 / 未绑等级，一句「以本名直发」就够 —— 说明悬停才见 */
  const targetTip = [
    t('以模型本名直发'),
    aliases.length
      ? t('另有 {n} 条别名映射（{aliases}），别名不参与本次', { n: aliases.length, aliases: aliases.join(t('、')) })
      : '',
    defaultClosed
      ? t('该行当前未启用：测试照常按本名直发（「先测通、再决定要不要启用」正是这颗按钮的用法），但本行的思考等级绑定不参与本次，生产路由也要等绑定打开后才会放行')
      : boundLevel
        ? t('映射上绑定的思考等级是 {level}，「跟随映射」按它注入', { level: boundLevel })
        : t('映射上未绑定思考等级，「跟随映射」等于这次不注入'),
  ].filter(Boolean).join(t('；')) + t('。')
  const accountOptions = usable.map(account => ({
    value: account.id,
    // 「限额中」按**这个模型**判（限额是按模型记的：一个账号可能对 A 模型限额、对 B 模型正常）
    label: `#${positions.get(account.id)?.position || 0} ${accountLabel(account)}`
      + (isRateLimited(account, target.id) ? t(' · 限额中') : ''),
  }))

  /** 一个账号的结果块（成功：回复 + 思考过程折叠；失败：错误结论 + 下一步） */
  function slotBlock(slot: Slot): React.ReactNode {
    const head = (badge: React.ReactNode, metrics: string): React.ReactNode => (
      <div key='head' className='flex items-center gap-2.5 border-b border-hairline px-3 py-2'>
        {badge}
        <b className='min-w-0 truncate text-[12.5px] text-foreground' title={slot.label}>{slot.label}</b>
        {slot.position ? <span className='flex-none text-xs text-subtle'>#{slot.position}</span> : null}
        <span className='ml-auto flex-none font-mono text-[11.5px] tabular-nums text-subtle'>{metrics}</span>
      </div>
    )

    if (slot.state === 'running') {
      const waited = (now - slot.startedAt) / 1000
      return (
        <div key={slot.testId}
          className='flex items-center gap-2.5 rounded-md border border-border bg-surface-2 px-3 py-2.5 text-xs text-subtle'>
          <Spinner className='flex-none' />
          <span className='min-w-0 truncate'>
            {t('账号 ')}<b className='text-foreground'>{slot.label}</b>{t(' 已发出，等待上游首帧…')}
          </span>
          <span className='ml-auto flex-none font-mono tabular-nums'>
            {t('已等待 {seconds}s', { seconds: waited.toFixed(1) })}
          </span>
        </div>
      )
    }

    const result = slot.result
    if (!isOk(slot)) {
      const status = Number(result?.status) || 0
      const hint = STATUS_HINT.find(item => item.test(status))?.hint
        ?? (slot.transport ? t('确认桌面端还在运行，再重试一次。') : '')
      return (
        <div key={slot.testId} className='rounded-md border border-border bg-surface-2'>
          {head(
            <Badge shape='tag' variant='destructive' className='flex-none'>{status || t('失败')}</Badge>,
            t('用时 {time}', { time: formatMs(result?.duration_ms) }),
          )}
          <div className='flex flex-col gap-1.5 px-3 py-2.5'>
            <div className='text-[12.5px] leading-[1.7]'>
              {slot.transport
                ? <><b className='text-destructive'>{t('本机网关没有返回结论')}</b>{t('：')}{slot.transport}</>
                : <><b className='text-destructive'>{t('测试未通过')}</b>{t('：')}{result?.error || t('上游没有给出可读的错误说明')}</>}
              {Number(result?.attempts) > 1 ? t('（中间共尝试 {n} 次）', { n: Number(result?.attempts) }) : ''}
            </div>
            {hint ? <p className='text-xs leading-[1.65] text-subtle'>{hint}</p> : null}
          </div>
        </div>
      )
    }

    const reply = String(result?.reply ?? '')
    const reasoningText = String(result?.reasoning ?? '')
    const foldKey = `think-${slot.testId}`
    // 「上游 xxx」只在实际模型与被测名不同（映射改名）时才有信息量，同名不重复念一遍
    const metaLine = [
      result?.upstream_model && result.upstream_model !== target.id
        ? t('上游 {model}', { model: result.upstream_model }) : '',
      result?.upstream_reasoning ? t('思考等级 {level}', { level: result.upstream_reasoning }) : '',
      Number(result?.attempts) > 1 ? t('尝试 {n} 次', { n: Number(result?.attempts) }) : '',
    ].filter(Boolean).join(' · ')
    return (
      <div key={slot.testId} className='rounded-md border border-border bg-surface-2'>
        {head(
          <Badge shape='tag' variant='success' className='flex-none'>{Number(result?.status) || 200}</Badge>,
          t('用时 {time} · 首字 {ttfb}', { time: formatMs(result?.duration_ms), ttfb: formatMs(result?.ttfb_ms) }),
        )}
        <div className='flex flex-col gap-2 px-3 py-2.5'>
          <div className='flex items-center gap-2 text-[11.5px] text-subtle'>
            <span className='flex-none'>{t('回复')}</span>
            {metaLine ? <span className='min-w-0 truncate' title={metaLine}>{metaLine}</span> : null}
            {/* 复制走 clipboard.js 的全局委托（data-copy），与模型名那枚复制同一套 */}
            <Button variant='ghost' size='2xs' className='ml-auto flex-none' data-copy={reply}>{t('复制')}</Button>
          </div>
          <div className='max-h-[220px] overflow-y-auto whitespace-pre-wrap rounded-sm border border-border bg-surface px-3 py-2 text-[12.5px] leading-[1.7]'>
            {reply || <span className='text-subtle'>{t('（上游这一次返回了空正文）')}</span>}
          </div>
          {reasoningText ? (
            <Fold open={folds.has(foldKey)} label={t('思考过程（{n} 字）', { n: reasoningText.length })} text={reasoningText}
              onToggle={() => setFolds(previous => {
                const next = new Set(previous)
                if (next.has(foldKey)) next.delete(foldKey)
                else next.add(foldKey)
                return next
              })} />
          ) : null}
        </div>
      </div>
    )
  }

  /**
   * 结果区：测试中把已回来的账号**就地换成结果块**（只有还在等的留骨架）—— 结果本来就是一个
   * 账号一条回来的，全憋到最后一起显示等于白等；出结果后每行一个块，外加一条成功 / 失败读数。
   */
  function results(): React.ReactNode {
    if (running) {
      const done = slots.length - slots.filter(slot => slot.state === 'running').length
      return (
        <>
          {/* 单账号时这行只是复述下面那块本身，省掉 */}
          {slots.length > 1 ? (
            <p className='text-xs text-subtle'>
              {t('正在测 ')}<b className='text-foreground'>{slots.length}</b>{t(' 个账号')}
              {done ? <>{t('，已完成 ')}<b className='text-foreground'>{done}</b></> : null}{t('。')}
            </p>
          ) : null}
          {slots.map(slot => slotBlock(slot))}
        </>
      )
    }
    const okCount = slots.filter(isOk).length
    return (
      <>
        {/* 单账号同上：结果块的徽章与指标已经说明一切；非 0 的读数才上色，注意力给问题 */}
        {slots.length > 1 ? (
          <p className='text-xs text-subtle'>
            <b className={okCount ? 'text-success' : 'text-foreground'}>{okCount}</b>{t(' 成功 ·')}
            <b className={slots.length - okCount ? 'text-destructive' : 'text-foreground'}> {slots.length - okCount}</b>{t(' 失败')}
          </p>
        ) : null}
        {slots.map(slot => slotBlock(slot))}
        {okCount === 0 ? (
          <NoteBlock>
            {t('两边原因不一样时各自处理。如果')}<b className='text-foreground'>{t('同一条错误出现在所有账号上')}</b>{t('，通常就不是账号问题了：先确认这个模型还在上游目录里（「获取模型」刷一次），再看默认绑定指向的上游模型对不对。')}
          </NoteBlock>
        ) : null}
      </>
    )
  }

  return (
    // 参数层：受控 open（恒为 true）。本弹窗是「打开时建、关闭即卸」，关窗一律由 onClose 收口
    // （Esc / 点遮罩 / ✕ 都由 Base UI 汇到 onOpenChange）。
    <Dialog open onOpenChange={next => { if (!next) onClose() }}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{t('测试模型')}{modelTag}</DialogTitle>
        </DialogHeader>
        <DialogBody>
          {/* 目标行：测的是哪一条（与「模型能力」弹窗的预览行同一用意）；发送口径收进行尾 ⓘ，不再常驻一段 */}
          <div className='flex items-center gap-2 rounded-md border border-border bg-surface-inset px-3 py-2.5'>
            <span className='min-w-0 flex-1 truncate font-mono text-[12.5px] font-semibold text-primary-fg'
              title={model.id}>{model.id}</span>
            <Badge shape='tag' variant='brand'>{providerLabelOf(provider)}</Badge>
            <Badge shape='tag' variant='outline'>
              {customSource.isCustom(provider) ? t('自定义家') : (SOURCE_LABEL[model.source] || t('来源未知'))}
            </Badge>
            <Tooltip>
              <TooltipTrigger render={<span className='tip-q' tabIndex={0} aria-label={t('本次测试的发送口径')} />}>?</TooltipTrigger>
              <TooltipContent>{targetTip}</TooltipContent>
            </Tooltip>
          </div>

          <div className='flex flex-col gap-1.5'>
            <Label htmlFor='model-test-system'>{t('系统提示词')}</Label>
            <Textarea id='model-test-system' rows={2} maxLength={MAX_PROMPT_CHARS}
              placeholder={t('留空则不携带；设置页的提示词模式照常生效')}
              value={systemPrompt}
              onChange={event => setSystemPrompt(event.currentTarget.value)} />
          </div>

          <div className='flex flex-col gap-1.5'>
            <Label htmlFor='model-test-prompt'>{t('用户提示词')}</Label>
            <Textarea id='model-test-prompt' rows={2} maxLength={MAX_PROMPT_CHARS}
              placeholder={t('留空用默认问候「你好」')}
              value={prompt}
              onChange={event => setPrompt(event.currentTarget.value)} />
          </div>

          <div className='grid grid-cols-2 gap-4'>
            <div className='flex flex-col gap-1.5'>
              <Label htmlFor='model-test-reasoning'>{t('思考等级')}</Label>
              <Select value={reasoning} onValueChange={next => setReasoning(String(next))}>
                <SelectTrigger id='model-test-reasoning' className='w-full'>
                  <SelectValue>{reasoning ? t('{level}（本次指定）', { level: reasoning }) : followLabel}</SelectValue>
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value=''>{followLabel}</SelectItem>
                  {levelOptions.map(level => (
                    <SelectItem key={level} value={level}>{t('{level}（本次指定）', { level })}</SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div className='flex flex-col gap-1.5'>
              <div className='flex items-center gap-1.5'>
                <Label htmlFor='model-test-stream'>{t('流式请求')}</Label>
                <Tooltip>
                  <TooltipTrigger render={<span className='tip-q' tabIndex={0} aria-label={t('流式请求的说明')} />}>?</TooltipTrigger>
                  <TooltipContent>{t('默认开，与真实请求一致：可同时验证 SSE 链路与首字延迟。')}</TooltipContent>
                </Tooltip>
              </div>
              <Switch id='model-test-stream' checked={stream}
                onCheckedChange={value => setStream(Boolean(value))} />
            </div>
          </div>

          <div className='flex flex-col gap-1.5'>
            <div className='flex items-center gap-2'>
              <Label htmlFor='model-test-accounts'>{t('测试账号')}</Label>
              <Tooltip>
                <TooltipTrigger render={<span className='tip-q' tabIndex={0} aria-label={t('测试账号的说明')} />}>?</TooltipTrigger>
                <TooltipContent>{t('勾几个就并行测几次；默认选队列里最靠前的一个。')}</TooltipContent>
              </Tooltip>
              <Button variant='ghost' size='xs' className='ml-auto'
                disabled={!usable.length || picked.length === usable.length}
                onClick={() => setPicked(usable.map(account => account.id))}>
                {t('全选（{n}）', { n: usable.length })}
              </Button>
            </div>
            <MultiSelect id='model-test-accounts' value={picked} onValueChange={setPicked}
              options={accountOptions} placeholder={t('选一个账号（至少一个）')}
              searchPlaceholder={t('搜索账号…')}
              emptyHint={t('这家没有可用账号（要在账号页启用一个、且凭证完整）')} />
          </div>

          <NoteBlock plain>{t('真实链路 · 消耗少量额度 · 日志带「测试」标记，不计报表')}</NoteBlock>
        </DialogBody>
        <DialogFooter className='justify-end'>
          <Button variant='outline' onClick={onClose}>{t('关闭')}</Button>
          <Button variant='default' disabled={!picked.length || !usable.length} onClick={run}>{t('开始测试')}</Button>
        </DialogFooter>

        {/* ── 结果层：叠在参数层之上的一层（位置与 overlayForceRender 的理由见模块头）── */}
        <Dialog open={slots.length > 0} onOpenChange={next => { if (!next) closeResults() }}>
          <DialogContent overlayForceRender className='w-[min(680px,calc(100vw-48px))]'>
            <DialogHeader>
              <DialogTitle>{running ? t('测试中…') : t('测试结果')}{modelTag}</DialogTitle>
            </DialogHeader>
            <DialogBody>{results()}</DialogBody>
            <DialogFooter className='justify-end'>
              {running ? (
                <>
                  <span className='mr-auto text-xs text-subtle'>{t('关闭即中止')}</span>
                  <Button variant='outline' onClick={() => abort()}>{t('中止测试')}</Button>
                  <Button variant='default' disabled>{t('测试中…')}</Button>
                </>
              ) : (
                <>
                  <Button variant='outline' onClick={closeResults}>{t('关闭')}</Button>
                  <Button variant='default' onClick={run}>{t('再测一次')}</Button>
                </>
              )}
            </DialogFooter>
          </DialogContent>
        </Dialog>
      </DialogContent>
    </Dialog>
  )
}
