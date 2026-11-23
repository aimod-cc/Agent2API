import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  actionLabels,
  cooldownSummary,
  errorPolicyValidation,
  fetchRoutingErrors,
  kindLabels,
  mergeProviderRules,
  ruleLabel,
  saveRoutingErrors,
  type ErrorPolicyView,
  type ErrorRule,
  type ErrorSettings,
  type PluginErrorRule,
} from '../core/api'
import { providerDisplayName, syncProviderLabels } from '../core/format'
import { t } from '../core/i18n'
import { store, useStore } from '../core/store'
import {
  BottomSheet,
  CommonCard,
  CommonSwitch,
  ConnectedButtonGroup,
  DashboardCard,
  EmptyBlock,
  SectionHeader,
  SegmentedControl,
  TonalChip,
} from '../ui/components'
import { PickerSheet, PickerTrigger, type PickerOption } from '../ui/pickers'
import { RoutingErrorRuleSheet } from './RoutingErrorRuleSheet'

type ErrorSubTab = 'rules' | 'protection' | 'boundaries'
type RuleView = 'global' | 'provider'

export function RoutingErrorsPanel({ refreshNonce }: { refreshNonce: number }) {
  const { server } = useStore()
  const [view, setView] = useState<ErrorPolicyView | null>(null)
  const [draft, setDraft] = useState<ErrorSettings | null>(null)
  const [subTab, setSubTab] = useState<ErrorSubTab>('rules')
  const [ruleView, setRuleView] = useState<RuleView>('global')
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [errText, setErrText] = useState('')
  const [confirmDefaultsOpen, setConfirmDefaultsOpen] = useState(false)
  const [provFilter, setProvFilter] = useState('__all__')
  const [filterPickerOpen, setFilterPickerOpen] = useState(false)
  const [editor, setEditor] = useState<{
    rule: ErrorRule
    exception: boolean
    pluginDefault?: PluginErrorRule
    isNew?: boolean
  } | null>(null)

  const load = useCallback(async (signal?: AbortSignal) => {
    setLoading(true)
    setErrText('')
    try {
      const res = await fetchRoutingErrors(signal)
      syncProviderLabels(res.providers)
      setView(res)
      setDraft(structuredClone(res.settings))
    } catch (err) {
      if (!signal?.aborted) {
        setErrText(err instanceof Error ? err.message : String(err))
      }
    } finally {
      if (!signal?.aborted) setLoading(false)
    }
  }, [])

  useEffect(() => {
    const ac = new AbortController()
    void load(ac.signal)
    return () => ac.abort()
  }, [load, refreshNonce, server?.base])

  const dirty = Boolean(view && draft && JSON.stringify(view.settings) !== JSON.stringify(draft))

  const patchDraft = (patch: Partial<ErrorSettings>) => {
    setDraft(prev => (prev ? { ...prev, ...patch } : prev))
  }

  const saveAll = async () => {
    if (!view || !draft || saving) return
    const invalid = errorPolicyValidation(draft, t)
    if (invalid) {
      setErrText(invalid)
      store.toast(invalid, 'danger')
      return
    }
    setSaving(true)
    setErrText('')
    try {
      const next = await saveRoutingErrors(draft, view.revision)
      setView(next)
      setDraft(structuredClone(next.settings))
      store.toast(t('错误处理设置已保存，下一个请求生效'), 'success')
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      setErrText(msg)
      store.toast(msg, 'danger')
    } finally {
      setSaving(false)
    }
  }

  const providers = view?.providers || []
  const pluginRules = view?.providerRules || []

  const allProviderEntries = useMemo(
    () => (draft ? mergeProviderRules(draft, pluginRules) : []),
    [draft, pluginRules],
  )

  const exceptionFilterOptions: PickerOption[] = useMemo(() => {
    const ids = new Set<string>()
    for (const item of allProviderEntries) ids.add(item.rule.provider || '*')
    return [
      { value: '__all__', label: t('全部提供商') },
      ...[...ids].map(id => ({
        value: id,
        label: id === '*' ? t('通用规则') : providerDisplayName(id, providers.find(p => p.id === id)?.label),
      })),
    ]
  }, [allProviderEntries, providers])

  const effectiveProvFilter = exceptionFilterOptions.some(o => o.value === provFilter) ? provFilter : '__all__'

  const exceptionItems = useMemo(
    () => allProviderEntries.filter(x => effectiveProvFilter === '__all__' || (x.rule.provider || '*') === effectiveProvFilter),
    [allProviderEntries, effectiveProvFilter],
  )

  if (loading && !draft) {
    return <EmptyBlock icon='clock' title={t('正在加载错误处理配置…')} />
  }

  if (!view || !draft) {
    return (
      <EmptyBlock
        icon='alert'
        title={t('读取错误处理配置失败')}
        description={errText}
        action={
          <button type='button' className='fc-btn is-primary is-sm' onClick={() => void load()}>
            {t('重试')}
          </button>
        }
      />
    )
  }

  const actions = actionLabels(t)
  const kinds = kindLabels(t)

  const applyEditedRule = (nextRule: ErrorRule) => {
    if (!editor) return
    if (!editor.exception) {
      patchDraft({
        rules: draft.rules.map(r => (r.id === nextRule.id ? nextRule : r)),
      })
      return
    }
    const exists = draft.overrides.some(r => r.id === nextRule.id)
    patchDraft({
      overrides: exists ? draft.overrides.map(r => (r.id === nextRule.id ? nextRule : r)) : [...draft.overrides, nextRule],
    })
  }

  const removeEditedRule = () => {
    if (!editor?.exception) return
    patchDraft({
      overrides: draft.overrides.filter(
        r => r.id !== editor.rule.id && (!editor.pluginDefault || r.id !== editor.pluginDefault.pluginRuleId),
      ),
    })
  }

  const addNewException = () => {
    const source = draft.rules.find(r => r.kind === 'unsupported') || draft.rules[0]
    if (!source) return
    const fallbackProv = effectiveProvFilter !== '__all__' ? effectiveProvFilter : providers[0]?.id || '*'
    setEditor({
      exception: true,
      isNew: true,
      rule: {
        ...structuredClone(source),
        id: `rule-${Date.now().toString(36)}`,
        name: t('新提供商规则'),
        provider: fallbackProv,
        statuses: [501],
      },
    })
  }

  return (
    <>
      <CommonCard>
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 10 }}>
          <div>
            <div style={{ fontSize: 14, fontWeight: 700 }}>
              {dirty ? t('有未保存的策略改动') : t('错误处理策略已同步')}
            </div>
            <div style={{ fontSize: 12, color: 'var(--fc-text-secondary)', marginTop: 2 }}>
              {t('修改后点右侧保存即对下一个请求生效')}
            </div>
          </div>
          <div style={{ display: 'inline-flex', gap: 6 }}>
            <button
              type='button'
              className='fc-btn is-sm'
              disabled={saving}
              onClick={() => setConfirmDefaultsOpen(true)}
            >
              {t('恢复默认')}
            </button>
            <button
              type='button'
              className={`fc-btn is-sm ${dirty ? 'is-primary' : ''}`}
              disabled={!dirty || saving}
              onClick={() => void saveAll()}
            >
              {saving ? t('保存中') : t('保存')}
            </button>
          </div>
        </div>
        {errText ? <div style={{ fontSize: 12.5, color: 'var(--fc-danger)', marginTop: 8 }}>{errText}</div> : null}
      </CommonCard>

      <CommonCard>
        <div style={{ fontSize: 13.5, fontWeight: 700, marginBottom: 8 }}>{t('单条请求重试上限')}</div>
        <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 8 }}>
          <NumberField label={t('原账号重发(次)')} value={draft.resends} min={0} max={10} onChange={resends => patchDraft({ resends })} />
          <NumberField label={t('最多再换号(个)')} value={draft.switches} min={0} max={10} onChange={switches => patchDraft({ switches })} />
          <NumberField label={t('重发间隔(秒)')} value={draft.intervalSeconds} min={0} max={300} onChange={intervalSeconds => patchDraft({ intervalSeconds })} />
          <NumberField label={t('累计等待上限(秒)')} value={draft.waitBudgetSeconds} min={0} max={300} onChange={waitBudgetSeconds => patchDraft({ waitBudgetSeconds })} />
        </div>
      </CommonCard>

      <ConnectedButtonGroup<ErrorSubTab>
        value={subTab}
        onChange={setSubTab}
        options={[
          { id: 'rules', label: t('处理规则') },
          { id: 'protection', label: t('失败保护') },
          { id: 'boundaries', label: t('安全边界') },
        ]}
      />

      {subTab === 'rules' ? (
        <>
          <SegmentedControl<RuleView>
            value={ruleView}
            onChange={setRuleView}
            options={[
              { id: 'global', label: t('全局处理规则') },
              {
                id: 'provider',
                label: allProviderEntries.length > 0
                  ? t('提供商处理规则 ({n})', { n: allProviderEntries.length })
                  : t('提供商处理规则'),
              },
            ]}
          />

          {ruleView === 'global' ? (
            <>
              {draft.rules.map(rule => (
                <CommonCard key={rule.id}>
                  <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 10 }}>
                    <div style={{ minWidth: 0, flex: 1 }}>
                      <div style={{ fontSize: 14.5, fontWeight: 700 }}>{ruleLabel(rule, t)}</div>
                      <div style={{ fontSize: 12, color: 'var(--fc-text-secondary)', marginTop: 2 }}>
                        {rule.statuses?.length ? rule.statuses.join(', ') : t('提供商已识别')} · {actions[rule.action]}
                      </div>
                      <div style={{ fontSize: 12, color: 'var(--fc-text-muted)', marginTop: 2 }}>
                        {cooldownSummary(rule, draft, t)}
                      </div>
                    </div>
                    <button
                      type='button'
                      className='fc-btn is-sm is-tonal'
                      onClick={() => setEditor({ rule, exception: false })}
                    >
                      {t('编辑')}
                    </button>
                  </div>
                </CommonCard>
              ))}
            </>
          ) : (
            <>
              <SectionHeader
                title={t('共 {n} 条规则', { n: exceptionItems.length })}
                trailing={
                  <button type='button' className='fc-btn is-sm is-primary' onClick={addNewException}>
                    {t('添加规则')}
                  </button>
                }
              />

              {exceptionFilterOptions.length > 2 ? (
                <PickerTrigger
                  valueLabel={exceptionFilterOptions.find(o => o.value === effectiveProvFilter)?.label || t('全部提供商')}
                  onClick={() => setFilterPickerOpen(true)}
                />
              ) : null}

              {exceptionItems.length === 0 ? (
                <EmptyBlock icon='shield' title={t('暂无提供商处理规则')} description={t('特定提供商、模型或状态码需要单独处理时可在此添加')} />
              ) : (
                exceptionItems.map(item => {
                  const pName = item.rule.provider === '*'
                    ? t('通用规则')
                    : providerDisplayName(item.rule.provider, providers.find(p => p.id === item.rule.provider)?.label)
                  return (
                    <CommonCard key={item.id}>
                      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 10 }}>
                        <div style={{ minWidth: 0, flex: 1 }}>
                          <div style={{ display: 'flex', alignItems: 'center', gap: 6, flexWrap: 'wrap' }}>
                            <span style={{ fontSize: 14.5, fontWeight: 700 }}>{item.rule.name}</span>
                            <TonalChip tone={item.rule.enabled ? 'primary' : 'neutral'}>{pName}</TonalChip>
                            {item.custom ? (
                              <TonalChip tone='neutral'>{t('自定义')}</TonalChip>
                            ) : item.overridden ? (
                              <TonalChip tone='warning'>{item.rule.enabled ? t('已修改') : t('已关闭')}</TonalChip>
                            ) : (
                              <TonalChip tone='neutral'>{t('插件默认')}</TonalChip>
                            )}
                          </div>
                          <div style={{ fontSize: 12, color: 'var(--fc-text-secondary)', marginTop: 3 }}>
                            {item.rule.statuses.join(', ') || kinds[item.rule.kind]}
                            {item.rule.model ? ` · ${item.rule.model}` : ''}
                            {` · ${actions[item.rule.action]}`}
                          </div>
                          <div style={{ fontSize: 12, color: 'var(--fc-text-muted)', marginTop: 2 }}>
                            {cooldownSummary(item.rule, draft, t)}
                          </div>
                        </div>
                        <button
                          type='button'
                          className='fc-btn is-sm is-tonal'
                          onClick={() => setEditor({ rule: item.rule, exception: true, pluginDefault: item.pluginDefault })}
                        >
                          {t('编辑')}
                        </button>
                      </div>
                    </CommonCard>
                  )
                })
              )}
            </>
          )}
        </>
      ) : subTab === 'protection' ? (
        <>
          <DashboardCard icon='clock' title={t('失败退避')} subtitle={t('仅统计连接与服务异常，连续失败逐次延长')}>
            <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
              <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
                <span style={{ fontSize: 14, fontWeight: 600 }}>{t('失败后暂时休息')}</span>
                <CommonSwitch checked={draft.backoff} onChange={backoff => patchDraft({ backoff })} />
              </div>
              <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 8 }}>
                <NumberField
                  label={t('首次休息(秒)')}
                  value={draft.backoffSeconds}
                  min={1}
                  max={3600}
                  disabled={!draft.backoff}
                  onChange={backoffSeconds =>
                    patchDraft({ backoffSeconds, backoffCapSeconds: Math.max(draft.backoffCapSeconds, backoffSeconds) })
                  }
                />
                <NumberField
                  label={t('最长休息(秒)')}
                  value={draft.backoffCapSeconds}
                  min={draft.backoffSeconds}
                  max={3600}
                  disabled={!draft.backoff}
                  onChange={backoffCapSeconds => patchDraft({ backoffCapSeconds })}
                />
              </div>
            </div>
          </DashboardCard>

          <DashboardCard icon='route' title={t('限流后降低优先级')} subtitle={t('成功请求一次可提前恢复')}>
            <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
              <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
                <span style={{ fontSize: 14, fontWeight: 600 }}>{t('限流后暂时排到队尾')}</span>
                <CommonSwitch checked={draft.sink} onChange={sink => patchDraft({ sink })} />
              </div>
              <NumberField
                label={t('最长保留时间(秒)')}
                value={draft.sinkSeconds}
                min={30}
                max={3600}
                disabled={!draft.sink}
                onChange={sinkSeconds => patchDraft({ sinkSeconds })}
              />
            </div>
          </DashboardCard>

          <DashboardCard icon='shield' title={t('连续失败熔断保护')} subtitle={t('到达失败次数后暂时隔离保护')}>
            <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
              <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
                <span style={{ fontSize: 14, fontWeight: 600 }}>{t('启用连续失败保护')}</span>
                <CommonSwitch checked={draft.breaker} onChange={breaker => patchDraft({ breaker })} />
              </div>
              <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr 1fr', gap: 8 }}>
                <NumberField
                  label={t('失败阈值(次)')}
                  value={draft.breakerFailures}
                  min={1}
                  max={20}
                  disabled={!draft.breaker}
                  onChange={breakerFailures => patchDraft({ breakerFailures })}
                />
                <NumberField
                  label={t('保护时长(秒)')}
                  value={draft.breakerSeconds}
                  min={10}
                  max={600}
                  disabled={!draft.breaker}
                  onChange={breakerSeconds => patchDraft({ breakerSeconds })}
                />
                <NumberField
                  label={t('恢复成功(次)')}
                  value={draft.breakerSuccesses}
                  min={1}
                  max={10}
                  disabled={!draft.breaker}
                  onChange={breakerSuccesses => patchDraft({ breakerSuccesses })}
                />
              </div>
            </div>
          </DashboardCard>

          <DashboardCard icon='shield' title={t('限流恢复保护')} subtitle={t('防止过短恢复时间导致反复撞限')}>
            <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
              <NumberField
                label={t('最短限流冷却(秒)')}
                value={draft.cooldownFloorSeconds}
                min={1}
                max={60}
                onChange={cooldownFloorSeconds => patchDraft({ cooldownFloorSeconds })}
              />
              <div style={{ fontSize: 12, color: 'var(--fc-text-muted)', lineHeight: 1.5 }}>
                {t('限流或额度耗尽时至少冷却这么多秒，避免过快重试再次撞限；服务商明确给出的更长恢复时间不会被截短，后续收到真实恢复时间时会自动替换估算值。')}
              </div>
            </div>
          </DashboardCard>
        </>
      ) : (
        <CommonCard>
          <div style={{ fontSize: 15, fontWeight: 700, marginBottom: 10 }}>{t('始终保留的安全边界')}</div>
          <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
            {[
              { title: t('已经开始输出'), desc: t('不再从头重发，也不会把不同账号的半截回答拼接到一起。') },
              { title: t('任务或扣费状态不确定'), desc: t('缺乏可靠安全重放依据时，不会擅自重复提交请求。') },
              { title: t('验证、风控与明确拒绝'), desc: t('不尝试通过自动换号绕过验证码或风控拦截。') },
              { title: t('既定密钥与路由范围'), desc: t('严格遵守客户端 API Key 的模型/提供商授权限制。') },
            ].map(row => (
              <div key={row.title} className='fc-inner-block' style={{ padding: '10px 12px' }}>
                <div style={{ fontSize: 13.5, fontWeight: 700 }}>{row.title}</div>
                <div style={{ fontSize: 12.5, color: 'var(--fc-text-secondary)', marginTop: 2 }}>{row.desc}</div>
              </div>
            ))}
          </div>
        </CommonCard>
      )}

      <PickerSheet
        open={filterPickerOpen}
        title={t('筛选提供商')}
        options={exceptionFilterOptions}
        value={effectiveProvFilter}
        onSelect={setProvFilter}
        onClose={() => setFilterPickerOpen(false)}
      />

      <RoutingErrorRuleSheet
        open={Boolean(editor)}
        rule={editor?.rule || null}
        exception={Boolean(editor?.exception)}
        pluginDefault={editor?.pluginDefault}
        isNewCustom={editor?.isNew}
        providers={providers}
        onClose={() => setEditor(null)}
        onSave={applyEditedRule}
        onRemove={
          editor?.exception &&
          !editor?.isNew &&
          (!editor.pluginDefault ||
            draft.overrides.some(r => r.id === editor.rule.id || r.id === editor.pluginDefault?.pluginRuleId))
            ? removeEditedRule
            : undefined
        }
      />

      <BottomSheet
        open={confirmDefaultsOpen}
        title={t('恢复建议值')}
        onClose={() => setConfirmDefaultsOpen(false)}
        footer={
          <div style={{ display: 'flex', gap: 10, width: '100%' }}>
            <button type='button' className='fc-btn' style={{ flex: 1 }} onClick={() => setConfirmDefaultsOpen(false)}>
              {t('取消')}
            </button>
            <button
              type='button'
              className='fc-btn is-primary'
              style={{ flex: 1 }}
              onClick={() => {
                setDraft(structuredClone(view.defaults))
                setConfirmDefaultsOpen(false)
                store.toast(t('已填入建议值，点击保存后生效'), 'info')
              }}
            >
              {t('填入建议值')}
            </button>
          </div>
        }
      >
        <div style={{ fontSize: 14, color: 'var(--fc-text-secondary)', lineHeight: 1.55 }}>
          {t('会将错误处理配置还原为推荐默认参数（点保存后才正式生效）；现有账号与冷却记录保持不变。')}
        </div>
      </BottomSheet>
    </>
  )
}

function NumberField({
  label,
  value,
  min,
  max,
  disabled = false,
  onChange,
}: {
  label: string
  value: number
  min: number
  max: number
  disabled?: boolean
  onChange: (next: number) => void
}) {
  return (
    <div>
      <div style={{ fontSize: 11.5, color: 'var(--fc-text-secondary)', marginBottom: 4 }}>{label}</div>
      <input
        className='fc-input fc-mono'
        style={{ height: 38, fontSize: 13.5 }}
        type='number'
        min={min}
        max={max}
        disabled={disabled}
        value={value}
        onChange={e => onChange(Number(e.target.value) || 0)}
      />
    </div>
  )
}
