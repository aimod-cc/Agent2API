import { modelReasoning } from '../../../ui-shared/models-reasoning'
import { activeModelBindings } from '../../../ui-shared/models-bindings'
import { InputGroup, InputGroupInput, InputGroupAddon, InputGroupButton } from '@ui'
import { useMemo, useRef, useState } from 'react'
import { PageIntro, IconButton, Badge, Empty, ErrorNote, SkeletonList } from '../components/ui'
import { ProviderLogo } from '../components/ProviderLogo'
import { Icon } from '../components/Icon'
import { PullIndicator, usePullToRefresh } from '../components/PullToRefresh'
import { store } from '../state/store'
import { providerLabel } from '../lib/providers'
import { fetchManage, refreshModels, setModelEnabled, removeModelMapping, type ManageModel, type ManageMapping } from '../api/gateway'
import { usePolling } from '../lib/usePolling'
import { t } from '../i18n'
import { Input, Card, Button, confirmDialog } from '@ui'
import { Switch as UiSwitch } from '@ui'
import { Badge as UiBadge } from '@ui'
import { Toggle as UiToggle } from '@ui'

/**
 * 模型管理。
 *
 * ── 折叠分组代替左栏 + 宽表 ──────────────────────────────────
 * 桌面端是「左侧提供商栏 + 右侧宽表」，手机上改成按提供商折叠的卡片：
 * 一行一个模型，只留扫读需要的四个事实 —— 模型 ID、上下文窗口、来源、
 * 是否展示。映射与能力位编辑进抽屉（在桌面端做）。
 *
 * ── 两个开关要分开 ────────────────────────────────────────────
 * 「启用」决定这个模型对客户端是否可用；「可见性」是上游官方客户端是否
 * 展示它 —— 两者在桌面端是两列，手机上用小标签表达，不合并成一个开关。
 */

type Filter = 'all' | 'enabled' | 'hidden'

