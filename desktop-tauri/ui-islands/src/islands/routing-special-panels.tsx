/**
 * 路由页「特殊请求路由」的三块配置面板。
 *
 * ── 版面口径（2026-11 改版）──────────────────────────────────
 * 三块配置的真实形状都是「一份规则列表」：压缩上下文是「哪个客户端发来的压缩请求
 * 改成哪只模型」，图片兼容是「哪些模型看不了图、交给谁」，标题生成是「认哪些客户端、
 * 用哪只模型」。原来把它们画成「触发 → 改写 → 目标」三个方框 + 一堆徽章，一屏里
 * 十来个描边矩形，读者得自己拼出「谁触发、改成什么」。
 *
 * 现在改成系统设置那种分组列表（与设置页的客户端识别面板同一口径）：
 *   一句话说清这一类请求怎么被改写 → 一份规则列表（每行一句话）→
 *   点列表里的一行改这条规则，「添加规则」开弹窗填字段。
 *
 * 三块共用同一套零件（RulePanel / RuleSection / RuleRow / 弹窗字段），
 * 差别只在表单字段 —— 状态机也一样（读取 / 改草稿 / 保存 / 撤销），
 * 由 routing-special-state 的 useSpecialEditor 收口。
 *
 * ── 迁移进度 ──────────────────────────────────────────────────
 * 已迁到新版式：压缩上下文。
 * 仍用旧版式（SpecialShell / SpecialFlow，下面那一段）：标题生成 / 图片兼容 ——
 * 它们要连同后端契约一起改（标题生成从「一套模型管所有客户端」改成按规则，
 * 图片兼容从「两块固定设置」改成按规则），改完一起删掉旧壳子。
 */

import * as React from 'react'
import {
  Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle,
  Label, MultiSelect, Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
  Spinner, Switch,
} from '@ui'
import { t } from '../i18n'
import { RouteIcon } from './routing-ui'
import { SearchableSelect } from '../../../ui-shared/searchable-select'
import {
  modelKey, type CompactConfig, type CompactRoute, type CompactView, type SpecialEditor, type TitleConfig, type TitleRule,
  type TitleView, type VisionConfig, type VisionModelRef, type VisionRuleItem, type VisionTarget, type VisionView,
} from './routing-special-state'
import '../styles/routing-special.css'

type ModelChoice = { id: string; name?: string; providerLabel?: string; provider?: string; kind?: string }

/** 模型下拉显示 ID，名称与提供商仅用于搜索。 */
function ModelSelect({ value, models, exclude, onChange, disabled, ariaLabel }: {
  value: string; models: ModelChoice[]; exclude?: string[]; onChange(next: string): void
  disabled?: boolean; ariaLabel: string
}) {
  // 同一个模型 ID 可能来自多家（别名映射），按 key 去重后再列出。
  const ids = [...new Map(models.map(model => [modelKey(model.id), model.id])).values()]
    .filter(id => !exclude?.includes(modelKey(id)) || modelKey(id) === modelKey(value))
  const selectedId = ids.find(id => modelKey(id) === modelKey(value)) || value
  return <SearchableSelect value={selectedId} disabled={disabled} onValueChange={onChange}
    className='w-full' aria-label={ariaLabel} searchPlaceholder={t('搜索模型')}
    displayValue={selectedId || t('请选择模型')}
    options={ids.map(id => ({ value: id, label: id,
      searchText: models.filter(model => modelKey(model.id) === modelKey(id))
        .map(model => `${model.name || ''} ${model.providerLabel || ''} ${model.provider || ''}`).join(' '),
    }))} />
}

/* ═══════════════ 新版式：整页 = 一句话 + 一份规则列表 ═══════════════ */

/**
 * 面板外壳：名字 + 一句话规则 + 总开关 + 正文 + 底部保存条。
 *
 * 保存条留在面板底部而不是提到页头工具条：三块配置各有一份修订号、各自独立保存，
 * 页头那一格同时只能属于一块，「保存」按钮离它管的那块内容太远反而容易点错。
 */
