import * as React from 'react'
import { Badge, Button, Checkbox, Input, Spinner, Switch } from '@ui'
import { formatTime, shared, toast } from './accounts-shared'
import { t } from '../i18n'
import {
  getCheckinStore, setKeepaliveDraft, submitKeepaliveModels, subscribeCheckinStore,
  type CheckinStore,
} from './checkin-state'
import {
  deleteSchedule, getSchedulesStore, loadSchedules, runScheduleNow, saveSchedule,
  setEditingId, subscribeSchedulesStore, toggleMaster, toggleScheduleEnabled,
  type SchedulesStore,
} from './checkin-schedules-state'
import type { ScheduleEntry } from './checkin-state'

/**
 * 签到中心的「定时签到」标签页。
 *
 * ── 结构 ────────────────────────────────────────────────────
 *   · 总开关面板 —— `autoCheckin.enabled`（写走老的 /api/auto-checkin，见
 *     checkin-schedules-state 的模块头）。关了所有条目都不跑；开着时每条
 *     条目按自己的时刻与范围各自执行、各自当天去重。
 *   · 条目列表 —— 每条：开关 / 时刻 / 范围 / 下次执行 / 上次执行摘要 +
 *     立即执行 / 编辑 / 删除。行内展开编辑表单（一次只开一张，状态在
 *     schedules-state 的 editingId）；新建同表单（editingId = 'new'）。
 *   · 保活模型链面板 —— 自原「自动签到」卡迁入（它是全局一份的配置，
 *     与条目无关，所以独立成卡）：WorkBuddy 国际版日活保活的免费模型，
 *     按顺序回退；清空提交 = 恢复缺省链。
 *
 * 数据：GET /api/checkin-schedules 一次拉全（loadSchedules，挂载时调）；
 * 写动作的响应自带整份新状态，无轮询。
 */

/**
 * 问号提示全文（沿用原「自动签到」卡的说明，调度语义没变 —— 变的只是
 * 「一个时刻」变成「多条目各自时刻」）。
 * 整段是一个键：扫描器只认字符串字面量（拼接串 / 模板串提不到键），所以不拆行。
 */
const AUTO_CHECKIN_DESC = t(
    '到点后自动签到勾选提供商的可用账号（WorkBuddy 国内版走每日签到接口，国际版走每日活跃任务：探测活动、领日活奖励、再用免费模型保活并在网页通道建会话点亮日活；小浣熊走桌面端每日积分链路；AutoClaw 走官方客户端的每日签到任务；Qoder 中国版走活动领取，没被下发活动的账号会得到中性提示；Qoder 国际版要先带设备风控身份（本机 Qoder 客户端 / qodercli 的 UMID 组件生成）才能看到「每日 100 Credits」活动，缺组件时会得到明确的说明而不是报错；Trae 领 SOLO 的每日签到积分（按自然日 0 点重置），上游没对该账号开活动、或凭据里没有设备号时得到的是「未领取 + 具体原因」而不是报错；Loomy 每天替账号打一次首次登录积分；KukuAI 领「免费领积分」的每日任务；CodeArts 领当日的运营福利（领取 → 确认 → 回读核实，自动执行受每天 6 次 / 间隔 10 分钟的写请求限流）；OfficeAce 果办一次领当天全部奖励（每日签到 + 新人礼））。错过时点开机后会自动补签，不会因为当时没开机而漏掉。各家签到接口都是幂等的，重复执行不会重复领取。',
)

/* ─── store 订阅 ───────────────────────────── */

function useSchedulesStore(): SchedulesStore {
  const [store, setStore] = React.useState(getSchedulesStore())
  React.useEffect(() => subscribeSchedulesStore(() => setStore(getSchedulesStore())), [])
  return store
}

/** 保活模型链的草稿 / 快照在 checkin-state 里，这里只订阅读 */
function useCheckinSnapshot(): CheckinStore {
  const [store, setStore] = React.useState(getCheckinStore())
  React.useEffect(() => subscribeCheckinStore(() => setStore(getCheckinStore())), [])
  return store
}

/* ─── 展示工具 ─────────────────────────────── */

/** 勾选的家名（providers 顺序即注册顺序：注册表顺序，不是勾选先后） */
function pickedLabels(entry: ScheduleEntry, options: Array<{ id: string; label: string }>): string[] {
  const picked = Array.isArray(entry.providers) ? entry.providers : []
  return options.filter(option => picked.includes(option.id)).map(option => option.label)
}