export function ModelsPage() {
  const [models, setModels] = useState<ManageModel[] | null>(null)
  const [mappings, setMappings] = useState<ManageMapping[]>([])
  const [error, setError] = useState('')
  const [filter, setFilter] = useState<Filter>('all')
  const [query, setQuery] = useState('')
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set())
  const [busy, setBusy] = useState('')
  const revision = useRef(0)
  const [refreshResults, setRefreshResults] = useState<Array<{ provider: string; providerLabel?: string; status: string; message?: string }>>([])

  const load = async () => {
    const current = ++revision.current
    try {
      const view = await fetchManage()
      if (current !== revision.current) return
      setModels(view.models || [])
      setMappings(view.mappings || [])
      setError('')
    } catch (failure) {
      if (current !== revision.current) return
      setError(failure instanceof Error ? failure.message : String(failure))
    }
  }

  usePolling(load, 120_000)

  const pull = usePullToRefresh({ onRefresh: load })

  const filtered = useMemo(() => {
    const keyword = query.trim().toLowerCase()
    return (models || []).filter(item => {
      if (filter === 'enabled' && !item.enabled) return false
      if (filter === 'hidden' && item.visibility !== 'hidden') return false
      if (!keyword) return true
      return `${item.id || ''} ${item.providerLabel || ''} ${item.provider || ''}`.toLowerCase().includes(keyword)
    })
  }, [models, filter, query])

  const groups = useMemo(() => {
    const map = new Map<string, ManageModel[]>()
    filtered.forEach(item => {
      const provider = String(item.provider || 'unknown')
      const list = map.get(provider) || []
      list.push(item)
      map.set(provider, list)
    })
    return [...map.entries()].map(([provider, list]) => ({
      provider,
      label: list[0]?.providerLabel,
      list: [...list].sort((a, b) => String(a.id).localeCompare(String(b.id))),
    }))
  }, [filtered])

  const enabledCount = (models || []).filter(item => item.enabled).length
  const hiddenCount = (models || []).filter(item => item.visibility === 'hidden').length

  /**
   * 「未挂载的映射」：指向的上游模型已不在清单里（上游下线 / 承载它的那家当前
   * 没有行），因此挂不到任何模型行上。与桌面端同一份判据（后端的 `dangling`），
   * 手机端只做**提示 + 清理** —— 常规的映射编辑仍在桌面端「模型管理」里做。
   */
  const danglingMappings = useMemo(
    () => mappings.filter(item => item.dangling === true)
      .sort((a, b) => String(a.provider || '').localeCompare(String(b.provider || ''))
        || String(a.alias || '').localeCompare(String(b.alias || ''))),
    [mappings],
  )

  const removeDangling = async (item: ManageMapping) => {
    if (busy) return
    const alias = String(item.alias || '')
    const target = String(item.target || '')
    if (!await confirmDialog(t('确定删除映射「{alias} → {target}」？它指向的上游模型已不在清单里。', { alias, target }))) return
    setBusy(`mapping:${alias}:${target}`)
    try {
      const view = await removeModelMapping({ alias, target, provider: item.provider })
      revision.current++
      setModels(view.models || [])
      setMappings(view.mappings || [])
      store.toast(t('映射已删除'))
    } catch (failure) {
      store.toast(failure instanceof Error ? failure.message : String(failure), 'err')
    } finally {
      setBusy('')
    }
  }

  const toggle = async (model: ManageModel, next: boolean) => {
    if (!model.id || !model.provider || busy) return
    const conflicts = next ? activeModelBindings(
      (models || []).filter(item => item.provider === model.provider),
      mappings.filter(item => !item.provider || item.provider === model.provider),
      model.id, model.id,
    ) : []
    setBusy(`${model.provider}:${model.id}`)
    try {
      const view = await setModelEnabled({ id: model.id, provider: model.provider, enabled: next })
      revision.current++
      setModels(view.models || [])
      setMappings(view.mappings || [])
      store.toast(next ? (conflicts.length ? t('已启用 {name}，其他同名绑定已自动关闭', { name: model.id })
        : t('已启用 {name}', { name: model.id })) : t('已禁用 {name}', { name: model.id }))
    } catch (failure) {
      store.toast(failure instanceof Error ? failure.message : String(failure), 'err')
    } finally {
      setBusy('')
    }
  }

  const refresh = async () => {
    if (busy) return
    setBusy('refresh')
    setRefreshResults([])
    try {
      const result = await refreshModels() as { results?: Array<{ provider: string; providerLabel?: string; status: string; message?: string }> }
      if (!Array.isArray(result?.results) || result.results.length === 0 || result.results.some(item => !item || typeof item.provider !== 'string'
        || !item.provider || !['refreshed', 'failed', 'skipped'].includes(item.status))
        || new Set(result.results.map(item => item.provider)).size !== result.results.length) {
        throw new Error(t('网关未返回逐家模型目录刷新结果'))
      }
      setRefreshResults(result.results)
      await load()
      const done = result.results.filter(item => item.status === 'refreshed').length
      const failed = result.results.filter(item => item.status === 'failed').length
      store.toast(t('{total} 家 · 成功 {done} · 失败 {failed}', { total: result.results.length, done, failed }), failed ? 'err' : done ? 'ok' : 'warn')
    } catch (failure) {
      store.toast(failure instanceof Error ? failure.message : String(failure), 'err')
    } finally {
      setBusy('')
    }
  }

  return (
    <div className='m-page m-fixed' ref={pull.bindRef}>
      <PullIndicator pull={pull.pull} refreshing={pull.refreshing} />
      <div className='m-sticky'>
        <PageIntro actions={<IconButton name='refresh' label={t('刷新清单')} onClick={() => void refresh()} />}
        />

      <div className='m-section' style={{ marginTop: 0 }}>
        <InputGroup>
          <InputGroupInput value={query} onChange={event => setQuery(event.target.value)} placeholder={t('搜索模型 ID / 提供商')} />
          <InputGroupAddon><Icon name='search' style={{ width: 16, height: 16 }} /></InputGroupAddon>

        </InputGroup>
      </div>

      <div className='m-chips' style={{ padding: '10px 16px 12px' }}>
        <Chip label={t('全部')} count={(models || []).length} on={filter === 'all'} onClick={() => setFilter('all')} />
        <Chip label={t('已启用')} count={enabledCount} on={filter === 'enabled'} onClick={() => setFilter('enabled')} />
        <Chip label={t('不展示')} count={hiddenCount} on={filter === 'hidden'} onClick={() => setFilter('hidden')} />
      </div>
      </div>

      <div className='m-body'>
      {error ? <ErrorNote text={error} /> : null}
      {refreshResults.length ? <div className='m-section'><Card className='m-card m-card-pad'>
        <div className='text-xs font-medium'>{t('模型目录刷新结果')}</div>
        {refreshResults.map(item => <p key={item.provider} className='mt-2 text-xs text-subtle'>
          {providerLabel(item.provider, item.providerLabel)} · {item.status === 'refreshed' ? t('已刷新')
            : item.status === 'failed' ? t('失败') : t('跳过')}
          {item.message ? ` · ${t(item.message)}` : ''}
        </p>)}
      </Card></div> : null}

      {/* 未挂载的映射：目标上游已不在清单里，别名调不通也挂不到行上。桌面端在
          表格下方单列一块（models-dangling.tsx）；手机端只提示 + 清理。 */}
      {danglingMappings.length ? (
        <div className='m-section'>
          <Card className='m-card m-card-pad'>
            <div className='text-xs font-medium'>
              {t('未挂载的模型映射')} · {danglingMappings.length}
            </div>
            <p className='mt-2 text-[11.5px] text-subtle' style={{ lineHeight: 1.65 }}>
              {t('这些映射指向的上游模型已不在当前清单里，用对应别名发请求会失败。可以在这里删除，删掉后该对外名即可重新使用。')}
            </p>
            {danglingMappings.map(item => {
              const key = `${item.provider || ''}:${item.alias || ''}:${item.target || ''}`
              // 自映射（alias == target）= 模型自己的默认绑定：用户启停过它就会留下
              // 这一条，模型下线后挂不到行上。显示成 `x → x` 只会让人困惑，只写一次。
              const isDefault = item.isDefault === true
                || String(item.alias || '').trim().toLowerCase() === String(item.target || '').trim().toLowerCase()
              return (
                <div key={key} style={{
                  display: 'flex', alignItems: 'center', gap: 10, marginTop: 10,
                  paddingTop: 10, borderTop: '1px solid var(--m-line)',
                }}>
                  <div style={{ flex: 1, minWidth: 0 }}>
                    <div className='m-mono' style={{ fontSize: 12, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                      {item.alias}{isDefault ? '' : ` → ${item.target}`}
                    </div>
                    <div style={{ fontSize: 10.5, color: 'var(--m-text-3)', marginTop: 3 }}>
                      {providerLabel(item.provider)}
                      {isDefault ? ` · ${t('默认绑定')}` : ''}
                      {item.carried === false ? ` · ${t('目标已下线')}` : ` · ${t('未接入')}`}
                    </div>
                  </div>
                  <Button size='sm' variant='ghost' className='text-destructive'
                    onClick={() => void removeDangling(item)}>{t('删除')}</Button>
                </div>
              )
            })}
          </Card>
        </div>
      ) : null}
      {!models ? <SkeletonList rows={4} height={70} /> : null}

      {models && !groups.length ? (
        <div className='m-section'>
          <Card className='m-card'>
            <Empty text={t('没有匹配的模型')} />
          </Card>
        </div>
      ) : null}

      {groups.map(group => {
        const isCollapsed = collapsed.has(group.provider)
        return (
          <div key={group.provider} className='m-section' style={{ marginTop: 12 }}>
            <Button size='default' variant='ghost'
              type='button'
              onClick={() => {
                const next = new Set(collapsed)
                if (isCollapsed) next.delete(group.provider)
                else next.add(group.provider)
                setCollapsed(next)
              }}
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: 9,
                width: '100%',
                padding: '0 3px 8px',
                background: 'none',
                border: 0,
                textAlign: 'left',
              }}
            >
              <ProviderLogo id={group.provider} label={group.label} />
              <span style={{ flex: 1, fontSize: 13, fontWeight: 600 }}>{providerLabel(group.provider, group.label)}</span>
              <span style={{ fontSize: 11, color: 'var(--m-text-3)' }}>{t('{n} 个模型', { n: group.list.length })}</span>
              <Icon
                name='chev'
                style={{
                  width: 15,
                  height: 15,
                  color: 'var(--m-text-3)',
                  transform: isCollapsed ? 'none' : 'rotate(90deg)',
                  transition: 'transform .16s',
                }}
              />
            </Button>
            {isCollapsed ? null : (
              <Card className='m-card'>
                {group.list.map(model => {
                  const key = `${model.provider}:${model.id}`
                  const hidden = model.visibility === 'hidden'
                  const caps = readCaps(model.capabilities)
                  const reasoningInfo = modelReasoning(model)
                  return (
                    <div
                      key={key}
                      style={{
                        display: 'flex',
                        alignItems: 'center',
                        gap: 10,
                        padding: '11px 13px',
                        borderBottom: '1px solid var(--m-line)',
                        opacity: model.enabled ? 1 : 0.55,
                      }}
                    >
                      <div style={{ flex: 1, minWidth: 0 }}>
                        <div
                          className='m-mono'
                          style={{ fontSize: 12.5, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}
                        >
                          {model.id}
                        </div>
                        <div style={{ display: 'flex', gap: 6, marginTop: 4, fontSize: 10.5, color: 'var(--m-text-3)', flexWrap: 'wrap' }}>
                          {model.kind === 'image' ? <UiBadge variant='secondary' shape='tag' className='m-pill'>{t('图片生成')}</UiBadge> : null}
                          {model.supports_image_editing ? <UiBadge variant='secondary' shape='tag' className='m-pill'>{t('图片编辑')}</UiBadge> : null}
                          {model.upstreamStatus === 'undeployed' ? (
                            <UiBadge variant='warning' shape='tag'
                              title={t('上游目录列出了这个模型，但推理端尚未识别；保留在清单中，后续探测成功后自动清除标记')}>
                              {t('上游未部署')}
                            </UiBadge>
                          ) : null}
                          {caps.context ? <UiBadge variant='secondary' shape='tag' className='m-pill'>{caps.context}</UiBadge> : null}
                          <UiBadge variant='secondary' shape='tag' className='m-pill'>{sourceLabel(model.source)}</UiBadge>
                          {model.aliases?.length ? (
                            <UiBadge variant='secondary' shape='tag' className='m-pill' style={{ color: 'var(--m-primary)' }}>
                              {t('{n} 个映射', { n: model.aliases.length })}
                            </UiBadge>
                          ) : null}
                          {hidden ? (
                            <UiBadge variant='secondary' shape='tag' className='m-pill' style={{ color: 'var(--m-warn)' }}>
                              {t('不展示')}
                            </UiBadge>
                          ) : null}
                          {caps.reasoning ? (
                            <UiBadge variant='secondary' shape='tag' className='m-pill' style={{ color: 'var(--m-primary)' }}>
                              {t('思考')}
                            </UiBadge>
                          ) : null}
                        </div>
                        {reasoningInfo.fixedLevel ? (
                          <p className='mt-1 text-[10.5px] text-subtle'>{t('固定思考档位：{level}', { level: reasoningInfo.fixedLevel })}</p>
                        ) : reasoningInfo.levels.length ? (
                          <p className='mt-1 text-[10.5px] text-subtle'>
                            {reasoningInfo.source === 'modelsdev' ? t('社区参考档位：{levels}', { levels: reasoningInfo.levels.join(' / ') })
                              : t('上游可用档位：{levels}', { levels: reasoningInfo.levels.join(' / ') })}
                            {reasoningInfo.defaultLevel ? ` · ${t('默认档位：{level}', { level: reasoningInfo.defaultLevel })}` : ''}
                          </p>
                        ) : reasoningInfo.bindingSupported === false ? (
                          <p className='mt-1 text-[10.5px] text-subtle'>{t('当前通道未声明可用档位，映射等级不会注入；请先获取模型目录。')}</p>
                        ) : null}
                      </div>
                      <SwitchSmall
                        checked={Boolean(model.enabled)}
                        disabled={Boolean(busy)}
                        label={String(model.id)}
                        onChange={next => void toggle(model, next)}
                      />
                    </div>
                  )
                })}
              </Card>
            )}
          </div>
        )
      })}

      <div className='m-section' style={{ paddingBottom: 8 }}>
        <Card className='m-card m-card-pad' style={{ fontSize: 11.5, color: 'var(--m-text-3)', lineHeight: 1.7 }}>
          {t('映射、思考等级、上下文窗口与工具调用能力在桌面端「模型管理」里改；手机端只做启停。')}
        </Card>
      </div>
      </div>
    </div>
  )
}

