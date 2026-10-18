import { useState } from 'react'
import { PageIntro, IconButton, Badge, Empty, ErrorNote, SkeletonList } from '../components/ui'
import { GlyphLogo } from '../components/ProviderLogo'
import { Icon } from '../components/Icon'
import { store } from '../state/store'
import { fetchAgents, restoreAgentClient, type AgentClient } from '../api/gateway'
import { usePolling } from '../lib/usePolling'
import { t } from '../i18n'

/**
 * Agent 接管。
 *
 * ── 手机上只读，这是有意的 ────────────────────────────────────
 * 这一页的作用是改写**本机 AI 客户端的配置文件**（`~/.claude/settings.json`
 * 之类）。手机读不到那台机器的文件系统 —— 它连的是网关，而网关跑在另一台
 * 机器上。所以手机上：
 *   · 看得到有哪些客户端、各自的配置路径、是否已接管；
 *   · 可以「还原」（走网关的 `/api/agents/restore`，那是网关在替我们写）；
 *   · **不能**改配置内容（那需要浏览本机文件、选 profile）。
 *
 * 页内明说这一点，免得用户以为功能坏了。
 */
export function AgentPage() {
  const [clients, setClients] = useState<AgentClient[] | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState('')

  const load = async () => {
    try {
      const data = await fetchAgents()
      setClients(data.clients || [])
      setError('')
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
    }
  }

  usePolling(load, 120_000)

  const restore = async (client: AgentClient) => {
    if (!client.id) return
    if (!window.confirm(t('确定按最近一次备份还原「{name}」？', { name: client.name || client.id }))) return
    setBusy(client.id)
    try {
      await restoreAgentClient(client.id)
      store.toast(t('已还原「{name}」', { name: client.name || client.id }))
      await load()
    } catch (failure) {
      store.toast(failure instanceof Error ? failure.message : String(failure), 'err')
    } finally {
      setBusy('')
    }
  }

  return (
    <div className='m-page'>
      <PageIntro subtitle={t('改写网关所在机器上的客户端配置')}
        actions={<IconButton name='refresh' label={t('刷新')} onClick={() => void load()} />}
      />

      <div className='m-section' style={{ marginTop: 0 }}>
        <div
          className='m-card m-card-pad'
          style={{ background: 'var(--m-warn-soft)', borderColor: 'color-mix(in srgb, var(--m-warn) 24%, transparent)' }}
        >
          <div style={{ fontSize: 12.3, lineHeight: 1.6, color: 'var(--m-text-2)' }}>
            {t('手机上只能查看与还原接管。改写配置内容请回桌面端')}
            <b style={{ color: 'var(--m-text)' }}>{t('（手机读不到本机文件）')}</b>
            {t('。这里的开关与还原作用在网关所在的机器上。')}
          </div>
        </div>
      </div>

      {error ? <ErrorNote text={error} /> : null}
      {!clients ? <SkeletonList rows={3} height={96} /> : null}

      {clients && !clients.length ? (
        <div className='m-section'>
          <div className='m-card'>
            <Empty text={t('没有检测到可接管的客户端')} />
          </div>
        </div>
      ) : null}

      {(clients || []).map(client => (
        <div key={client.id} className='m-section' style={{ marginTop: 10 }}>
          <div className='m-card m-card-pad'>
            <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
              <GlyphLogo name='agent' />
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ fontSize: 14, fontWeight: 600 }}>{client.name || client.id}</div>
                <div
                  className='m-mono'
                  style={{
                    fontSize: 11,
                    color: 'var(--m-text-3)',
                    marginTop: 3,
                    overflow: 'hidden',
                    textOverflow: 'ellipsis',
                    whiteSpace: 'nowrap',
                  }}
                >
                  {client.path || '—'}
                </div>
              </div>
              <Badge tone={client.applied ? 'ok' : 'mute'}>{client.applied ? t('已接管') : t('未接管')}</Badge>
            </div>
            <div
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: 8,
                marginTop: 11,
                paddingTop: 10,
                borderTop: '1px solid var(--m-line)',
                fontSize: 11.5,
                color: 'var(--m-text-3)',
              }}
            >
              <span>{client.mode || t('单配置文件')}</span>
              {client.applied ? (
                <button
                  type='button'
                  className='m-btn m-sm m-ghost'
                  style={{ marginLeft: 'auto', height: 28, fontSize: 11.5, padding: '0 10px' }}
                  disabled={busy === client.id}
                  onClick={() => void restore(client)}
                >
                  <Icon name='refresh' style={{ width: 12, height: 12 }} />
                  {busy === client.id ? t('还原中…') : t('还原')}
                </button>
              ) : null}
            </div>
          </div>
        </div>
      ))}

      <div className='m-section' style={{ paddingBottom: 8 }}>
        <div className='m-card m-card-pad' style={{ fontSize: 11.5, color: 'var(--m-text-3)', lineHeight: 1.7 }}>
          {t('接管会把客户端的 base_url 指向本网关，并在写入前自动备份原文件。')}
        </div>
      </div>
    </div>
  )
}
