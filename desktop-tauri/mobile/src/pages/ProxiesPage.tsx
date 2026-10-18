import { useState } from 'react'
import { PageIntro, IconButton, Badge, Empty, ErrorNote, SkeletonList } from '../components/ui'
import { GlyphLogo } from '../components/ProviderLogo'
import { Icon } from '../components/Icon'
import { store } from '../state/store'
import {
  fetchProxyPool,
  setProxyEnabled,
  syncClashProxies,
  testProxy,
  type PoolItem,
  type PoolPayload,
} from '../api/gateway'
import { formatRelative } from '../lib/format'
import { usePolling } from '../lib/usePolling'
import { t } from '../i18n'

/**
 * 网络代理。
 *
 * 每个出口一张卡：地址、上次测试结果、被几个账号引用、开关。
 * 「引用数」是删除前最需要知道的事 —— 删掉一个还有账号挂着的出口，
 * 那些账号会静默回退直连。
 *
 * 「测试连通性」是行内按钮，结果就地显示在卡里，不弹窗打断。
 */
export function ProxiesPage() {
  const [data, setData] = useState<PoolPayload | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState('')
  const [tested, setTested] = useState<Record<string, { ok: boolean; text: string }>>({})

  const load = async () => {
    try {
      const next = await fetchProxyPool()
      setData(next)
      setError('')
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
    }
  }

  usePolling(load, 120_000)

  const items = data?.items || []
  const enabled = items.filter(item => item.enabled).length
  const clash = data?.clash

  const toggle = async (item: PoolItem, next: boolean) => {
    if (!item.id) return
    setBusy(item.id)
    try {
      const payload = await setProxyEnabled(item.id, next)
      setData(payload)
      store.toast(next ? t('已启用「{name}」', { name: item.name || item.id }) : t('已禁用「{name}」', { name: item.name || item.id }))
    } catch (failure) {
      store.toast(failure instanceof Error ? failure.message : String(failure), 'err')
    } finally {
      setBusy('')
    }
  }

  const test = async (item: PoolItem) => {
    if (!item.id) return
    setBusy(item.id)
    try {
      const result = await testProxy(item.id)
      const ok = result.ok !== false && !result.error
      setTested(prev => ({
        ...prev,
        [item.id as string]: {
          ok,
          text: ok ? (result.ip ? t('出口 IP {ip}', { ip: result.ip }) : t('连接成功')) : String(result.error || t('连接失败')),
        },
      }))
      store.toast(ok ? t('出口可用') : t('连接失败'), ok ? 'ok' : 'err')
      await load()
    } catch (failure) {
      const message = failure instanceof Error ? failure.message : String(failure)
      setTested(prev => ({ ...prev, [item.id as string]: { ok: false, text: message } }))
      store.toast(message, 'err')
    } finally {
      setBusy('')
    }
  }

  const sync = async () => {
    setBusy('sync')
    try {
      const result = await syncClashProxies()
      setData(result)
      if (result.syncError) store.toast(t('Clash 同步失败：{msg}', { msg: result.syncError }), 'err')
      else store.toast(t('已同步 {n} 项变更', { n: result.changes || 0 }))
    } catch (failure) {
      store.toast(failure instanceof Error ? failure.message : String(failure), 'err')
    } finally {
      setBusy('')
    }
  }

  return (
    <div className='m-page'>
      <PageIntro actions={<IconButton name='refresh' label={t('刷新')} onClick={() => void load()} />}
      />

      <div className='m-section' style={{ marginTop: 0 }}>
        <div className='m-card'>
          <button type='button' className='m-row' onClick={() => void sync()} disabled={busy === 'sync'}>
            <GlyphLogo name='refresh' />
            <div className='m-grow'>
              <div className='m-t1'>{busy === 'sync' ? t('同步中…') : t('同步 Clash Verge 出口')}</div>
              <div className='m-t2'>
                {clash?.available
                  ? t('镜像当前出口集合，名称与端口跟随 Clash')
                  : clash?.error || t('本机没有读到 Clash Verge 配置')}
              </div>
            </div>
            <span className='m-chev'>
              <Icon name='chev' />
            </span>
          </button>
        </div>
      </div>

      {error ? <ErrorNote text={error} /> : null}
      {!data ? <SkeletonList rows={3} height={104} /> : null}

      {data && !items.length ? (
        <div className='m-section'>
          <div className='m-card'>
            <Empty text={t('还没有出口。在桌面端「网络代理」页新增，或点上面同步 Clash。')} />
          </div>
        </div>
      ) : null}

      {items.map(item => {
        const id = String(item.id || '')
        const result = tested[id]
        const last = item.lastTest
        const ok = result ? result.ok : last?.success
        const text = result
          ? result.text
          : last?.at
            ? last.success
              ? last.ip
                ? t('出口 IP {ip}', { ip: last.ip })
                : t('连接成功')
              : String(last.error || t('连接失败'))
            : t('未测试')
        return (
          <div key={id} className='m-section' style={{ marginTop: 10 }}>
            <div className='m-card m-card-pad'>
              <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
                <GlyphLogo name='globe' />
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div style={{ fontSize: 14, fontWeight: 600, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                    {item.name || t('未命名')}
                  </div>
                  <div style={{ fontSize: 11.5, color: 'var(--m-text-3)', marginTop: 2, fontFamily: 'var(--m-mono)' }}>
                    {addressOf(item)}
                  </div>
                </div>
                <SwitchSmall
                  checked={Boolean(item.enabled)}
                  disabled={busy === id}
                  label={item.name || id}
                  onChange={next => void toggle(item, next)}
                />
              </div>

              <div
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  gap: 10,
                  marginTop: 11,
                  paddingTop: 10,
                  borderTop: '1px solid var(--m-line)',
                  fontSize: 11.5,
                  color: 'var(--m-text-3)',
                }}
              >
                <span style={{ display: 'inline-flex', alignItems: 'center', gap: 5, minWidth: 0 }}>
                  <i
                    style={{
                      width: 6,
                      height: 6,
                      borderRadius: '50%',
                      flex: 'none',
                      background: ok === undefined ? 'var(--m-text-3)' : ok ? 'var(--m-ok)' : 'var(--m-danger)',
                    }}
                  />
                  <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{text}</span>
                </span>
                {item.usedBy ? <span style={{ flex: 'none' }}>{t('{n} 个账号引用', { n: item.usedBy })}</span> : null}
                {last?.at ? <span style={{ flex: 'none' }}>{formatRelative(last.at)}</span> : null}
                <button
                  type='button'
                  className='m-btn m-sm m-ghost'
                  style={{ marginLeft: 'auto', height: 28, fontSize: 11.5, padding: '0 10px' }}
                  disabled={busy === id}
                  onClick={() => void test(item)}
                >
                  {busy === id ? t('测试中…') : t('测试')}
                </button>
              </div>

              {item.source === 'subscription' ? (
                <div style={{ marginTop: 8 }}>
                  <Badge tone='brand'>{t('订阅节点')}</Badge>
                </div>
              ) : item.source === 'clash' ? (
                <div style={{ marginTop: 8 }}>
                  <Badge tone='mute'>{t('Clash 镜像')}</Badge>
                </div>
              ) : null}
            </div>
          </div>
        )
      })}

      <div className='m-section' style={{ paddingBottom: 8 }}>
        <div className='m-card m-card-pad' style={{ fontSize: 11.5, color: 'var(--m-text-3)', lineHeight: 1.7 }}>
          {t('新增出口、编辑地址与订阅管理在桌面端完成；手机端做启停与连通性测试。')}
        </div>
      </div>
    </div>
  )
}

function addressOf(item: PoolItem): string {
  if (item.address) return String(item.address)
  const protocol = item.protocol || 'http'
  if (item.host && item.port) return `${protocol}://${item.host}:${item.port}`
  return t('未分配端口')
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
    <button
      type='button'
      role='switch'
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      style={{
        width: 40,
        height: 24,
        borderRadius: 999,
        border: checked ? '0' : '1px solid var(--m-line-strong)',
        background: checked ? 'var(--m-ok)' : 'var(--m-surface-3)',
        position: 'relative',
        transition: 'background .18s',
        padding: 0,
        flex: 'none',
        opacity: disabled ? 0.5 : 1,
      }}
    >
      <span
        style={{
          position: 'absolute',
          top: 2,
          left: checked ? 18 : 2,
          width: 20,
          height: 20,
          borderRadius: '50%',
          background: '#fff',
          boxShadow: '0 1px 3px rgba(0,0,0,.25)',
          transition: 'left .18s cubic-bezier(.3,.9,.4,1)',
        }}
      />
    </button>
  )
}
