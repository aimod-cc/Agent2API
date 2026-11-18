import * as React from 'react'
import { Button, Card, CardHeader, Empty, Input, Switch } from '@ui'
import { patch, saveKeepaliveModels, saveTravel, runTravelNow, store } from './checkin-store'
import { formatTime, type CheckinCenter } from './checkin-types'
import { t } from '../i18n'
import { callJson } from './assistant-state'
import { ModelApiKeyField } from '../../../ui-shared/model-api-key-field'
import { useModelApiKeys } from '../../../ui-shared/use-model-api-keys'
import { CheckinPluginTasks } from './checkin-plugin-tasks'

/* ─── 保活模型链（国际版）──────────────────── */

/**
 * 拖动排序：模块级状态。
 *
 * 拖动期间**只动 DOM**（`insertBefore`），松手才把最终顺序写进 state。
 * 每次移动都写 state 会让 React 重渲、行节点被替换，手里那个节点变成孤儿
 * （`getBoundingClientRect()` 全 0），后续落点判定全错 —— 这是旧 iframe 版
 * 真实踩过的坑。
 */
type DragState = {
  container: HTMLElement
  row: HTMLElement
  models: string[]
  onChange: (next: string[]) => void
}

let drag: DragState | null = null

function insertIndex(rows: HTMLElement[], clientY: number): number {
  for (let index = 0; index < rows.length; index += 1) {
    const rect = rows[index].getBoundingClientRect()
    if (clientY < rect.top + rect.height / 2) return index
  }
  return rows.length
}

function onPointerMove(event: PointerEvent): void {
  if (!drag) return
  if (event.buttons === 0) { finishDrag(); return }
  const rows = [...drag.container.querySelectorAll<HTMLElement>('.ck-chain-row')]
    .filter(row => row !== drag!.row)
  const reference = rows[insertIndex(rows, event.clientY)] ?? null
  if (reference === drag.row.nextElementSibling) return
  if (reference === null && drag.row === drag.container.lastElementChild) return
  drag.container.insertBefore(drag.row, reference)
}

function finishDrag(): void {
  const state = drag
  drag = null
  window.removeEventListener('pointermove', onPointerMove)
  window.removeEventListener('pointerup', finishDrag)
  window.removeEventListener('pointercancel', finishDrag)
  document.body.classList.remove('ck-chain-dragging')
  if (!state) return
  state.row.classList.remove('dragging')
  // 行上的 `data-chain-index` 记的是**原数组**下标：按 DOM 顺序读回来就是新顺序
  const order = [...state.container.querySelectorAll<HTMLElement>('.ck-chain-row')]
    .map(row => Number(row.dataset.chainIndex))
    .filter(index => Number.isInteger(index))
    .map(index => state.models[index])
  const changed = order.length === state.models.length
    && order.some((model, index) => model !== state.models[index])
  if (changed) state.onChange(order)
}

function startDrag(event: React.PointerEvent<HTMLSpanElement>, index: number, models: string[], onChange: (next: string[]) => void): void {
  if (event.button !== 0 || drag) return
  if (models.length < 2) return
  const row = event.currentTarget.closest<HTMLElement>('.ck-chain-row')
  const container = row?.parentElement
  if (!row || !container) return
  event.preventDefault()
  drag = { container, row, models: [...models], onChange }
  row.classList.add('dragging')
  document.body.classList.add('ck-chain-dragging')
  window.addEventListener('pointermove', onPointerMove)
  window.addEventListener('pointerup', finishDrag)
  window.addEventListener('pointercancel', finishDrag)
}

/** 拖动把手（六点两列，与账号页顺序拖拽同一枚） */
function GripIcon(): React.ReactElement {
  return (
    <svg viewBox="0 0 24 24" width="14" height="14" fill="currentColor" aria-hidden="true">
      <circle cx="9" cy="6" r="1.5" /><circle cx="15" cy="6" r="1.5" />
      <circle cx="9" cy="12" r="1.5" /><circle cx="15" cy="12" r="1.5" />
      <circle cx="9" cy="18" r="1.5" /><circle cx="15" cy="18" r="1.5" />
    </svg>
  )
}

