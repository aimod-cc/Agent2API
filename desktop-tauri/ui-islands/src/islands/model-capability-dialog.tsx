/**
 * Agent2API · 「模型能力」弹窗：编辑一条模型**对下游声明**的六个能力位。
 *
 * ── 解决什么 ────────────────────────────────────────────────
 * 下游（客户端）按 `/v1/models` 里的 `max_input_tokens` / `supports_images`
 * 这些字段决定怎么构造请求 —— 而上游目录给的值可能是错的（远程目录撒谎、
 * 静态表跟不上上游调整），自定义家则是**整个键都没有**。以前纠正它的唯一
 * 手段是改代码；这个弹窗就是那层纠正在界面上的入口（存储与出口见后端
 * `core::capability` 的模块头）。
 *
 * ── 三态怎么呈现（与存储协议一一对应）───────────────────────
 * 每一项都是「继承 / 覆盖」两态，数值与布尔各用最自然的形式表达：
 *   · 数值键（上下文窗口 / 最大输出 Token）：输入框里是**覆盖值**，留空 =
 *     继承（placeholder 给上游值）。已覆盖的字段旁挂「恢复继承」小件；
 *   · 布尔键（工具 / 图片 / 视频 / 思考）：三档分段选择 —— 继承上游 / 支持 /
 *     不支持。
 * 自定义家没有「上游」，两处的「继承」换成「未声明 / 清除」文案（见
 * `custom` 分支）—— 那家的能力全是用户手填的。
 *
 * ── 保存 = 六个键一次全给 ───────────────────────────────────
 * 全量提交（而不是只发改动的键）最不容易出歧义：用户在这个弹窗里看到的就是
 * 他要的结果。值的三态与后端一致：`null` = 清除覆盖 / 恢复继承、有值 = 覆盖。
 *
 * ── 弹窗生命周期 ────────────────────────────────────────────
 * 上下文只带 `(provider, id)`，行数据每次渲染从快照现读（`modelRowOf`）：
 * 弹窗开着的时候目录刷新 / 别的写操作改了数据，展示跟着新值走；行整个没了
 * （模型被移除、自定义家被删）则给一句解释并要求关闭，不让保存按钮打空靶。
 */

import * as React from 'react'
import {
  Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle,
  Input, Label, SegmentedControl, type SegmentedControlOption,
} from '@ui'

import {
  BOOLEAN_KEYS, CAPABILITY_KEYS, CAPABILITY_LABELS, TOKEN_KEYS,
  capabilitiesOf, capabilityState, exactTokens, normalizeCapability, normalizeOverrides,
  patchFromDraft, reasoningLevelsOf, type CapabilityDraft, type CapabilityKey,
} from './model-capability'
import * as customSource from './models-custom-source'
import {
  accept, errorMessage, modelRowOf, providerLabelOf, toast, writeCapabilities,
  type CapabilityContext,
} from './models-panel-state'
import { t } from '../i18n'

/** 布尔键的三档（label 随数据源换，`inheritLabel` 由调用方拼） */
const BOOLEAN_OPTIONS = (inheritLabel: string): SegmentedControlOption<string>[] => [
  { value: 'inherit', label: inheritLabel },
  { value: 'on', label: t('支持') },
  { value: 'off', label: t('不支持') },
]

/** 布尔值的展示文案（状态行与 tooltip 共用） */
function booleanText(value: number | boolean | null): string {
  const state = capabilityState(value)
  if (state === 'on') return t('支持')
  if (state === 'off') return t('不支持')
  return t('未声明')
}

/**
 * 字段下方的状态行：这一项**当前对下游**是什么、值从哪来。
 * 「未声明」要写明下游会怎么处理（布尔按不支持、数值按未知），否则用户会把
 * 「不知道」当成「明确不支持」照抄。
 */
function statusText(
  key: CapabilityKey,
  value: number | boolean | null,
  set: boolean,
  custom: boolean,
): string {
  const has = value !== null && value !== undefined
  const shown = TOKEN_KEYS.includes(key) ? exactTokens(value) : booleanText(value)
  if (set) {
    return custom
      ? t('已填写：对下游声明 {value}', { value: shown })
      : t('已覆盖：对下游声明 {value}', { value: shown })
  }
  if (custom) return t('未填写（下游按未声明处理）')
  return has ? t('继承上游：{value}', { value: shown }) : t('上游未声明（下游会按不支持处理）')
}