function RulePanel({ title, lede, enabled, onEnabled, busy, dirty, error, loading, onReload, onSave, onDiscard, children }: {
  title: string
  lede: string
  enabled: boolean
  onEnabled(next: boolean): void
  busy: boolean
  dirty: boolean
  error: string
  loading: boolean
  onReload(): void
  onSave(): void
  onDiscard(): void
  children: React.ReactNode
}) {
  return <section className='rp-panel'>
    <header className='rp-head'>
      <div className='rp-head-text'>
        <h2>{title}</h2>
        <p className='rp-lede'>{lede}</p>
      </div>
      <label className='rp-enable'>
        <span>{enabled ? t('已启用') : t('已关闭')}</span>
        <Switch checked={enabled} disabled={busy} aria-label={enabled ? t('关闭') : t('启用')}
          onCheckedChange={onEnabled} />
      </label>
    </header>
    <p className='rp-note'>{t('规则全局生效，不受原路由池限制；指定模型按自己的路由选账号，网关 Key 权限仍然生效。')}</p>
    {loading ? <div className='rp-loading'><Spinner />{t('读取中…')}</div> : children}
    {error ? <p className='routing-config-error' role='alert'>{error}</p> : null}
    <footer className='rp-foot'>
      <span className='rp-foot-state'>{dirty ? t('有未保存的修改') : t('已保存')}</span>
      <div className='rp-foot-actions'>
        <Button variant='ghost' size='sm' disabled={busy} onClick={onReload}>{t('重新读取')}</Button>
        <Button variant='ghost' size='sm' disabled={!dirty || busy} onClick={onDiscard}>{t('撤销修改')}</Button>
        <Button size='sm' disabled={!dirty || busy} onClick={onSave}>{busy ? t('保存中…') : t('保存')}</Button>
      </div>
    </footer>
  </section>
}

/** 一个分组：13px 半粗标题（带计数与行尾操作）+ 一张无描边白底圆角卡 */
function RuleSection({ title, count, action, children }: {
  title: string; count?: string; action?: React.ReactNode; children: React.ReactNode
}) {
  return <section className='rp-section'>
    <div className='rp-section-head'>
      <h3>{title}</h3>
      {count ? <span className='rp-section-count'>{count}</span> : null}
      {action ? <span className='rp-section-action'>{action}</span> : null}
    </div>
    <div className='rp-rows'>{children}</div>
  </section>
}

/** 一行规则：整行可点（真按钮，键盘能 Tab 到），右侧一枚雪佛龙暗示「点进去改」 */
function RuleRow({ title, detail, side, off, onClick }: {
  title: React.ReactNode
  detail: React.ReactNode
  side?: React.ReactNode
  off?: boolean
  onClick(): void
}) {
  return <button type='button' className='rp-row' data-off={off ? '' : undefined} onClick={onClick}>
    <span className='rp-row-main'>
      <span className='rp-row-title'>{title}</span>
      <span className='rp-row-detail'>{detail}</span>
    </span>
    <span className='rp-row-side'>
      {side}
      <span className='rp-caret'><RouteIcon name='chevronRight' size={13} /></span>
    </span>
  </button>
}

/** 弹窗表单的一格：标签在上、控件在下（与设置页的规则编辑弹窗同一口径） */
function DlgField({ label, children }: { label: string; children: React.ReactNode }) {
  return <div className='rp-field'><Label>{label}</Label>{children}</div>
}

/** 弹窗顶部那条「启用规则」：开关在左、状态右对齐 */
function RuleSwitchRow({ enabled, disabled, onChange }: {
  enabled: boolean; disabled?: boolean; onChange(next: boolean): void
}) {
  return <div className='rp-switch-row'>
    <Switch checked={enabled} disabled={disabled} aria-label={t('启用规则')} onCheckedChange={onChange} />
    <span>{t('启用规则')}</span>
    <small>{enabled ? t('已启用') : t('已停用')}</small>
  </div>
}