function KeepalivePanel({ center }: { center: CheckinCenter }): React.ReactElement {
  const keyState = useModelApiKeys(() => callJson('GET', '/api/model-access/keys'))
  const current = center.keepalive
  const models = store.keepaliveDraft ?? current.models
  const defaults = current.defaultModels ?? []
  const dirty = store.keepaliveDraft !== null
    && (store.keepaliveDraft.length !== current.models.length
      || store.keepaliveDraft.some((item, index) => item !== current.models[index]))
  const [input, setInput] = React.useState('')

  const setDraft = (next: string[]) => patch({ keepaliveDraft: next })

  const add = () => {
    const name = input.trim()
    if (!name) return
    if (models.includes(name)) return
    setDraft([...models, name])
    setInput('')
  }

  return (
    <Card render={<section />} className="panel ck-chain-panel">
      <CardHeader className="panel-head">
        <h2>保活模型链</h2>
        <span className="panel-sub">WorkBuddy 国际版日活任务的免费模型 · 全局一份，与条目无关</span>
        <Button
          size="sm"
          variant="default"
          className="ml-auto"
          disabled={!dirty || store.saving}
          onClick={() => void saveKeepaliveModels(models)}
        >
          {store.saving ? '保存中…' : '保存'}
        </Button>
      </CardHeader>
      <div className="ck-panel-pad">
        <ModelApiKeyField {...keyState} value={current.apiKeyId ?? null} disabled={store.saving}
          onChange={apiKeyId => { void saveKeepaliveModels(models, apiKeyId) }}
          onRefresh={() => { void keyState.refresh().catch(() => {}) }} t={t} id='keepalive-api-key' />
        {current.apiKeyDefaulted
          ? <p className='text-xs leading-[1.6] text-subtle'>
              {t('默认使用第一把可用 API Key，想换就在上面选一把')}
            </p>
          : null}
        {current.authError ? <p role='status' className='text-xs text-destructive'>{current.authError}</p> : null}
        <div className="ck-set-line ck-set-line-top">
          <span className="ck-set-k">模型清单</span>
          <span className="ck-set-v">
            <div className="ck-mini-row">
              <span className="ck-mini-note">按序逐个尝试，共 {models.length} 个</span>
              <Button size='default' variant='ghost'
                type="button"
                className="ck-mini-btn"
                disabled={store.saving || defaults.length === 0}
                title="把缺省链填进清单，再点「保存」生效"
                onClick={() => setDraft([...defaults])}
              >
                填入缺省链
              </Button>
            </div>
            <div className="ck-chain">
              {models.map((model, index) => (
                <div className="ck-chain-row" key={model} data-chain-index={index}>
                  <span
                    className="ck-chain-grip"
                    role="button"
                    aria-label="按住拖动调整顺序"
                    title="按住拖动，调整这个模型在链里的顺序"
                    onPointerDown={event => startDrag(event, index, models, setDraft)}
                  >
                    <GripIcon />
                  </span>
                  <span className="ck-chain-name" title={model}>{model}</span>
                  <span className="ck-chain-index">#{index + 1}</span>
                  <Button size='default' variant='ghost'
                    type="button"
                    className="ck-chain-del"
                    disabled={store.saving}
                    aria-label={`移除 ${model}`}
                    title="从清单里移除"
                    onClick={() => setDraft(models.filter((_, keep) => keep !== index))}
                  >
                    ×
                  </Button>
                </div>
              ))}
              {models.length === 0
                ? <Empty className="ck-chain-empty">清单为空 —— 保存后恢复缺省链</Empty>
                : null}
            </div>
            <div className="ck-chain-add">
              <Input
                type="text"
                className="ck-chain-input font-mono text-xs"
                placeholder="WorkBuddy 国际版保活模型，按顺序回退"
                value={input}
                disabled={store.saving}
                onChange={event => setInput(event.currentTarget.value)}
                onKeyDown={event => {
                  if (event.key !== 'Enter') return
                  event.preventDefault()
                  add()
                }}
              />
              <Button size="sm" variant="outline" disabled={store.saving || input.trim() === ''} onClick={add}>
                添加模型
              </Button>
              {dirty ? <span className="ck-chain-dirty">有未保存的改动</span> : null}
            </div>
          </span>
        </div>
        <div className="ck-note">
          「保活模型链」是 WorkBuddy 国际版领日活时用来保活的免费模型，按顺序逐个尝试、
          第一个成功即止；清空清单并保存 = 恢复缺省链（当前缺省：
          {defaults.length ? defaults.join('、') : '（无）'}）。
          当天去重 + 幂等领取：重复执行只会拿到「已领取」，不会重复加分。
        </div>
      </div>
    </Card>
  )
}

/* ─── 猫猫旅行排程（国内版）────────────────── */

const TRAVEL_TIP = '每天按这里的时刻自动巡检一遍 WorkBuddy 国内版账号：没有猫就领养、'
  + '在家就派出、到站就领奖。一趟只推进一步，所以建议留两个时刻'
  + '（早上派出、晚上领奖）。企业版与国际版账号自动跳过。'