/**
 * 范围摘要。**全选时只说「全部 n 家」**：默认就是全选，把 12 个名字铺一行
 * 之后，这一行里真正该被看见的「每天几点」和「上次执行」全被名字淹掉了
 * （名称列被挤成两行）。部分勾选才逐家列 —— 那时名单本身就是信息
 * （悬停看全貌，见 `providersFullList`）。
 */
function providersSummary(entry: ScheduleEntry, options: Array<{ id: string; label: string }>): string {
  const labels = pickedLabels(entry, options)
  if (!labels.length) return t('未勾选提供商')
  if (options.length > 0 && labels.length >= options.length) return t('全部 {n} 家', { n: options.length })
  return labels.join(t('、'))
}

/** 完整名单（行里会截断，悬停提示用这一份） */
function providersFullList(entry: ScheduleEntry, options: Array<{ id: string; label: string }>): string {
  const labels = pickedLabels(entry, options)
  return labels.length ? labels.join(t('、')) : t('未勾选提供商')
}

/** 上次执行的一行摘要（时间 · 触发原因 —— 成功 n · 失败 n） */
function lastResultLine(entry: ScheduleEntry): string {
  const last = entry.lastResult
  if (!last) return t('还没执行过')
  const numberOf = (value: unknown): number => Number(value) || 0
  const parts = [t('成功 {n}', { n: numberOf(last.succeeded) })]
  const active = numberOf(last.active)
  if (active > 0) parts.push(t('保活 {n}', { n: active }))
  const failed = numberOf(last.failedCount)
  if (failed > 0) parts.push(t('失败 {n}', { n: failed }))
  return `${last.at ? formatTime(last.at) : '—'}${last.reason ? ` · ${last.reason}` : ''} —— ${parts.join(t(' · '))}`
}

/* ─── 编辑表单 ─────────────────────────────── */

/**
 * 条目的行内编辑 / 新建表单。草稿是**组件内**状态（展开时按 entry 现起，
 * 收起即弃）—— 与时刻草稿跨卡共享的旧场景不同，这里一次只开一张表单。
 */
function ScheduleEditor({ entry, store }: { entry: ScheduleEntry | null; store: SchedulesStore }) {
  const options = Array.isArray(store.data?.providerOptions) ? store.data!.providerOptions! : []
  const isNew = entry === null
  const [label, setLabel] = React.useState(entry?.label ?? '')
  const [time, setTime] = React.useState(entry?.time ?? '00:01')
  const [picked, setPicked] = React.useState<ReadonlySet<string>>(
    new Set(isNew ? options.map(option => option.id) : entry?.providers ?? []),
  )
  // 新建缺省全选；没有 options（后端没给）时退到这条已有的 providers
  React.useEffect(() => {
    if (isNew && options.length > 0) setPicked(new Set(options.map(option => option.id)))
    // 只在挂载 / 切换编辑对象时初始化一次：options 后到不该把用户改了一半的勾选冲掉
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])
  const canSave = time.trim() !== '' && picked.size > 0
  const pickAll = (): void => setPicked(new Set(options.map(option => option.id)))
  const clearAll = (): void => setPicked(new Set<string>())
  const submit = async (): Promise<void> => {
    await saveSchedule({
      id: entry?.id,
      label: label.trim(),
      enabled: entry?.enabled ?? true,
      time: time.trim(),
      providers: options.map(option => option.id).filter(id => picked.has(id)),
    })
  }
  return (
    <div className='ck-sched-editor'>
      <div className='ck-set-line'>
        <span className='ck-set-k'>{t('名称')}</span>
        <span className='ck-set-v'>
          <Input
            type='text'
            className='max-w-[220px]'
            placeholder={t('可选 · 缺省显示时刻')}
            value={label}
            disabled={store.saving}
            onChange={event => setLabel(event.currentTarget.value)}
          />
        </span>
      </div>
      <div className='ck-set-line'>
        <span className='ck-set-k'>{t('每天时刻')}</span>
        <span className='ck-set-v flex items-center gap-2'>
          <Input
            type='time'
            className='w-[96px] max-w-[96px] font-mono tabular-nums'
            value={time}
            disabled={store.saving}
            onChange={event => setTime(event.currentTarget.value)}
          />
          <span className='text-subtle text-xs'>{t('北京时间 · 错过时点开机后会自动补签')}</span>
        </span>
      </div>
      <div className='ck-set-line ck-set-line-top'>
        <span className='ck-set-k'>{t('签到范围')}</span>
        <span className='ck-set-v'>
          <div className='ck-mini-row'>
            <span className='ck-mini-note'>
              {t('到点时逐个签到，已勾 {n} / {total} 家', { n: picked.size, total: options.length })}
            </span>
            <button type='button' className='ck-mini-btn' disabled={store.saving} onClick={pickAll}>
              {t('全选')}
            </button>
            <button
              type='button'
              className='ck-mini-btn'
              disabled={store.saving || picked.size === 0}
              onClick={clearAll}
            >
              {t('清空')}
            </button>
          </div>
          <div className='ck-range'>
            {options.map(option => (
              <label className='ck-range-item' key={option.id}>
                <Checkbox
                  checked={picked.has(option.id)}
                  disabled={store.saving}
                  onCheckedChange={next => {
                    const nextPicked = new Set(picked)
                    if (next === true) nextPicked.add(option.id)
                    else nextPicked.delete(option.id)
                    setPicked(nextPicked)
                  }}
                />
                <span className='ck-range-name' title={option.label}>{option.label}</span>
              </label>
            ))}
          </div>
          {picked.size === 0 ? (
            <div className='ck-mini-warn'>{t('至少勾一家才能保存（不勾等于这条条目不签到）')}</div>
          ) : null}
        </span>
      </div>
      <div className='ck-sched-actions'>
        <Button size='sm' variant='outline' disabled={store.saving} onClick={() => setEditingId(null)}>
          {t('取消')}
        </Button>
        <Button size='sm' variant='default' disabled={!canSave || store.saving} onClick={() => void submit()}>
          {store.saving ? t('保存中…') : t('保存')}
        </Button>
      </div>
    </div>
  )
}

