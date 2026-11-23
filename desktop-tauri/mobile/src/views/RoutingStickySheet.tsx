import { useCallback, useEffect, useState } from 'react'
import { clearRoutingSticky, fetchRoutingSticky, type StickyEntry } from '../core/api'
import { formatDateTime, formatDuration, providerDisplayName } from '../core/format'
import { t } from '../core/i18n'
import { store } from '../core/store'
import { BottomSheet, CommonCard, EmptyBlock, SearchField, TonalChip } from '../ui/components'

const STICKY_NAMES: Record<string, string> = {
  auto: '自动识别',
  session: '整段会话固定',
  turn: '单轮工具链固定',
  off: '关闭粘性',
}

export function RoutingStickySheet({
  open,
  model,
  onClose,
  onChanged,
}: {
  open: boolean
  model?: string
  onClose: () => void
  onChanged?: () => void
}) {
  const [entries, setEntries] = useState<StickyEntry[]>([])
  const [mode, setMode] = useState('')
  const [now, setNow] = useState(Date.now())
  const [offset, setOffset] = useState(0)
  const [search, setSearch] = useState('')
  const [busy, setBusy] = useState(false)
  const [loading, setLoading] = useState(true)

  const load = useCallback(async () => {
    setLoading(true)
    try {
      const res = await fetchRoutingSticky(model || undefined)
      setEntries(res.entries || [])
      setMode(res.sticky || '')
      setOffset((res.now || Date.now()) - Date.now())
      setNow(res.now || Date.now())
    } catch (err) {
      store.toast(err instanceof Error ? err.message : t('读取粘性会话失败'), 'danger')
    } finally {
      setLoading(false)
    }
  }, [model])

  useEffect(() => {
    if (!open) return
    void load()
  }, [open, load])

  useEffect(() => {
    if (!open) return
    const timer = window.setInterval(() => setNow(Date.now() + offset), 1000)
    return () => window.clearInterval(timer)
  }, [open, offset])

  const active = entries.filter(e => e.expiresAt > now)
  const q = search.trim().toLowerCase()
  const visible = q
    ? active.filter(e =>
        `${e.model} ${e.accountName} ${e.provider} ${e.providerLabel} ${e.source} ${e.keyPreview}`
          .toLowerCase()
          .includes(q),
      )
    : active

  const handleClear = async (entry?: StickyEntry) => {
    if (busy) return
    setBusy(true)
    try {
      const res = await clearRoutingSticky(
        entry ? { id: entry.id, model: entry.model } : model?.trim() ? { model: model.trim() } : { all: true },
      )
      store.toast(t('已解除 {n} 条会话绑定', { n: res.cleared ?? 0 }), 'success')
      await load()
      onChanged?.()
    } catch (err) {
      store.toast(err instanceof Error ? err.message : t('解除失败'), 'danger')
    } finally {
      setBusy(false)
    }
  }

  return (
    <BottomSheet
      open={open}
      title={t('活跃粘性会话')}
      onClose={onClose}
      actions={
        <button
          type='button'
          className='fc-btn is-sm is-pill'
          disabled={loading || busy}
          onClick={() => void load()}
        >
          {t('刷新')}
        </button>
      }
      fixedHeader={
        <>
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8 }}>
            <TonalChip tone='primary'>{t(STICKY_NAMES[mode] || mode || '自动识别')}</TonalChip>
            <span style={{ fontSize: 12.5, color: 'var(--fc-text-secondary)' }}>
              {t('{n} 条活跃绑定', { n: active.length })}
            </span>
          </div>
          <SearchField
            inSheet
            value={search}
            onChange={setSearch}
            placeholder={t('搜索模型、账号或来源…')}
          />
        </>
      }
      footer={
        <div style={{ display: 'flex', gap: 10, width: '100%' }}>
          <button
            type='button'
            className='fc-btn is-danger'
            style={{ flex: 1 }}
            disabled={busy || !active.length}
            onClick={() => void handleClear()}
          >
            {t('解除全部绑定')}
          </button>
          <button type='button' className='fc-btn is-primary' style={{ flex: 1 }} onClick={onClose}>
            {t('完成')}
          </button>
        </div>
      }
    >
      {mode === 'off' || mode === 'turn' ? (
        <CommonCard>
          <div style={{ fontSize: 12.5, color: 'var(--fc-warning)' }}>
            {t('当前粘性档位不跨请求保留绑定，因此没有活跃记录。')}
          </div>
        </CommonCard>
      ) : null}

      {loading && !visible.length ? (
        <EmptyBlock icon='clock' title={t('正在加载粘性会话…')} />
      ) : !visible.length ? (
        <EmptyBlock
          icon='route'
          title={search ? t('没有匹配的绑定') : t('当前暂无活跃会话绑定')}
          description={t('解除绑定不会关闭会话或清理历史，仅让下一次请求重新选号')}
        />
      ) : (
        visible.map(entry => (
          <CommonCard key={entry.id}>
            <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8 }}>
              <div style={{ minWidth: 0, flex: 1 }}>
                <div style={{ fontSize: 14.5, fontWeight: 700 }}>{entry.accountName}</div>
                <div style={{ fontSize: 12, color: 'var(--fc-text-secondary)', marginTop: 2 }}>
                  {providerDisplayName(entry.provider, entry.providerLabel)} ·{' '}
                  <span className='fc-mono'>{entry.model}</span>
                </div>
              </div>
              <button
                type='button'
                className='fc-btn is-sm is-tonal'
                disabled={busy}
                onClick={() => void handleClear(entry)}
              >
                {t('解除')}
              </button>
            </div>
            <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6, marginTop: 8, alignItems: 'center' }}>
              <TonalChip>{entry.source === 'content-hash' ? t('内容哈希') : entry.source}</TonalChip>
              {entry.keyPreview ? <TonalChip>{entry.keyPreview}</TonalChip> : null}
              <span style={{ fontSize: 12, color: 'var(--fc-text-muted)' }}>
                {t('命中 {n} 次', { n: entry.hits })} · {t('剩余 {d}', { d: formatDuration(entry.expiresAt - now) })}
              </span>
            </div>
            <div style={{ fontSize: 11.5, color: 'var(--fc-text-tertiary)', marginTop: 4 }}>
              {t('最近使用')}：{formatDateTime(entry.lastSeen)}
            </div>
          </CommonCard>
        ))
      )}
    </BottomSheet>
  )
}