/** 弹窗底栏：左边一枚删除（只编辑已有规则时出现），右边取消 + 保存 */
function RuleDialogFooter({ onClose, onSave, onDelete, disabled }: {
  onClose(): void; onSave(): void; onDelete?(): void; disabled?: boolean
}) {
  return <DialogFooter>
    {onDelete ? <Button variant='destructive' className='mr-auto' disabled={disabled}
      onClick={() => { onDelete(); onClose() }}>{t('删除规则')}</Button> : null}
    <Button variant='ghost' disabled={disabled} onClick={onClose}>{t('取消')}</Button>
    <Button disabled={disabled} onClick={onSave}>{t('保存规则')}</Button>
  </DialogFooter>
}

/* ─────────────────── 压缩上下文 ─────────────────── */

function CompactRuleDialog({ rule, models, clients, saving, onClose, onSave, onDelete }: {
  rule: CompactRoute
  models: ModelChoice[]
  clients: CompactView['clients']
  saving: boolean
  onClose(): void
  onSave(rule: CompactRoute): void
  onDelete?(): void
}) {
  const [draft, setDraft] = React.useState(rule)
  const update = (patch: Partial<CompactRoute>) => setDraft(current => ({ ...current, ...patch }))
  const clientLabel = clients.find(client => client.id === draft.client)?.label || draft.client
  return <Dialog open onOpenChange={next => { if (!next) onClose() }}>
    <DialogContent className='w-[min(520px,calc(100vw-24px))]'>
      <DialogHeader><DialogTitle>{onDelete ? t('编辑规则') : t('添加规则')}</DialogTitle></DialogHeader>
      <DialogBody>
        <p className='text-[12px] leading-[1.7] text-muted-foreground'>
          {t('命中这条规则的压缩请求会改用「压缩模型」重发一次；改写之后按新模型自己的路由挑账号。')}
        </p>
        <RuleSwitchRow enabled={draft.enabled} disabled={saving} onChange={enabled => update({ enabled })} />
        <DlgField label={t('触发客户端')}>
          <Select value={draft.client} disabled={saving}
            onValueChange={next => update({ client: String(next) })}>
            <SelectTrigger aria-label={t('触发客户端')}><SelectValue>{clientLabel}</SelectValue></SelectTrigger>
            <SelectContent>{clients.map(client =>
              <SelectItem key={client.id} value={client.id}>{client.label}</SelectItem>)}</SelectContent>
          </Select>
        </DlgField>
        <DlgField label={t('原模型')}>
          <ModelSelect value={draft.source} models={models} exclude={[modelKey(draft.target)]} disabled={saving}
            ariaLabel={t('原模型')} onChange={source => update({ source })} />
        </DlgField>
        <DlgField label={t('压缩模型')}>
          <ModelSelect value={draft.target} models={models} exclude={[modelKey(draft.source)]} disabled={saving}
            ariaLabel={t('压缩模型')} onChange={target => update({ target })} />
        </DlgField>
      </DialogBody>
      <RuleDialogFooter disabled={saving} onClose={onClose} onSave={() => onSave(draft)} onDelete={onDelete} />
    </DialogContent>
  </Dialog>
}

