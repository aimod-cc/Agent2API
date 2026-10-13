/**
 * Agent2API · 「添加账号」弹窗 → 「导入」分段：从其他工具批量导入供应商。
 *
 * 替换旧 ui/add-provider-import.js。当前来源只有 **cc-switch**（后端
 * `GET /api/import/cc-switch` 扫描本机 SQLite，见 core::import_ccswitch），
 * new-api / sub2api 待续 —— 届时在 scan 结果上并成多来源列表即可，
 * 「扫描 → 勾选 → 批量创建」三段结构不用动。
 *
 * ── 为什么这一段不点卡片进第 2 步 ────────────────────────────
 * 导入进来的每一家都对应一个「新建自定义提供商 + 首个账号」，没有需要用户逐条填的
 * 字段（名称 / 协议 / Base URL / Key 全部来自被导入的配置），所以勾选完直接提交。
 * 面板占的是第 1 步卡片网格那块位置，底部主按钮走**列表弹窗**那条操作条。
 *
 * ── 状态为什么放在模块级 ────────────────────────────────────
 * 旧实现的状态（扫描结果 / 勾选集合）是模块级变量，切走再切回不重扫、勾选原样保留。
 * 岛上沿用同一取向：状态在这里、组件用 useSyncExternalStore 订阅，于是
 * 「切到别的分段再切回来」与「关掉弹窗再打开」都保持旧行为。
 *
 * 注：这个分段当前在界面上收起（IMPORT_SEGMENT_ENABLED = false，见
 * add-provider-pick.tsx），实现整块留着，打开那个开关即可用。
 */

import * as React from 'react'
import { Button, Checkbox } from '@ui'

import { shared, toast } from './add-account-bridge'
import { t } from '../i18n'

type ImportProvider = {
  id: string | number
  name?: string
  protocol?: string
  baseUrl?: string
  apiKey?: string
  models?: unknown[]
  modelMappings?: Array<{ alias?: string; target?: string }>
  importable?: boolean
  reason?: string
  notes?: string
}

type ImportScan = {
  available?: boolean
  reason?: string
  path?: string
  providers?: ImportProvider[]
}

type ImportState = {
  /** '' = 还没扫过（首次切入时触发）；'loading'；'done' */
  scanState: '' | 'loading' | 'done'
  scan: ImportScan | null
  /** 勾选集合（扫描条目 id；只有 importable 的条目会出现在集合里） */
  checked: Set<string>
  /** 导入进行中：与底部条的其他提交动作同一把锁的语义（各自独立，不共享） */
  busy: boolean
  /** 失败行原地标注的原因（旧实现直接改那一行的 DOM，这里记在状态里） */
  failures: Map<string, string>
  /** 底部条上的失败提示（toast 几秒后就没了，这里留一份） */
  footHint: string
}

let state: ImportState = {
  scanState: '',
  scan: null,
  checked: new Set(),
  busy: false,
  failures: new Map(),
  footHint: '',
}

const listeners = new Set<() => void>()

function patch(next: Partial<ImportState>): void {
  state = { ...state, ...next }
  for (const listener of listeners) listener()
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => { listeners.delete(listener) }
}

function useImportState(): ImportState {
  return React.useSyncExternalStore(subscribe, () => state)
}

const request = (method: string, path: string, body?: unknown): Promise<unknown> | undefined =>
  shared().wbProviders?.customRequest?.(method, path, body)

/** 扫描本机 cc-switch（失败在状态区给原因，不 toast —— 空态本身就是结果） */
async function runScan(options: { force?: boolean } = {}): Promise<void> {
  const force = options.force === true
  if (state.busy) return
  if (state.scanState === 'loading' || (state.scanState === 'done' && !force)) return
  patch({ scanState: 'loading', failures: new Map() })
  let scan: ImportScan
  try {
    const data = (await request('GET', '/api/import/cc-switch')) as ImportScan | null
    scan = data || { available: false, reason: t('响应为空') }
  } catch (error) {
    scan = {
      available: false,
      reason: error instanceof Error ? error.message : String(error),
    }
  }
  // 首次扫描默认全选可导入项（之后的重扫保留用户已勾的）
  const checked = new Set(state.checked)
  if (!checked.size) {
    for (const item of scan.providers || []) {
      if (item.importable === true) checked.add(String(item.id))
    }
  }
  patch({ scanState: 'done', scan, checked })
}

/** 协议值的展示名（扫描条目上的小徽标） */
function protocolLabel(protocol?: string): string {
  if (protocol === 'anthropic') return 'Anthropic'
  if (protocol === 'responses') return 'Responses'
  return String(protocol || '')
}