/* ─── 条目行 ───────────────────────────────── */

/**
 * 新建条目的那一行。它必须是**真渲染出来的一段界面** —— 之前「添加条目」
 * 只把 `editingId` 置成 `'new'`，而没有一处按这个值渲染（条目行只认
 * `entry.id`），于是点了完全没反应（状态变了、界面没动）。
 *
 * 位置在列表**首位**：点按钮的地方（卡头右上）离它最近，也不会被已有条目
 * 推到看不见的地方去。
 */
function NewEntryRow({ store }: { store: SchedulesStore }) {
  return (
    <div className='ck-sched-row open ck-sched-row-new'>
      <div className='ck-sched-main'>
        <span className='ck-new-dot' aria-hidden>＋</span>
        <div className='ck-prov-info'>
          <div className='ck-prov-name'>{t('新建定时条目')}</div>
          <div className='ck-prov-desc'>
            {t('填好时刻与签到范围后点「保存」；名称可以留空（界面显示时刻）')}
          </div>
        </div>
      </div>
      <ScheduleEditor entry={null} store={store} />
    </div>
  )
}

function ScheduleEntryRow({ entry, store }: { entry: ScheduleEntry; store: SchedulesStore }) {
  const options = Array.isArray(store.data?.providerOptions) ? store.data!.providerOptions! : []
  const expanded = store.editingId === entry.id
  const busy = store.runningId === entry.id
  const masterOn = store.data?.enabled === true
  const nextRunAt = Number(entry.nextRunAt)
  const remove = async (): Promise<void> => {
    const ask = shared().wbConfirm?.ask
    if (!ask) return
    // ConfirmOptions 只收 html（调用方负责转义）：条目名来自用户输入，先转义再拼
    const name = entry.displayLabel || entry.time || entry.id
    const escaped = name.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
    if (!(await ask({
      title: t('删除定时条目'),
      html: t('确定删除「<strong>{name}</strong>」？删除后到点不再执行。', { name: escaped }),
      okText: t('删除'),
      okClass: 'danger',
    }))) return
    await deleteSchedule(entry.id)
  }
  return (
    <div className={expanded ? 'ck-sched-row open' : 'ck-sched-row'}>
      <div className='ck-sched-main'>
        <Switch
          checked={entry.enabled === true}
          disabled={store.saving}
          onCheckedChange={next => void toggleScheduleEnabled(entry, next === true)}
        />
        <div className='ck-prov-info'>
          <div className='ck-prov-name'>
            {entry.displayLabel || entry.time || entry.id}
            {entry.enabled === true
              ? null
              : <Badge variant='outline' shape='tag'>{t('已停用')}</Badge>}
          </div>
          <div className='ck-prov-desc ck-sched-scope' title={providersFullList(entry, options)}>
            {t('每天 {time} · {scope}', { time: entry.time ?? '—', scope: providersSummary(entry, options) })}
          </div>
          <div className='ck-prov-desc'>
            {t('上次执行：{line}', { line: lastResultLine(entry) })}
          </div>
        </div>
        <div className='ck-prov-right'>
          <span className='ck-sched-next'>
            {masterOn && entry.enabled === true && Number.isFinite(nextRunAt) && nextRunAt > 0
              ? t('下次 {date}', { date: formatTime(nextRunAt) })
              : '—'}
          </span>
          <Button
            size='sm'
            variant='ghost'
            disabled={busy || store.runningId !== null}
            onClick={() => void runScheduleNow(entry.id)}
          >
            {busy ? t('执行中…') : t('立即执行')}
          </Button>
          <Button
            size='sm'
            variant='ghost'
            onClick={() => setEditingId(expanded ? null : entry.id)}
          >
            {expanded ? t('收起') : t('编辑')}
          </Button>
          <Button size='sm' variant='ghost' className='text-destructive' disabled={store.saving} onClick={() => void remove()}>
            {t('删除')}
          </Button>
        </div>
      </div>
      {expanded ? <ScheduleEditor entry={entry} store={store} /> : null}
    </div>
  )
}