export function CompactSpecialPanel({ editor }: { editor: SpecialEditor<CompactConfig, CompactView> }) {
  const { view, draft, loading, saving, dirty, error } = editor
  const models = view?.models || []
  const clients = view?.clients || []
  const maxRules = view?.maxRules || 0
  const rules = draft?.rules || []
  /** 正在编辑的那条规则：index = -1 表示新增（保存时追加到末尾） */
  const [editing, setEditing] = React.useState<{ rule: CompactRoute; index: number } | null>(null)
  const clientLabel = (id: string) => clients.find(client => client.id === id)?.label || id
  const full = maxRules > 0 && rules.length >= maxRules
  const write = (index: number, rule: CompactRoute) => editor.update(current => ({
    ...current,
    rules: index < 0 ? [...current.rules, rule] : current.rules.map((item, at) => at === index ? rule : item),
  }))
  const blank: CompactRoute = { client: clients[0]?.id || '', source: '', target: '', enabled: true }
  return <RulePanel title={t('压缩上下文')}
    lede={t('命中下面规则的上下文压缩请求，会改用指定的模型压缩一次；账号仍按新模型自己的路由挑。')}
    enabled={Boolean(draft?.enabled)} onEnabled={next => editor.update(current => ({ ...current, enabled: next }))}
    busy={saving || loading} dirty={dirty} error={error} loading={loading}
    onReload={() => void editor.reload()} onSave={() => void editor.save()} onDiscard={editor.discard}>
    {draft ? <>
      <RuleSection title={t('规则')} count={t('{n} 条', { n: rules.length })}
        action={<Button variant='outline' size='sm' disabled={saving || full || !models.length}
          onClick={() => setEditing({ rule: blank, index: -1 })}>
          <RouteIcon name='plus' size={13} />{t('添加规则')}
        </Button>}>
        {rules.length ? rules.map((rule, index) => <RuleRow key={index} off={!rule.enabled}
          title={t('{client} 的压缩请求', { client: clientLabel(rule.client) })}
          detail={<>
            <span>{rule.source || t('还没选原模型')}</span>
            <span aria-hidden='true'>→</span>
            <span className='rp-row-target'>{rule.target || t('还没选压缩模型')}</span>
          </>}
          side={rule.enabled ? null : t('已停用')}
          onClick={() => setEditing({ rule, index })} />)
          : <p className='rp-empty'>{t('还没有规则。压缩请求会按原模型照常转发。')}</p>}
      </RuleSection>
      {!models.length ? <p className='rp-note'>{t('没有可选的对话模型，请先启用模型和对应账号。')}</p> : null}
      {full ? <p className='rp-note'>{t('规则最多 {n} 条。', { n: maxRules })}</p> : null}
    </> : null}
    {editing ? <CompactRuleDialog key={editing.index} rule={editing.rule} models={models} clients={clients} saving={saving}
      onClose={() => setEditing(null)}
      onSave={rule => { write(editing.index, rule); setEditing(null) }}
      onDelete={editing.index < 0 ? undefined : () => editor.update(current => ({
        ...current, rules: current.rules.filter((_, at) => at !== editing.index),
      }))} /> : null}
  </RulePanel>
}

/* ─────────────────── 图片兼容 ─────────────────── */

const visionKey = (target: { provider: string; model: string }) => JSON.stringify([target.provider, target.model])

const parseVisionKey = (key: string): VisionModelRef | null => {
  try {
    const parsed: unknown = JSON.parse(key)
    return Array.isArray(parsed) && parsed.length === 2 && parsed.every(item => typeof item === 'string')
      ? { provider: String(parsed[0]), model: String(parsed[1]) } : null
  } catch { return null }
}

/** 图片兼容按提供商绑定模型，显示 ID 时仍需保留提供商以区分同名模型。 */
function visionLabel(models: VisionView['models'], ref: { provider: string; model: string } | null): string {
  if (!ref) return ''
  const found = models.find(model => model.provider === ref.provider && modelKey(model.model) === modelKey(ref.model))
  return `${found?.providerLabel || ref.provider} · ${found?.model || ref.model}`
}

