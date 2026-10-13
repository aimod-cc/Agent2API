/**
 * 定时签到**条目**的状态 store（签到中心「定时签到」标签页；非岛 `.ts`）。
 *
 * ── 与 checkin-state 的分工 ─────────────────────────────────
 * 签到中心的聚合快照（每日分组 / 新手福利 / 活动福利）归 `checkin-state`；
 * 本模块只管「定时签到」页的数据：条目数组 + 总开关（GET /api/checkin-schedules
 * 的响应）。总开关的写走老的 /api/auto-checkin（`saveAutoCheckin` 桥），条目的
 * 写走 /api/checkin-schedules —— 后端的分组理由见 `api::checkin_schedules` 模块头。
 *
 * 保存 / 删除 / 开关的响应都带**整份最新状态**（后端归一化后的值，含
 * displayLabel / nextRunAt），拿它直接覆盖本模块的 data，不必再跑一趟 GET；
 * 但签到中心的聚合快照（总览卡的「定时签到」卡）与顶栏徽标要跟上，所以写
 * 动作完成后顺带调一次 `loadCheckinCenter()`（不 await，静默跟进）。
 *
 * store 模式与 checkin-state 完全同款：模块级快照 + patch() 换新对象再通知。
 */

import { errorMessage, shared, toast } from './accounts-shared'
import { t } from '../i18n'
import {
  loadCheckinCenter,
  type ScheduleEntry,
  type SchedulesSnapshot,
} from './checkin-state'

export type SchedulesStore = {
  /** 每次变更 +1（React 重画判据） */
  version: number
  /** 状态（null = 还没读到 / 读失败） */
  data: SchedulesSnapshot | null
  /** 至少成功拉过一次 */
  loaded: boolean
  loadError: string
  /** 保存 / 删除在途（写控件临时禁用；总开关与条目按钮共用一把闸） */
  saving: boolean
  /** 正在「立即执行」的条目 id（该条按钮转圈） */
  runningId: string | null
  /** 展开编辑表单的条目 id；'new' = 新建表单；null = 全部收起 */
  editingId: string | null
}

let store: SchedulesStore = {
  version: 0,
  data: null,
  loaded: false,
  loadError: '',
  saving: false,
  runningId: null,
  editingId: null,
}

const subscribers = new Set<() => void>()

function emitChange(): void {
  for (const notify of subscribers) notify()
}

function patch(next: Partial<SchedulesStore>): void {
  store = { ...store, ...next, version: store.version + 1 }
  emitChange()
}

export function getSchedulesStore(): SchedulesStore {
  return store
}

export function subscribeSchedulesStore(notify: () => void): () => void {
  subscribers.add(notify)
  return () => subscribers.delete(notify)
}

/** 桥的窄读（签到两组方法在 checkin-state 的 CheckinBridge 里，这里按需再窄一份） */
type SchedulesBridge = {
  getCheckinSchedules?(): Promise<SchedulesSnapshot | null | undefined>
  saveCheckinSchedule?(patch: Record<string, unknown>): Promise<SchedulesSnapshot | null | undefined>
  deleteCheckinSchedule?(id: string): Promise<{ success?: boolean } | null | undefined>
  runCheckinSchedule?(id: string): Promise<Record<string, unknown> | null | undefined>
  saveAutoCheckin?(patch: Record<string, unknown>): Promise<SchedulesSnapshot | null | undefined>
}

function bridge(): SchedulesBridge {
  return (shared().workbuddyDesktop ?? {}) as SchedulesBridge
}

/* ─── 读 ─────────────────────────────────── */

/**
 * 拉状态。挂载 / 切入本页时调一次；写动作的响应自带新状态，不需要轮询。
 * 失败不清空已有数据（老数据还在展示，错误提示挂在页头）。
 */
export async function loadSchedules(): Promise<void> {
  try {
    const data = await bridge().getCheckinSchedules?.()
    if (!data) throw new Error(t('后端未返回定时签到数据'))
    patch({ data, loaded: true, loadError: '' })
  } catch (error) {
    patch({ loaded: true, loadError: errorMessage(error) })
  }
}

/* ─── 写 ─────────────────────────────────── */

/** 写动作的公共收尾：响应带整份状态就覆盖；签到中心快照静默跟进 */
function applySnapshot(next: SchedulesSnapshot | null | undefined): void {
  if (next) patch({ data: next, loadError: '' })
  void loadCheckinCenter()
}