function TravelPanel({ center }: { center: CheckinCenter }): React.ReactElement {
  const travel = center.travel
  const times = travel.times ?? []
  const fired = travel.firedToday ?? []
  const draft = store.travelTimesDraft
  const text = draft ?? times.join(', ')
  const dirty = draft !== null && draft.replace(/\s+/g, '') !== times.join(',')
  const last = travel.lastResult
  const actions = Array.isArray(last?.actions) ? last.actions : []
  const isPaused = travel.available === false
  const failed = Number(last?.failed) || 0

  const submit = () => {
    const list = String(text)
      .split(/[,，\s]+/)
      .map(item => item.trim())
      .filter(Boolean)
    void saveTravel({ times: list })
  }

  return (
    <Card render={<section />} className="panel">
      <CardHeader className="panel-head">
        <h2>猫猫旅行定时排程</h2>
        {/* 问号提示：宿主有 desktop-tooltip.tsx，`data-tip` 是它的契约 —— 迁到宿主后
            这条提示才真正长得和别的页一样（iframe 里拿不到那个脚本） */}
        <span className="tip-q" data-tip={TRAVEL_TIP} />
        <span className="panel-sub">每天定点巡检 · 一趟推进一步 · 与签到排程互相独立</span>
        <Button
          size="sm"
          variant="outline"
          className="ml-auto flex-none"
          disabled={travel.running || isPaused || store.saving}
          onClick={() => void runTravelNow()}
        >
          {travel.running ? '巡检中…' : '立即巡检全部'}
        </Button>
      </CardHeader>
      <div className="ck-panel-pad">
        {isPaused ? (
          <div className="ck-note">
            {t('自动巡检已暂停：{reason}', { reason: travel.pausedReason ?? '' })}
          </div>
        ) : null}
        <div className="ck-set-line">
          <span className="ck-set-k">开关</span>
          <span className="ck-set-v ck-set-row">
            <Switch
              checked={travel.enabled}
              disabled={store.saving}
              onCheckedChange={checked => void saveTravel({ enabled: checked === true })}
            />
            <span className="ck-sub">
              {travel.enabled ? '开着：按下面的时刻自动巡检' : '关着：不自动巡检（仍可手动巡检）'}
            </span>
          </span>
        </div>
        <div className="ck-set-line">
          <span className="ck-set-k">时刻</span>
          <span className="ck-set-v ck-set-row">
            <Input
              className="ck-travel-times"
              placeholder="09:00, 21:00"
              value={text}
              disabled={store.saving}
              onChange={event => patch({ travelTimesDraft: event.currentTarget.value })}
            />
            <Button size="sm" variant="outline" disabled={!dirty || store.saving} onClick={submit}>
              保存
            </Button>
          </span>
        </div>
        <div className="ck-note">
          北京时间 · 逗号分隔 · 最多 6 条 · 到点后最多晚 30 秒执行；错过时点开机后会补跑一轮。
          {fired.length ? ` 今天已执行：${fired.join(' / ')}` : ''}
        </div>
        {last && (actions.length > 0 || last.error) ? (
          <div className="ck-note">
            {last.error ? t('巡检失败：{reason}', { reason: last.error }) : (
              <>
                上一轮（{last.at ? formatTime(last.at) : ''}）：{actions.length} 个动作，+{Number(last.credit) || 0} 积分
                {failed > 0 ? ` · ${t('失败 {n}', { n: failed })}` : ''}
                {actions.slice(0, 6).map(action =>
                  ` · ${action.name ?? action.accountId ?? ''}: ${action.message ?? action.outcome ?? ''}`).join('')}
              </>
            )}
          </div>
        ) : null}
      </div>
    </Card>
  )
}

export function CheckinScheduleExtras({ center }: { center: CheckinCenter }): React.ReactElement | null {
  const panels = new Set(center.adminPanels ?? [])
  const keepalive = panels.has('daily_keepalive') ? <KeepalivePanel center={center} /> : null
  const travel = panels.has('growth_tasks') && center.travel.engine !== 'wasm_tasks' ? <TravelPanel center={center} /> : null
  if (!keepalive && !travel && !center.pluginTasks?.length) return null
  return <section className='ck-schedule-extras'>
    <div className='ck-schedule-heading'><div><h2>{t('其他自动任务')}</h2><p>{t('保活模型链为全局配置，猫猫旅行有独立的开关与时刻。')}</p></div></div>
    {keepalive || travel ? <div className='ck-region-row'>{keepalive}{travel}</div> : null}
    <CheckinPluginTasks center={center} expanded />
  </section>
}