/** 行副标题里那句「模型 …」：清单会长，超过两个就折叠成「等 N 个」 */
function modelNote(item: ImportProvider): string {
  const models = (Array.isArray(item?.models) ? item.models : [])
    .map(entry => String(entry || '').trim())
    .filter(Boolean)
  if (!models.length) return ''
  const head = models.slice(0, 2).join(t('、'))
  const rest = models.length > 2 ? t(' 等 {n} 个', { n: models.length }) : ''
  return t(' · 模型 {head}{rest}', { head, rest })
}

/**
 * 把 cc-switch 配置里配的模型登记成新家的**初始模型清单**（含 `[1M]` 别名映射）。
 *
 * 为什么必须做：自定义家的模型清单是路由的前提 —— 没登记过的模型名会被直接拒掉
 * （custom_providers::bindings::resolve 只在 models / mappings 里找）。cc-switch 里
 * claude 类有 ANTHROPIC_MODEL 与档位覆盖、codex 类有 model，都是用户当时在用的
 * 模型名，顺手带过来新家开箱能用。
 *
 * 清单写失败**不推翻已建的家**：提供商与账号已经落盘，模型管理页可以补登记；
 * 这里失败只留一条控制台记录，导入计数照常 +1。
 */
async function saveModels(created: unknown, item: ImportProvider): Promise<void> {
  const providerId = (created as { provider?: { id?: string } } | null)?.provider?.id
  const models = (Array.isArray(item?.models) ? item.models : [])
    .map(entry => String(entry || '').trim())
    .filter(Boolean)
    .map(id => ({ id, enabled: true, reasoning: '' }))
  if (!providerId || !models.length) return
  const mappings = (Array.isArray(item?.modelMappings) ? item.modelMappings : [])
    .map(pair => ({
      alias: String(pair?.alias || '').trim(),
      target: String(pair?.target || '').trim(),
    }))
    .filter(pair => pair.alias && pair.target
      && pair.alias.toLowerCase() !== pair.target.toLowerCase())
    .map(pair => ({ ...pair, enabled: true, reasoning: '' }))
  try {
    await request('POST', '/api/custom-providers/models', { providerId, models, mappings })
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error)
    console.warn('导入时写入初始模型清单失败:', message)
  }
}

/**
 * 执行导入：把勾选的每条 cc-switch 配置创建成自定义提供商 + 首个账号
 * （复用 POST /api/custom-providers，一条请求建齐）。逐条提交、逐条记账：
 * 单条失败不拖累其余（失败的行标出原因留在列表里可重试），全部成功才关弹窗。
 */
async function runImport(): Promise<void> {
  const scan = state.scan
  const selected = (Array.isArray(scan?.providers) ? scan.providers : [])
    .filter(item => item.importable === true && state.checked.has(String(item.id)))
  if (!selected.length || state.busy) return
  const failures = new Map(state.failures)
  const checked = new Set(state.checked)
  patch({ busy: true, footHint: '', failures })
  let ok = 0
  for (const item of selected) {
    const payload = {
      name: String(item.name || ''),
      protocol: String(item.protocol || 'chat_completions'),
      baseUrl: String(item.baseUrl || ''),
      apiKey: String(item.apiKey || ''),
    }
    try {
      const created = await request('POST', '/api/custom-providers', payload)
      await saveModels(created, item)
      checked.delete(String(item.id))
      ok += 1
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      failures.set(String(item.id), message)
    }
  }
  // 有成功的就作废扫描缓存：下次进入本段重扫一次，刚导入的家会被后端判成
  // 「同名跳过」而不是又一份「可导入」—— 否则用户再点一次就是重复创建。
  patch({
    busy: false,
    checked,
    failures,
    scanState: ok > 0 ? '' : state.scanState,
  })
  if (ok > 0) {
    // 目录与账号列表都要刷：账号行 / 筛选器 / 模型管理页左栏都读它们
    void shared().wbProviders?.refreshCustom?.()
    await shared().wbApp?.refresh?.()
  }
  const failedCount = selected.length - ok
  if (ok > 0 && !failedCount) {
    shared().wbAddAccountModal?.close?.()
    toast(t('✅ 已从 cc-switch 导入 {n} 个供应商', { n: ok }))
  } else if (ok > 0) {
    toast(t('已导入 {ok} 个，失败 {fail} 个（失败项已标注在列表里）', { ok, fail: failedCount }), 'err')
  } else {
    const first = [...failures.values()][0]
    patch({ footHint: first || t('导入失败') })
  }
}