function VisionRuleDialog({ rule, view, saving, onClose, onSave, onDelete }: {
  rule: VisionRuleItem
  view: VisionView
  saving: boolean
  onClose(): void
  onSave(rule: VisionRuleItem): void
  onDelete?(): void
}) {
  const [draft, setDraft] = React.useState(rule)
  const [candidate, setCandidate] = React.useState('')
  const update = (patch: Partial<VisionRuleItem>) => setDraft(current => ({ ...current, ...patch }))
  const listChoices = view.models.filter(model => model.capabilities?.supportsImages !== true)
    .map(model => ({ value: visionKey(model), label: `${model.providerLabel || model.provider} · ${model.model}` }))
  const targetChoices = view.models.filter(model => model.capabilities?.supportsImages === true)
    .map(model => ({ value: visionKey(model), label: `${model.providerLabel || model.provider} · ${model.model}`,
      searchText: `${model.name || ''} ${model.provider}` }))
  const [primary, ...backups] = draft.targets
  const setTargets = (targets: VisionTarget[]) =>
    update({ targets, fallback: targets.length ? 'designated' : 'stop' })
  const pick = (value: string): VisionTarget | null => {
    const ref = parseVisionKey(value)
    return ref ? { ...ref, reasoning: '' } : null
  }
  const move = (index: number, direction: number) => {
    const next = [...draft.targets]
    const to = index + direction
    if (to < 0 || to >= next.length) return
    ;[next[index], next[to]] = [next[to], next[index]]
    setTargets(next)
  }
  return <Dialog open onOpenChange={next => { if (!next) onClose() }}>
    <DialogContent className='w-[min(560px,calc(100vw-24px))]'>
      <DialogHeader><DialogTitle>{onDelete ? t('编辑规则') : t('添加规则')}</DialogTitle></DialogHeader>
      <DialogBody>
        <p className='text-[12px] leading-[1.7] text-muted-foreground'>
          {t('这条规则里的模型收到图片时按选定的方式处理；没被任何规则选中的模型一律原样放行。')}
        </p>
        <RuleSwitchRow enabled={draft.enabled} disabled={saving} onChange={enabled => update({ enabled })} />
        <DlgField label={t('哪些模型用这种方式')}>
          <MultiSelect className='w-full' value={draft.models.map(visionKey)} disabled={saving}
            options={listChoices} placeholder={t('选择一个或多个模型')} searchPlaceholder={t('搜索提供商或模型')}
            aria-label={t('哪些模型用这种方式')}
            onValueChange={keys => update({ models: keys.map(parseVisionKey).filter((ref): ref is VisionModelRef => ref !== null) })} />
        </DlgField>
        <DlgField label={t('处理方式')}>
          <Select value={draft.mode} disabled={saving}
            onValueChange={next => update({ mode: String(next) as VisionRuleItem['mode'] })}>
            <SelectTrigger className='w-full' aria-label={t('处理方式')}><SelectValue>
              {draft.mode === 'assist' ? t('辅助识图') : t('整体接管')}
            </SelectValue></SelectTrigger>
            <SelectContent>
              <SelectItem value='assist'>{t('辅助识图')}</SelectItem>
              <SelectItem value='reroute'>{t('整体接管')}</SelectItem>
            </SelectContent>
          </Select>
        </DlgField>
        {draft.mode === 'assist' ? <>
          <DlgField label={t('用哪只模型识图')}>
            <SearchableSelect className='w-full' value={draft.source === 'shared' ? 'shared' : (draft.target ? visionKey(draft.target) : '')}
              disabled={saving} aria-label={t('用哪只模型识图')} searchPlaceholder={t('搜索模型')}
              displayValue={draft.source === 'shared' ? t('沿用工具服务的视觉模型') : (visionLabel(view.models, draft.target) || t('请选择模型'))}
              options={[{ value: 'shared', label: t('沿用工具服务的视觉模型') }, ...targetChoices]}
              onValueChange={next => {
                if (next === 'shared') { update({ source: 'shared' }); return }
                const target = pick(next)
                if (target) update({ source: 'custom', target: { ...target, reasoning: draft.target?.reasoning || '' } })
              }} />
          </DlgField>
          <DlgField label={t('复用相同图片的识读结果')}>
            <div className='flex items-center gap-3'>
              <Switch checked={draft.reuseResults} disabled={saving} aria-label={t('复用相同图片的识读结果')}
                onCheckedChange={next => update({ reuseResults: next === true })} />
              <span className='text-[12px] text-muted-foreground'>{t('同一会话、同一问题、相同权限范围内复用；追问新细节会重新识读。')}</span>
            </div>
          </DlgField>
        </> : <>
          <DlgField label={t('优先使用当前路由内的视觉模型')}>
            <div className='flex items-center gap-3'>
              <Switch checked={draft.preferCurrentRoute} disabled={saving} aria-label={t('优先使用当前路由内的视觉模型')}
                onCheckedChange={next => update({ preferCurrentRoute: next === true })} />
              <span className='text-[12px] text-muted-foreground'>{t('先尝试当前路由内的视觉模型，不可用时再按顺序尝试指定模型；指定模型可跨路由池。')}</span>
            </div>
          </DlgField>
          <DlgField label={t('接管回答的模型')}>
            <SearchableSelect className='w-full' value={primary ? visionKey(primary) : ''} disabled={saving}
              aria-label={t('接管回答的模型')} searchPlaceholder={t('搜索模型')}
              displayValue={primary ? visionLabel(view.models, primary) : t('请选择模型')}
              options={targetChoices}
              onValueChange={next => {
                const target = pick(next)
                if (target) setTargets(target ? [{ ...target, reasoning: primary?.reasoning || '' }, ...backups] : backups)
              }} />
          </DlgField>
          <DlgField label={t('备选模型')}>
            <div className='flex flex-col gap-2'>
              <div className='flex items-center gap-2'>
                <SearchableSelect className='min-w-0 flex-1' value={candidate} disabled={saving || draft.targets.length >= view.maxTargets}
                  aria-label={t('备选模型')} searchPlaceholder={t('搜索模型')}
                  displayValue={visionLabel(view.models, parseVisionKey(candidate)) || t('请选择模型')}
                  options={targetChoices} onValueChange={setCandidate} />
                <Button variant='outline' size='sm' disabled={saving || !candidate || draft.targets.length >= view.maxTargets}
                  onClick={() => { const target = pick(candidate); if (target) { setTargets([...draft.targets, target]); setCandidate('') } }}>
                  {t('加入备选')}
                </Button>
              </div>
              {backups.map((backup, index) => <div key={visionKey(backup)} className='flex items-center gap-2 text-[12px]'>
                <span className='min-w-0 flex-1 truncate'>{visionLabel(view.models, backup)}</span>
                <Button variant='ghost' size='xs' disabled={saving || index === 0} onClick={() => move(index + 1, -1)}>{t('上移')}</Button>
                <Button variant='ghost' size='xs' disabled={saving || index === backups.length - 1} onClick={() => move(index + 1, 1)}>{t('下移')}</Button>
                <Button variant='ghost' size='xs' disabled={saving}
                  onClick={() => setTargets(draft.targets.filter((_, at) => at !== index + 1))}>{t('移除')}</Button>
              </div>)}
              <span className='text-[11.5px] leading-[1.6] text-muted-foreground'>
                {t('备选按顺序尝试；一个都不加时，当前路由内找不到视觉模型就停止并说明原因。')}
              </span>
            </div>
          </DlgField>
        </>}
      </DialogBody>
      <RuleDialogFooter disabled={saving} onClose={onClose} onSave={() => onSave(draft)} onDelete={onDelete} />
    </DialogContent>
  </Dialog>
}