/** 总开关（`autoCheckin.enabled`）：关了所有条目都不跑。走老的 /api/auto-checkin。 */
export async function toggleMaster(enabled: boolean): Promise<void> {
  if (store.saving) return
  patch({ saving: true })
  try {
    const next = await bridge().saveAutoCheckin?.({ enabled })
    applySnapshot(next)
    // 两条分开调（扫描器只认字面量首参，条件串提不到键）
    if (enabled) toast(t('✅ 定时签到已开启'))
    else toast(t('定时签到已关闭'))
  } catch (error) {
    toast(t('保存失败：{error}', { error: errorMessage(error) }), 'err')
  } finally {
    patch({ saving: false })
  }
}

/**
 * 保存一条条目（新建不带 id；编辑带 id + 全量字段 —— 后端按 id upsert，
 * 校验与归一都在后端，失败原样吐 message）。成功收起编辑表单。
 */
export async function saveSchedule(payload: {
  id?: string
  label?: string
  enabled?: boolean
  time?: string
  providers?: string[]
}): Promise<void> {
  if (store.saving) return
  patch({ saving: true })
  try {
    const next = await bridge().saveCheckinSchedule?.(payload as Record<string, unknown>)
    applySnapshot(next)
    patch({ editingId: null })
    toast(t('已保存：定时条目'))
  } catch (error) {
    toast(t('保存失败：{error}', { error: errorMessage(error) }), 'err')
  } finally {
    patch({ saving: false })
  }
}

/** 条目开关（不发整表单：带上这条的 time / providers，后端 upsert 需要时刻必填） */
export async function toggleScheduleEnabled(entry: ScheduleEntry, enabled: boolean): Promise<void> {
  await saveSchedule({
    id: entry.id,
    label: entry.label ?? '',
    enabled,
    time: entry.time ?? '',
    providers: Array.isArray(entry.providers) ? entry.providers : [],
  })
}

/**
 * 删除一条条目。确认在组件层做（wbConfirm），这里只管删。
 * 删除合成的默认条目 = 后端写空数组，「从旧字段合成」不再发生（界面照实变空）。
 */
export async function deleteSchedule(id: string): Promise<void> {
  if (store.saving) return
  patch({ saving: true })
  try {
    await bridge().deleteCheckinSchedule?.(id)
    applySnapshot(await bridge().getCheckinSchedules?.())
    toast(t('已删除定时条目'))
  } catch (error) {
    toast(t('删除失败：{error}', { error: errorMessage(error) }), 'err')
  } finally {
    patch({ saving: false })
  }
}

/**
 * 立即执行一条（与到点触发同一条 fire 路径，后端自带全局防重 —— 正在跑的
 * 时候这条给 400「签到正在执行中」）。响应是 `{...summary, state}`：summary
 * 供 toast，state 供重绘。执行完签到中心快照也要跟上（签到改了分组状态）。
 */
export async function runScheduleNow(id: string): Promise<void> {
  if (store.runningId) return
  patch({ runningId: id })
  try {
    const result = (await bridge().runCheckinSchedule?.(id)) ?? undefined
    const numberOf = (value: unknown): number => Number(value) || 0
    if (result) {
      const succeeded = numberOf(result.succeeded)
      const active = numberOf(result.active)
      const total = numberOf(result.total)
      const failed = numberOf(result.failedCount)
      const activeText = active > 0 ? t('，日活保活 {n} 个', { n: active }) : ''
      if (failed > 0) {
        toast(t('签到完成：{succeeded}/{total} 成功{active}，{failed} 个失败', {
          succeeded, total, active: activeText, failed,
        }), 'err')
      } else {
        toast(t('✅ 签到完成：{succeeded}/{total} 个账号成功领取{active}', {
          succeeded, total, active: activeText,
        }))
      }
      applySnapshot(result as SchedulesSnapshot)
    }
  } catch (error) {
    toast(t('执行失败：{error}', { error: errorMessage(error) }), 'err')
  } finally {
    patch({ runningId: null })
    await loadCheckinCenter()
  }
}

/** 展开 / 收起编辑表单（一次只开一张：再点别的条目 = 换目标） */
export function setEditingId(id: string | null): void {
  patch({ editingId: id })
}