/* ─── 保活模型链（清单编辑器：增 / 删 / 按住拖动排序）───── */

/** 两份清单是否逐项相同（判「草稿有没有真改过」） */
function sameModels(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((item, index) => item === b[index])
}

/**
 * 拖动状态放在**模块级**：指针监听挂在 window 上，回调里拿不到组件闭包。
 *
 * ── 为什么拖动是「换一份新数组」而不是自己搬 DOM ─────────────
 * 清单由 React 按 `key={模型名}` 渲染，拖动时做的只是把数组换一份新的 ——
 * React 会**移动既有节点**（不重建），所以拖动中 row 的 DOM 身份不变、
 * 后面的落点判定始终量的还是那一行。自己搬 DOM 的话下一次 diff 就会把顺序打回去。
 */
type ChainDrag = {
  /** 当前顺序（每次落点变化就换一份新的） */
  list: string[]
  /** 被拖的行现在在第几位 */
  from: number
  row: HTMLElement
  onChange: (models: string[]) => void
}

let chainDrag: ChainDrag | null = null

/** 指针落在第几位：拿其余各行的中线比（与账号页顺序拖拽同一口径） */
function chainInsertIndex(others: HTMLElement[], clientY: number): number {
  for (let index = 0; index < others.length; index += 1) {
    const rect = others[index].getBoundingClientRect()
    if (clientY < rect.top + rect.height / 2) return index
  }
  return others.length
}

function onChainPointerMove(event: PointerEvent): void {
  const state = chainDrag
  if (!state) return
  // 指针中途被放开（没接到 pointerup）时收尾，别留一个卡住的拖动态
  if (event.buttons === 0) { finishChainDrag(); return }
  const container = state.row.parentElement
  if (!container) return
  const rows = [...container.querySelectorAll<HTMLElement>('.ck-chain-row')]
  const others = rows.filter(row => row !== state.row)
  const to = chainInsertIndex(others, event.clientY)
  if (to === state.from) return
  const next = [...state.list]
  const [moved] = next.splice(state.from, 1)
  next.splice(to, 0, moved)
  // 就地记住新秩序：下一次落点判定要用它，而不是等 React 回灌
  state.list = next
  state.from = to
  state.onChange(next)
}

function finishChainDrag(): void {
  const state = chainDrag
  chainDrag = null
  window.removeEventListener('pointermove', onChainPointerMove)
  window.removeEventListener('pointerup', finishChainDrag)
  window.removeEventListener('pointercancel', finishChainDrag)
  document.body.classList.remove('ck-chain-dragging')
  state?.row.classList.remove('dragging')
}