export function VisionSpecialPanel({ editor }: { editor: SpecialEditor<VisionConfig, VisionView> }) {
  const { view, draft, loading, saving, dirty, error } = editor
  const rules = draft?.rules || []
  const [editing, setEditing] = React.useState<{ rule: VisionRuleItem; index: number } | null>(null)
  const blank: VisionRuleItem = {
    enabled: true, models: [], mode: 'assist', source: 'shared', target: null,
    reuseResults: true, preferCurrentRoute: true, fallback: 'stop', targets: [],
  }
  const write = (index: number, rule: VisionRuleItem) => editor.update(current => ({
    ...current,
    rules: index < 0 ? [...current.rules, rule] : current.rules.map((item, at) => at === index ? rule : item),
  }))
  const detail = (rule: VisionRuleItem) => {
    const label = visionLabel(view?.models || [], rule.targets[0] || rule.target)
    if (rule.mode === 'assist') {
      const reader = rule.source === 'shared' ? t('工具服务的视觉模型') : (label || t('还没选视觉模型'))
      return <><span>{t('先读图再交回')}</span><span className='rp-row-target'>{reader}</span><span>· {t('原模型继续回答')}</span></>
    }
    const taker = rule.preferCurrentRoute ? t('当前路由内的视觉模型') : (label || t('还没选视觉模型'))
    return <><span>{t('整次请求改交')}</span><span className='rp-row-target'>{taker}</span></>
  }
  return <RulePanel title={t('图片兼容')}
    lede={t('模型看不了图片时，按下面的规则交给视觉模型；没配到的模型、以及本来就能看图的，一律原样放行。')}
    enabled={Boolean(draft?.enabled)} onEnabled={next => editor.update(current => ({ ...current, enabled: next }))}
    busy={saving || loading} dirty={dirty} error={error} loading={loading}
    onReload={() => void editor.reload()} onSave={() => void editor.save()} onDiscard={editor.discard}>
    {draft && view ? <>
      <RuleSection title={t('规则')} count={t('{n} 条', { n: rules.length })}
        action={<Button variant='outline' size='sm' disabled={saving}
          onClick={() => setEditing({ rule: blank, index: -1 })}>
          <RouteIcon name='plus' size={13} />{t('添加规则')}
        </Button>}>
        {rules.length ? rules.map((rule, index) => <RuleRow key={index} off={!rule.enabled}
          title={rule.models.length
            ? t('{models} 看不了图时', { models: rule.models.map(model => visionLabel(view.models, model)).join('、') })
            : t('未选择模型')}
          detail={detail(rule)}
          side={rule.enabled ? null : t('已停用')}
          onClick={() => setEditing({ rule, index })} />)
          : <p className='rp-empty'>{t('还没有规则。图片会照常交给原模型，看不了的就会报错。')}</p>}
      </RuleSection>
      {!view.models.length ? <p className='rp-note'>{t('还没有可用的模型，请先启用模型和对应账号。')}</p> : null}
    </> : null}
    {editing && view ? <VisionRuleDialog key={editing.index} rule={editing.rule} view={view} saving={saving}
      onClose={() => setEditing(null)}
      onSave={rule => { write(editing.index, rule); setEditing(null) }}
      onDelete={editing.index < 0 ? undefined : () => editor.update(current => ({
        ...current, rules: current.rules.filter((_, at) => at !== editing.index),
      }))} /> : null}
  </RulePanel>
}