function Chip({ label, count, on, onClick }: { label: string; count: number; on: boolean; onClick: () => void }) {
  return (
    <UiToggle size='default' variant='outline' type='button' className='m-chip rounded-pill' pressed={on} onClick={onClick}>
      {label}
      <span className='m-n'>{count}</span>
    </UiToggle>
  )
}

function SwitchSmall({
  checked,
  onChange,
  disabled,
  label,
}: {
  checked: boolean
  onChange: (next: boolean) => void
  disabled?: boolean
  label: string
}) {
  return (
    <UiSwitch size='touch' variant='success' checked={checked} aria-label={label} disabled={disabled} onCheckedChange={onChange} />
  )
}

function sourceLabel(source?: string): string {
  if (source === 'remote') return t('远程')
  if (source === 'builtin') return t('内置')
  if (source === 'manual') return t('手动')
  return t('自定义')
}

/** 从能力位里读两件手机上值得显示的事实（上下文窗口 / 是否支持思考） */
function readCaps(capabilities: unknown): { context?: string; reasoning?: boolean } {
  if (!capabilities || typeof capabilities !== 'object') return {}
  const caps = capabilities as Record<string, unknown>
  const input = caps.maxInputTokens ?? caps.contextWindow ?? caps.maxContextTokens
  const reasoning = Boolean(caps.reasoning || caps.reasoningLevels || caps.supportsReasoning)
  return {
    context: typeof input === 'number' && input > 0 ? formatWindow(input) : undefined,
    reasoning,
  }
}

function formatWindow(tokens: number): string {
  if (tokens >= 1_000_000) return `${(tokens / 1_000_000).toFixed(tokens % 1_000_000 ? 1 : 0)}M`
  if (tokens >= 1000) return `${Math.round(tokens / 1000)}K`
  return String(tokens)
}