/** 按住把手起拖（把手是**唯一**的拖动起点：行里还有移除按钮，整行可拖会点错） */
function startChainDrag(
  event: React.PointerEvent<HTMLElement>,
  index: number,
  models: string[],
  onChange: (models: string[]) => void,
): void {
  if (event.button !== 0 || chainDrag) return
  if (models.length < 2) return
  const row = (event.currentTarget as HTMLElement).closest<HTMLElement>('.ck-chain-row')
  if (!row) return
  event.preventDefault()
  chainDrag = { list: [...models], from: index, row, onChange }
  row.classList.add('dragging')
  document.body.classList.add('ck-chain-dragging')
  window.addEventListener('pointermove', onChainPointerMove)
  window.addEventListener('pointerup', finishChainDrag)
  window.addEventListener('pointercancel', finishChainDrag)
}

/**
 * 保活模型链面板。
 *
 * ── 为什么从一行输入框改成清单 ──────────────────────────────
 * 原来是一个用「、」分隔的文本框，加一个模型要挪光标、删一个要精确选中、想换顺序
 * 得手动剪贴 —— 而这条链的语义恰恰**就是顺序**（按顺序逐个尝试、第一个成功即止）。
 * 现在：一行一个模型，+ 添加、× 移除、按住把手拖动排序，整份清单一次保存
 * （清空后保存 = 恢复缺省链）。
 */
function KeepalivePanel({ models, defaultModels, dirty, saving }: {
  models: string[]
  defaultModels: string[]
  dirty: boolean
  saving: boolean
}) {
  const [input, setInput] = React.useState('')
  const add = (): void => {
    const name = input.trim()
    if (!name) return
    if (models.includes(name)) {
      toast(t('清单里已经有「{name}」', { name }), 'err')
      return
    }
    setKeepaliveDraft([...models, name])
    setInput('')
  }
  return (
    <section className='panel ck-chain-panel'>
      <div className='panel-head'>
        <h2>{t('保活模型链')}</h2>
        <span className='panel-sub'>{t('WorkBuddy 国际版日活任务的免费模型 · 全局一份，与条目无关')}</span>
        <Button
          size='sm'
          variant='default'
          className='ml-auto'
          disabled={!dirty || saving}
          onClick={() => void submitKeepaliveModels(models)}
        >
          {saving ? t('保存中…') : t('保存')}
        </Button>
      </div>
      <div className='p-4'>
        <div className='ck-set-line ck-set-line-top'>
          <span className='ck-set-k'>{t('模型清单')}</span>
          <span className='ck-set-v'>
            {/* 计数与「填入缺省链」并排放在清单上方：原来想回到缺省链只能一个个 ×
                删空了再保存，现在一键填好（仍然要点「保存」才生效） */}
            <div className='ck-mini-row'>
              <span className='ck-mini-note'>{t('按序逐个尝试，共 {n} 个', { n: models.length })}</span>
              <button
                type='button'
                className='ck-mini-btn'
                disabled={saving || defaultModels.length === 0}
                title={t('把缺省链填进清单，再点「保存」生效')}
                onClick={() => setKeepaliveDraft([...defaultModels])}
              >
                {t('填入缺省链')}
              </button>
            </div>
            <div className='ck-chain'>
              {models.map((model, index) => (
                <div className='ck-chain-row' key={model}>
                  <span
                    className='ck-chain-grip'
                    role='button'
                    tabIndex={-1}
                    aria-label={t('按住拖动调整顺序')}
                    title={t('按住拖动，调整这个模型在链里的顺序')}
                    onPointerDown={event => startChainDrag(event, index, models, setKeepaliveDraft)}
                    dangerouslySetInnerHTML={{ __html: shared().wbIcons?.icon?.('gripVertical', 14) || '' }}
                  />
                  <span className='ck-chain-name' title={model}>{model}</span>
                  <span className='ck-chain-index'>#{index + 1}</span>
                  <button
                    type='button'
                    className='ck-chain-del'
                    disabled={saving}
                    aria-label={t('移除 {name}', { name: model })}
                    title={t('从清单里移除')}
                    onClick={() => setKeepaliveDraft(models.filter((_, keep) => keep !== index))}
                  >
                    ×
                  </button>
                </div>
              ))}
              {models.length === 0 ? (
                <div className='ck-chain-empty'>{t('清单为空 —— 保存后恢复缺省链')}</div>
              ) : null}
            </div>
            <div className='ck-chain-add'>
              <Input
                type='text'
                className='max-w-[260px] font-mono text-xs'
                placeholder={t('WorkBuddy 国际版保活模型，按顺序回退')}
                value={input}
                disabled={saving}
                onChange={event => setInput(event.currentTarget.value)}
                onKeyDown={event => {
                  if (event.key === 'Enter') { event.preventDefault(); add() }
                }}
              />
              <Button size='sm' variant='outline' disabled={saving || input.trim() === ''} onClick={add}>
                {t('添加模型')}
              </Button>
              {dirty ? <span className='ck-chain-dirty'>{t('有未保存的改动')}</span> : null}
            </div>
          </span>
        </div>
        <div className='ck-note'>
          {t('「保活模型链」是 WorkBuddy 国际版领日活时用来保活的免费模型，按顺序逐个尝试、 第一个成功即止；清空清单并保存 = 恢复缺省链（当前缺省：{defaults}）。 当天去重 + 幂等领取：重复执行只会拿到「已领取」，不会重复加分。', {
            defaults: defaultModels.join(t('、')),
          })}
        </div>
      </div>
    </section>
  )
}