/* ─────────────────── 标题生成 ─────────────────── */

function TitleRuleDialog({ rule, models, clients, saving, onClose, onSave, onDelete }: {
  rule: TitleRule
  models: ModelChoice[]
  clients: TitleView['clients']
  saving: boolean
  onClose(): void
  onSave(rule: TitleRule): void
  onDelete?(): void
}) {
  const [draft, setDraft] = React.useState(rule)
  const update = (patch: Partial<TitleRule>) => setDraft(current => ({ ...current, ...patch }))
  return <Dialog open onOpenChange={next => { if (!next) onClose() }}>
    <DialogContent className='w-[min(520px,calc(100vw-24px))]'>
      <DialogHeader><DialogTitle>{onDelete ? t('编辑规则') : t('添加规则')}</DialogTitle></DialogHeader>
      <DialogBody>
        <p className='text-[12px] leading-[1.7] text-muted-foreground'>
          {t('这条规则只改写被识别为标题生成的小请求，普通对话不受影响。')}
        </p>
        <RuleSwitchRow enabled={draft.enabled} disabled={saving} onChange={enabled => update({ enabled })} />
        <DlgField label={t('识别客户端')}>
          <MultiSelect className='w-full' value={draft.clients} disabled={saving}
            options={clients.map(client => ({ value: client.id, label: client.label, disabled: !client.supported }))}
            placeholder={t('选择一个或多个客户端')} searchPlaceholder={t('搜索客户端')}
            aria-label={t('识别客户端')} onValueChange={next => update({ clients: next })} />
        </DlgField>
        <DlgField label={t('标题模型')}>
          <ModelSelect value={draft.model} models={models} disabled={saving}
            ariaLabel={t('标题模型')} onChange={model => update({ model })} />
        </DlgField>
        <DlgField label={t('标题模型失败时')}>
          <Select value={draft.fallback} disabled={saving}
            onValueChange={next => update({ fallback: next as TitleRule['fallback'] })}>
            <SelectTrigger className='w-full' aria-label={t('标题模型失败时')}><SelectValue>
              {draft.fallback === 'original' ? t('退回原模型') : t('直接报错')}
            </SelectValue></SelectTrigger>
            <SelectContent>
              <SelectItem value='original'>{t('退回原模型')}</SelectItem>
              <SelectItem value='error'>{t('直接报错')}</SelectItem>
            </SelectContent>
          </Select>
        </DlgField>
      </DialogBody>
      <RuleDialogFooter disabled={saving} onClose={onClose} onSave={() => onSave(draft)} onDelete={onDelete} />
    </DialogContent>
  </Dialog>
}