/* ─── 面板（第 1 步的内容区）────────────────── */

/** 面板显隐由岛按分段切换（旧实现是 setSegment），首次切入触发扫描 */
export function ImportPanel({ segmentOn }: { segmentOn: boolean }): React.ReactElement {
  const current = useImportState()
  const style: React.CSSProperties = segmentOn ? {} : { display: 'none' }

  React.useEffect(() => {
    if (segmentOn) void runScan()
  }, [segmentOn])

  if (current.scanState === 'loading') {
    return (
      <div id='add-import-panel' style={style}>
        <div id='add-import-status'>{t('正在扫描本机 cc-switch…')}</div>
        <div id='add-import-list' />
      </div>
    )
  }

  const scan = current.scan
  const providers = Array.isArray(scan?.providers) ? scan.providers : []
  const importable = providers.filter(item => item.importable === true)

  return (
    <div id='add-import-panel' style={style}>
      <div id='add-import-status'>
        {!scan || !scan.available ? (
          <div className='add-import-empty'>
            <p>{scan?.reason || t('未能读取 cc-switch 数据')}</p>
            <Button variant='outline' size='sm' onClick={() => { void runScan({ force: true }) }}>
              {t('重新扫描')}
            </Button>
          </div>
        ) : (
          <div className='add-import-head'>
            <span>
              {t('在 cc-switch 里找到')} <b>{providers.length}</b> {t('条配置，其中')}{' '}
              <b>{importable.length}</b> {t('条可导入')}
            </span>
            <Button variant='ghost' size='xs' className='linkish' onClick={() => { void runScan({ force: true }) }}>
              {t('重新扫描')}
            </Button>
          </div>
        )}
      </div>
      <div id='add-import-list'>
        {providers.map(item => {
          const id = String(item.id)
          const canImport = item.importable === true
          const isChecked = current.checked.has(id)
          const failed = current.failures.get(id)
          // 模型名一并展示：它会被登记成新家的初始清单，用户看得见「带过来了什么」
          const meta = failed
            ? t('导入失败：{reason}', { reason: failed })
            : canImport
              ? String(item.baseUrl || '') + modelNote(item)
              : String(item.reason || t('不支持导入'))
          const toggle = (): void => {
            const checked = new Set(state.checked)
            if (checked.has(id)) checked.delete(id)
            else checked.add(id)
            patch({ checked })
          }
          return (
            // 行本身可点（旧实现是 <label> 包住原生 checkbox）：Checkbox 是 Base UI 的
            // button，套进 label 会形成「label 里嵌可交互元素」的非法结构，因此改成
            // div + 行点击，并在 Checkbox 上停掉冒泡避免点一次触发两遍
            <div
              key={id}
              className={`add-import-row${canImport && !failed ? '' : ' disabled'}`}
              title={canImport ? String(item.notes || '') : String(item.reason || '')}
              onClick={() => { if (canImport) toggle() }}
            >
              {/* 失败的行保留勾选框（旧实现只给它加 .disabled 与失败说明）：
                  它仍在勾选集合里，再点一次「导入所选」就是重试 */}
              {canImport ? (
                <Checkbox
                  checked={isChecked}
                  aria-label={String(item.name || id)}
                  onClick={event => event.stopPropagation()}
                  onCheckedChange={toggle}
                />
              ) : (
                <span className='add-import-dash' />
              )}
              <span className='add-import-info'>
                <span className='add-import-name'>
                  {String(item.name || id)}
                  {canImport ? (
                    <span className='add-import-badge'>{protocolLabel(item.protocol)}</span>
                  ) : null}
                </span>
                <span className='add-import-meta'>{meta}</span>
              </span>
            </div>
          )
        })}
      </div>
    </div>
  )
}

/* ─── 底部操作条上的「导入所选」（列表弹窗那条）─────── */

/** 可见性 / 文案（带数量）/ 禁用态与失败提示都在这里，与旧实现的 syncFoot 同一口径 */
export function ImportFootActions(): React.ReactElement {
  const current = useImportState()
  const count = current.checked.size
  return (
    <>
      <span className={`add-foot-hint${current.footHint ? ' err' : ''}`} id='add-foot-hint'>
        {current.footHint}
      </span>
      <span className='add-foot-actions' id='add-foot-actions'>
        <Button
          id='import-foot-button'
          disabled={current.busy || !count}
          onClick={() => { void runImport() }}
        >
          {current.busy ? t('导入中…') : count ? t('导入所选（{n}）', { n: count }) : t('导入所选')}
        </Button>
      </span>
    </>
  )
}