/**
 * 「这一项现在有没有值」的两套判据（这是两家数据源的**唯一**差异）：
 *   · 内置家：看 `capOverrides`（覆盖键列表）—— 值本身从清单来，「有没有
 *     覆盖」才是用户的状态；
 *   · 自定义家：`capOverrides` 恒空（那家没有「上游原值」这回事，见
 *     models-custom-source 的说明），判据换成「这一项填没填」—— 不这样区分，
 *     用户填过的值会被当成未设置：输入框显示为空、保存时被静默清掉。
 */
function setKeysOf(capabilities: ReturnType<typeof capabilitiesOf>, overrides: CapabilityKey[], custom: boolean): CapabilityKey[] {
  if (!custom) return overrides
  return CAPABILITY_KEYS.filter(key => {
    const value = capabilities[key]
    return value !== undefined && value !== null
  })
}

/**
 * 草稿初值：**已设置的项**给现值（数值键给数字串，布尔键给 'on' / 'off'），
 * 其余留空 / 'inherit'（数值键的 placeholder 会显示上游值）。
 * 与 `patchFromDraft` 互为逆过程。
 */
function draftFrom(capabilities: ReturnType<typeof capabilitiesOf>, setKeys: CapabilityKey[]): CapabilityDraft {
  const draft = {} as CapabilityDraft
  for (const key of CAPABILITY_KEYS) {
    const set = setKeys.includes(key)
    const value = capabilities[key]
    if (TOKEN_KEYS.includes(key)) {
      draft[key] = set && typeof value === 'number' ? String(value) : ''
    } else {
      draft[key] = set ? (value === true ? 'on' : 'off') : 'inherit'
    }
  }
  return draft
}