export function TitleSpecialPanel({ editor }: { editor: SpecialEditor<TitleConfig, TitleView> }) {
  const { view, draft, loading, saving, dirty, error } = editor
  const models = view?.models || []
  const clients = view?.clients || []
  const rules = draft?.rules || []
  const [editing, setEditing] = React.useState<{ rule: TitleRule; index: number } | null>(null)
  const clientLabel = (id: string) => clients.find(client => client.id === id)?.label || id
  const write = (index: number, rule: TitleRule) => editor.update(current => ({
    ...current,
    rules: index < 0 ? [...current.rules, rule] : current.rules.map((item, at) => at === index ? rule : item),
  }))
  const blank: TitleRule = { enabled: true, clients: [], model: '', fallback: 'original' }
  return <RulePanel title={t('标题生成')}
    lede={t('只改写被识别为标题生成的小请求，普通对话不受影响。')}
    enabled={Boolean(draft?.enabled)} onEnabled={next => editor.update(current => ({ ...current, enabled: next }))}
    busy={saving || loading} dirty={dirty} error={error} loading={loading}
    onReload={() => void editor.reload()} onSave={() => void editor.save()} onDiscard={editor.discard}>
    {draft ? <>
      <RuleSection title={t('规则')} count={t('{n} 条', { n: rules.length })}
        action={<Button variant='outline' size='sm' disabled={saving || !models.length}
          onClick={() => setEditing({ rule: blank, index: -1 })}>
          <RouteIcon name='plus' size={13} />{t('添加规则')}
        </Button>}>
        {rules.length ? rules.map((rule, index) => <RuleRow key={index} off={!rule.enabled}
          title={rule.clients.length
            ? t('{clients} 的标题请求', { clients: rule.clients.map(clientLabel).join('、') })
            : t('未选择客户端')}
          detail={<>
            <span>{t('改用')}</span>
            <span className='rp-row-target'>{rule.model || t('还没选标题模型')}</span>
            <span>·</span>
            <span>{rule.fallback === 'original' ? t('失败时退回原模型') : t('失败时直接报错')}</span>
          </>}
          side={rule.enabled ? null : t('已停用')}
          onClick={() => setEditing({ rule, index })} />)
          : <p className='rp-empty'>{t('还没有规则。标题请求会照常交给原模型。')}</p>}
      </RuleSection>
      {!models.length ? <p className='rp-note'>{t('没有可选的对话模型，请先启用模型和对应账号。')}</p> : null}
    </> : null}
    {editing ? <TitleRuleDialog key={editing.index} rule={editing.rule} models={models} clients={clients} saving={saving}
      onClose={() => setEditing(null)}
      onSave={rule => { write(editing.index, rule); setEditing(null) }}
      onDelete={editing.index < 0 ? undefined : () => editor.update(current => ({
        ...current, rules: current.rules.filter((_, at) => at !== editing.index),
      }))} /> : null}
  </RulePanel>
}