/* ─── 页面 ─────────────────────────────────── */

function ScheduleTab(): React.ReactElement {
  const store = useSchedulesStore()
  const checkin = useCheckinSnapshot()
  // 挂载即拉一次（切到本标签页 = 重新挂载，天然刷新）
  React.useEffect(() => { void loadSchedules() }, [])

  if (!store.loaded) {
    return (
      <div className='ck-loading'>
        <Spinner className='size-5' />
        <span>{t('正在读取定时签到数据…')}</span>
      </div>
    )
  }

  const data = store.data
  const masterOn = data?.enabled === true
  const entries = Array.isArray(data?.schedules) ? data!.schedules! : []
  // 保活模型链（全局一份，随卡迁入本页）：草稿非空表示用户正在编辑，否则跟随快照
  const keepalive = checkin.snapshot?.keepalive
  const keepaliveModels = Array.isArray(keepalive?.models) ? keepalive!.models : []
  const defaultModels = Array.isArray(keepalive?.defaultModels) ? keepalive!.defaultModels : []
  const chainDraft = checkin.keepaliveDraft
  const chainModels = chainDraft ?? keepaliveModels
  const chainDirty = chainDraft !== null && !sameModels(chainDraft, keepaliveModels)

  return (
    <div>
      {store.loadError ? (
        <div className='ck-page-error'>
          <span>{t('读取定时签到数据失败：{error}', { error: store.loadError })}</span>
          <Button size='sm' variant='outline' onClick={() => void loadSchedules()}>{t('重试')}</Button>
        </div>
      ) : null}

      <section className='panel'>
        <div className='panel-head'>
          <h2>{t('定时签到')}</h2>
          <span className='tip-q' data-tip={AUTO_CHECKIN_DESC}></span>
          <span className='panel-sub'>{t('每条条目各有时刻与范围 · 各自当天去重 · 错过时点开机自动补签')}</span>
          {/* 新建入口放卡头（原先是卡尾的一颗按钮：条目一多就要滚到底才看得见，
              而它恰恰是「这个页面第一件要做的事」） */}
          <Button
            size='sm'
            variant='outline'
            className='ml-auto flex-none'
            disabled={store.saving}
            onClick={() => setEditingId(store.editingId === 'new' ? null : 'new')}
          >
            {store.editingId === 'new' ? t('收起新建') : t('添加条目')}
          </Button>
        </div>
        <div className='p-4'>
          <div className='ck-set-line'>
            <span className='ck-set-k'>{t('总开关')}</span>
            <span className='ck-set-v flex items-center gap-2'>
              <Switch
                checked={masterOn}
                disabled={store.saving}
                onCheckedChange={next => void toggleMaster(next === true)}
              />
              <span className='text-subtle text-xs'>
                {masterOn ? t('开着：启用的条目按各自时刻执行') : t('关着：所有条目都不执行')}
              </span>
            </span>
          </div>
        </div>
        <div>
          {store.editingId === 'new' ? <NewEntryRow store={store} /> : null}
          {entries.map(entry => <ScheduleEntryRow key={entry.id} entry={entry} store={store} />)}
          {entries.length === 0 ? (
            <div className='ck-empty'>{t('还没有定时条目 —— 点「添加条目」建一条，或到「签到中心」手动签到。')}</div>
          ) : null}
        </div>
      </section>

      <KeepalivePanel
        models={chainModels}
        defaultModels={defaultModels}
        dirty={chainDirty}
        saving={checkin.keepaliveSaving}
      />
    </div>
  )
}

export { ScheduleTab }