export function CapabilityDialog({ context, onClose }: { context: CapabilityContext; onClose: () => void }) {
  const custom = customSource.isCustom(context.provider)
  const row = modelRowOf(context.provider, context.id)
  // 模型自己能配的思考档位（`null` = 上游没声明 → 那一块整个不显示）
  const levelsInfo = reasoningLevelsOf(row)
  const capabilities = capabilitiesOf(row)
  const setKeys = setKeysOf(capabilities, normalizeOverrides(row?.capOverrides), custom)
  const [draft, setDraft] = React.useState<CapabilityDraft>(() => draftFrom(capabilities, setKeys))
  const [status, setStatus] = React.useState('')
  const [saving, setSaving] = React.useState(false)

  /** 行没了：给一句解释 + 只能关闭（保存按钮打空靶比这难查得多） */
  if (!row) {
    return (
      <Dialog open onOpenChange={next => { if (!next) onClose() }}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t('模型能力')}</DialogTitle>
          </DialogHeader>
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

  // 收窄后的行标识取成 const：`save` 是函数声明（会被提升），TS 不在闭包里
  // 保留 `if (!row)` 的收窄 —— 用 row.id 会按 `possibly null` 报错
  const rowId = row.id
  const providerLabel = row.providerLabel || providerLabelOf(context.provider)
  const inheritLabel = custom ? t('未声明') : t('继承上游')
  const clearLabel = custom ? t('清除') : t('恢复继承')

  /** 数值字段的 placeholder：这一项没有值/未覆盖时提示上游值或「未声明」 */
  function tokenPlaceholder(key: CapabilityKey): string {
    const current = capabilities[key]
    if (isSet(key)) return ''
    if (custom) return t('未填写')
    return typeof current === 'number' ? t('继承上游 {value}', { value: exactTokens(current) }) : t('上游未声明')
  }

  /** 这一项当前有没有值（判据见 setKeysOf） */
  function isSet(key: CapabilityKey): boolean {
    return setKeys.includes(key)
  }

  function setValue(key: CapabilityKey, value: string): void {
    setDraft(prev => ({ ...prev, [key]: value }))
  }

  async function save(): Promise<void> {
    if (saving) return
    // 数值校验：留空 = 继承（合法）；填了就必须是 1 ~ 1 亿的整数
    for (const key of TOKEN_KEYS) {
      const raw = draft[key].trim()
      if (!raw) continue
      if (normalizeCapability(key, raw) === null) {
        setStatus(t('{label}要填 1 ~ 1 亿之间的整数（留空 = {inherit}）',
          { label: CAPABILITY_LABELS[key], inherit: inheritLabel }))
        return
      }
    }
    setSaving(true)
    setStatus(t('保存中…'))
    try {
      accept(await writeCapabilities(context.provider, context.id, patchFromDraft(draft)))
      onClose()
      toast(t('✅ 已保存 {id} 的能力位（{provider}）', { id: rowId, provider: providerLabel }))
    } catch (error) {
      setStatus(t('保存失败：{message}', { message: errorMessage(error) }))
      setSaving(false)
    }
  }

  return (
    <Dialog open onOpenChange={(next, eventDetails) => {
      if (next) return
      // 保存中不许关：关掉会让「到底存没存进去」变成未知状态
      if (saving) { eventDetails.cancel(); return }
      onClose()
    }}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{t('模型能力')}</DialogTitle>
        </DialogHeader>
        <DialogBody>
          {/* 预览行：改的是哪一条模型（与映射弹窗的预览区同一用意） */}
          <div className='rounded-md border border-border bg-surface-inset px-3 py-2.5 font-mono text-[12px] text-subtle'>
            <b className='text-primary-fg'>{row.id}</b>{t('（{provider}）', { provider: providerLabel })}
          </div>

          {TOKEN_KEYS.map(key => (
            <div key={key} className='flex flex-col gap-1.5'>
              <div className='flex items-center gap-[7px]'>
                <Label htmlFor={`cap-${key}`}>{CAPABILITY_LABELS[key]}</Label>
                {isSet(key) ? (
                  <Button variant='ghost' size='2xs' disabled={saving}
                    title={custom ? t('清除这一项（回到未声明）') : t('清除覆盖，回到上游声明值')}
                    onClick={() => setValue(key, '')}>{clearLabel}</Button>
                ) : null}
              </div>
              <Input id={`cap-${key}`} inputMode='numeric' autoComplete='off' spellCheck={false}
                placeholder={tokenPlaceholder(key)} disabled={saving} value={draft[key]}
                onChange={event => setValue(key, event.currentTarget.value)}
                onKeyDown={event => { if (event.key === 'Enter') { event.preventDefault(); void save() } }} />
              <p className='text-xs leading-[1.6] text-subtle'>
                {statusText(key, capabilities[key] ?? null, isSet(key), custom)}
              </p>
            </div>
          ))}

          {BOOLEAN_KEYS.map(key => (
            <div key={key} className='flex flex-col gap-1.5'>
              <Label>{CAPABILITY_LABELS[key]}</Label>
              <SegmentedControl options={BOOLEAN_OPTIONS(inheritLabel)} value={draft[key]}
                aria-label={CAPABILITY_LABELS[key]} className='self-start'
                onValueChange={next => setValue(key, String(next))} />
              <p className='text-xs leading-[1.6] text-subtle'>
                {statusText(key, capabilities[key] ?? null, isSet(key), custom)}
              </p>
            </div>
          ))}

          {levelsInfo ? (
            <div className='rounded-md border border-border bg-surface-inset px-3 py-2.5 text-xs leading-[1.7] text-subtle'>
              <div className='flex flex-wrap items-center gap-x-2 gap-y-1'>
                <span>{t('可配思考档位')}</span>
                <span className='font-mono text-[12px] text-primary-fg'>
                  {levelsInfo.levels.join(' / ')}
                </span>
                {levelsInfo.defaultLevel ? (
                  <span>{t('（默认 ')}<span className='font-mono text-[12px] text-primary-fg'>{levelsInfo.defaultLevel}</span>{t('）')}</span>
                ) : null}
              </div>
              <div className='mt-1'>
                {t('这是模型自己能配的档位（来自上游目录），只读；要改某一档请到映射弹窗里选。')}
              </div>
            </div>
          ) : null}

          <p className='text-xs leading-[1.65] text-subtle'>
            {t('这几项是给')}<b>{t('下游客户端')}</b>{t('看的能力声明（')}
            <code>/v1/models</code>{t(' 里的 ')}<code>max_input_tokens</code> / <code>supports_*</code>{t(' 与 ')}<code>input_modalities</code>
            {t('），下游按它们决定发不发图片、按多大的窗口堆历史。')}
            <b>{t('不影响网关的转发与路由')}</b>{t('。上游清单给的值不准时，在这里改；')}
            {custom ? t('这家的清单是你自己登记的，不填的项下游按未声明处理。') : t('「恢复继承」会把这一项交还给上游清单的值。')}
          </p>
          {/* 状态行：高度固定，出现错误时弹窗不跳高 */}
          <div className='min-h-[18px] text-xs text-subtle'>{status}</div>
        </DialogBody>
        <DialogFooter>
          <div className='mr-auto' />
          <Button variant='outline' disabled={saving} onClick={onClose}>{t('取消')}</Button>
          <Button variant='default' disabled={saving} onClick={() => void save()}>{t('保存')}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
