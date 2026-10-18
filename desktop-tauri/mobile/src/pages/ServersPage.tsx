import { useState } from 'react'
import { Icon } from '../components/Icon'
import { PageHead, SectionHead, ErrorNote } from '../components/ui'
import { store, useStore } from '../state/store'
import { probe } from '../api/client'
import { baseLabel, normalizeBase } from '../lib/servers'
import { t } from '../i18n'

/**
 * 服务器配置（仅 APK）。
 *
 * ── 为什么单独一屏而不是塞进设置页 ────────────────────────────
 * 首次启动时还没有任何服务器，所有页面都拿不到数据 —— 这时设置页本身也
 * 是空的。所以它必须是**前置**的一屏：没配置就进不去主界面。
 *
 * ── 为什么先探活再保存 ────────────────────────────────────────
 * 地址填错（少写端口、多写 `/m`、网关没开局域网访问）是最高频的失败。
 * 保存前打一次 `/health`，把「连不上」当场说清楚，比存进去之后每个页面
 * 各报一次错要好得多。探活成功顺带显示网关版本，用户能确认连的是对的那台。
 */
export function ServersPage() {
  const { servers, server } = useStore()
  const [editing, setEditing] = useState<string | null>(null)
  const [draftName, setDraftName] = useState('')
  const [draftBase, setDraftBase] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [probed, setProbed] = useState<{ version?: string } | null>(null)

  const startAdd = () => {
    setEditing('new')
    setDraftName('')
    setDraftBase('')
    setError('')
    setProbed(null)
  }

  const startEdit = (id: string) => {
    const entry = servers.find(item => item.id === id)
    if (!entry) return
    setEditing(id)
    setDraftName(entry.name)
    setDraftBase(baseLabel(entry.base))
    setError('')
    setProbed(null)
  }

  const cancel = () => {
    setEditing(null)
    setError('')
    setProbed(null)
  }

  const save = async () => {
    const base = normalizeBase(draftBase)
    if (!base) {
      setError(t('地址格式不对。填「IP:端口」即可，例如 192.168.1.23:3065'))
      return
    }
    setBusy(true)
    setError('')
    const result = await probe(base)
    setBusy(false)
    if (!result.ok) {
      setError(t('连不上这台网关。确认地址与端口正确、网关已启动，且已开启「局域网访问」'))
      return
    }
    setProbed({ version: result.version })
    if (editing === 'new') {
      store.addServer({ name: draftName, base })
    } else if (editing) {
      store.updateServer(editing, { name: draftName, base })
    }
    setEditing(null)
    setProbed(null)
    store.toast(t('已保存'))
    // 换到新服务器后立刻拉一次会话；没登录会弹登录层
    void store.refresh(true)
  }

  return (
    <div className='m-page'>
      <PageHead
        title={t('服务器')}
        subtitle={servers.length ? t('已添加 {n} 台', { n: servers.length }) : t('先添加一台网关')}
        actions={
          <button type='button' className='m-btn m-sm' onClick={startAdd} disabled={editing === 'new'}>
            <Icon name='plus' />
            {t('添加')}
          </button>
        }
      />

      {error ? <ErrorNote text={error} /> : null}

      {/* 编辑表单：新增与修改共用 */}
      {editing ? (
        <div className='m-section' style={{ marginTop: 0 }}>
          <div className='m-card m-card-pad'>
            <div style={{ fontSize: 13, fontWeight: 600 }}>
              {editing === 'new' ? t('添加服务器') : t('编辑服务器')}
            </div>
            <label style={{ display: 'block', marginTop: 12 }}>
              <span style={{ fontSize: 12, color: 'var(--m-text-3)' }}>{t('地址')}</span>
              <input
                value={draftBase}
                onChange={event => setDraftBase(event.target.value)}
                placeholder='192.168.1.23:3065'
                inputMode='url'
                autoCapitalize='off'
                autoCorrect='off'
                spellCheck={false}
                style={{
                  width: '100%',
                  boxSizing: 'border-box',
                  marginTop: 5,
                  height: 44,
                  padding: '0 12px',
                  borderRadius: 11,
                  border: '1px solid var(--m-line-strong)',
                  background: 'var(--m-surface-2)',
                  color: 'var(--m-text)',
                  fontSize: 16,
                  fontFamily: 'var(--m-mono)',
                }}
              />
            </label>
            <div style={{ fontSize: 11.5, color: 'var(--m-text-3)', marginTop: 6, lineHeight: 1.6 }}>
              {t('只填「IP:端口」即可，不写端口默认 3065。地址里的 /m、/v1 会自动去掉。')}
            </div>
            <label style={{ display: 'block', marginTop: 12 }}>
              <span style={{ fontSize: 12, color: 'var(--m-text-3)' }}>{t('备注名（可选）')}</span>
              <input
                value={draftName}
                onChange={event => setDraftName(event.target.value)}
                placeholder={t('例如：家里、公司')}
                style={{
                  width: '100%',
                  boxSizing: 'border-box',
                  marginTop: 5,
                  height: 44,
                  padding: '0 12px',
                  borderRadius: 11,
                  border: '1px solid var(--m-line-strong)',
                  background: 'var(--m-surface-2)',
                  color: 'var(--m-text)',
                  fontSize: 16,
                }}
              />
            </label>
            <div className='m-btn-row' style={{ marginTop: 14 }}>
              <button type='button' className='m-btn m-ghost' onClick={cancel} disabled={busy}>
                {t('取消')}
              </button>
              <button type='button' className='m-btn' onClick={() => void save()} disabled={busy || !draftBase.trim()}>
                {busy ? t('正在连接…') : t('连接并保存')}
              </button>
            </div>
          </div>
        </div>
      ) : null}

      <div className='m-section'>
        <SectionHead title={t('已添加的服务器')} />
        {servers.length === 0 ? (
          <div className='m-card m-card-pad' style={{ fontSize: 12.5, color: 'var(--m-text-2)', lineHeight: 1.7 }}>
            {t('还没有添加服务器。点右上角「添加」，填入网关所在机器的局域网 IP 与端口。')}
          </div>
        ) : (
          <div className='m-card'>
            {servers.map(entry => {
              const current = server?.id === entry.id
              return (
                <div key={entry.id} className='m-row'>
                  <span
                    className='m-logo'
                    style={
                      current
                        ? { background: 'var(--m-primary-soft)', borderColor: 'transparent', color: 'var(--m-primary)' }
                        : undefined
                    }
                  >
                    <Icon name='server' />
                  </span>
                  <button
                    type='button'
                    className='m-grow'
                    onClick={() => {
                      store.switchServer(entry.id)
                      void store.refresh(true)
                    }}
                    style={{ border: 0, background: 'none', textAlign: 'left', padding: 0 }}
                  >
                    <div className='m-t1' style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                      {entry.name || baseLabel(entry.base)}
                      {current ? (
                        <span className='m-badge m-badge-brand' style={{ height: 18, fontSize: 10.5 }}>
                          {t('当前')}
                        </span>
                      ) : null}
                    </div>
                    <div className='m-t2 m-mono' style={{ fontSize: 11.5 }}>
                      {baseLabel(entry.base)}
                    </div>
                  </button>
                  <button
                    type='button'
                    className='m-icon-btn'
                    aria-label={t('编辑')}
                    onClick={() => startEdit(entry.id)}
                    style={{ width: 30, height: 30, boxShadow: 'none' }}
                  >
                    <Icon name='edit' />
                  </button>
                  <button
                    type='button'
                    className='m-icon-btn'
                    aria-label={t('删除')}
                    onClick={() => {
                      store.deleteServer(entry.id)
                      store.toast(t('已删除'))
                      void store.refresh(true)
                    }}
                    style={{ width: 30, height: 30, boxShadow: 'none', color: 'var(--m-danger)' }}
                  >
                    <Icon name='trash' />
                  </button>
                </div>
              )
            })}
          </div>
        )}
      </div>

      <div className='m-section' style={{ paddingBottom: 8 }}>
        <div className='m-card m-card-pad' style={{ fontSize: 11.5, color: 'var(--m-text-3)', lineHeight: 1.7 }}>
          {t('每台服务器的登录状态各自独立保存，切换后不用重新登录。地址改掉后需要重新登录。')}
        </div>
      </div>
    </div>
  )
}
