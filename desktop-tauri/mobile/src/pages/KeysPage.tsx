import { useState } from 'react'
import { PageIntro, IconButton, Empty, ErrorNote, SkeletonList, copyWithToast } from '../components/ui'
import { GlyphLogo } from '../components/ProviderLogo'
import { Icon } from '../components/Icon'
import { store } from '../state/store'
import { deleteKey, fetchKeys, setKeyEnabled, type GatewayKey, type KeysPayload } from '../api/gateway'
import { providerLabel } from '../lib/providers'
import { usePolling } from '../lib/usePolling'
import { t } from '../i18n'

/**
 * 网关 Key。
 *
 * Key 卡直接展示脱敏后的值与复制按钮 —— 这是这一页最高频的动作（复制给
 * 某个客户端）。明文只在展开后才取，默认只显示掩码，避免肩窥。
 *
 * 可用范围用一行摘要，完整编辑器在桌面端：白名单是多选，窄屏上做多选
 * 既费事又容易点错。
 */
export function KeysPage() {
  const [data, setData] = useState<KeysPayload | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState('')
  const [revealed, setRevealed] = useState<Set<string>>(new Set())

  const load = async () => {
    try {
      const next = await fetchKeys()
      setData(next)
      setError('')
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
    }
  }

  usePolling(load, 120_000)

  const keys = data?.keys || []
  const enabled = keys.filter(item => item.enabled).length

  const toggle = async (key: GatewayKey, next: boolean) => {
    if (!key.id) return
    setBusy(key.id)
    try {
      const payload = await setKeyEnabled(key.id, next)
      setData(payload)
      store.toast(next ? t('已启用「{name}」', { name: key.name || key.id }) : t('已禁用「{name}」', { name: key.name || key.id }))
    } catch (failure) {
      store.toast(failure instanceof Error ? failure.message : String(failure), 'err')
    } finally {
      setBusy('')
    }
  }

  const remove = async (key: GatewayKey) => {
    if (!key.id) return
    if (!window.confirm(t('确定删除「{name}」？用这把 Key 的客户端会立即失效。', { name: key.name || key.id }))) return
    setBusy(key.id)
    try {
      const payload = await deleteKey(key.id)
      setData(payload)
      store.toast(t('已删除'))
    } catch (failure) {
      store.toast(failure instanceof Error ? failure.message : String(failure), 'err')
    } finally {
      setBusy('')
    }
  }

  const toggleReveal = (id: string) => {
    const next = new Set(revealed)
    if (next.has(id)) next.delete(id)
    else next.add(id)
    setRevealed(next)
  }

  return (
    <div className='m-page'>
      <PageIntro actions={<IconButton name='refresh' label={t('刷新')} onClick={() => void load()} />}
      />

      <div className='m-section' style={{ marginTop: 0 }}>
        <div
          className='m-card m-card-pad'
          style={{ background: 'var(--m-primary-soft)', borderColor: 'var(--m-primary-line)' }}
        >
          <div style={{ fontSize: 12.5, lineHeight: 1.6, color: 'var(--m-text-2)' }}>
            {t('客户端请求带上 ')}
            <span className='m-mono' style={{ fontSize: 11.5, color: 'var(--m-primary)' }}>
              Authorization: Bearer &lt;Key&gt;
            </span>
            {data?.authRequired ? t('；当前必须带 Key。') : t('；一把启用的 Key 都没有时不鉴权。')}
          </div>
        </div>
      </div>

      {error ? <ErrorNote text={error} /> : null}
      {!data ? <SkeletonList rows={3} height={120} /> : null}

      {data && !keys.length ? (
        <div className='m-section'>
          <div className='m-card'>
            <Empty text={t('还没有 Key。去桌面端「网关 Key」页新建一把。')} />
          </div>
        </div>
      ) : null}

      {keys.map(key => {
        const id = String(key.id || '')
        const shown = revealed.has(id)
        const value = shown ? key.key || '' : key.masked || maskKey(key.key)
        return (
          <div key={id} className='m-section' style={{ marginTop: 10 }}>
            <div className='m-card m-card-pad'>
              <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
                <GlyphLogo name='key' />
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div style={{ fontSize: 14, fontWeight: 600, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                    {key.name || t('未命名')}
                  </div>
                  <div style={{ fontSize: 11.5, color: 'var(--m-text-3)', marginTop: 2 }}>
                    {scopeText(key)}
                  </div>
                </div>
                <SwitchSmall
                  checked={Boolean(key.enabled)}
                  disabled={busy === id}
                  label={key.name || id}
                  onChange={next => void toggle(key, next)}
                />
              </div>

              <div
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  gap: 8,
                  marginTop: 10,
                  padding: '8px 10px',
                  borderRadius: 9,
                  background: 'var(--m-surface-2)',
                  border: '1px solid var(--m-line)',
                }}
              >
                <span
                  className='m-mono'
                  style={{ flex: 1, minWidth: 0, fontSize: 11.5, color: 'var(--m-text-2)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}
                >
                  {value}
                </span>
                <button
                  type='button'
                  onClick={() => toggleReveal(id)}
                  aria-label={shown ? t('隐藏') : t('显示')}
                  style={{ display: 'flex', border: 0, background: 'none', color: 'var(--m-text-3)', padding: 2 }}
                >
                  <Icon name='eye' style={{ width: 15, height: 15, opacity: shown ? 1 : 0.55 }} />
                </button>
                <button
                  type='button'
                  onClick={() => void copyWithToast(key.key || '', key.name || t('网关 Key'))}
                  aria-label={t('复制')}
                  style={{ display: 'flex', border: 0, background: 'none', color: 'var(--m-primary)', padding: 2 }}
                >
                  <Icon name='copy' style={{ width: 15, height: 15 }} />
                </button>
              </div>

              <div style={{ display: 'flex', alignItems: 'center', gap: 12, marginTop: 9, fontSize: 11.5, color: 'var(--m-text-3)' }}>
                <span>{key.enabled ? t('已启用') : t('已停用')}</span>
                <span>{t('创建于 {time}', { time: key.createdAt ? new Date(key.createdAt).toLocaleDateString('zh-CN') : '—' })}</span>
                <button
                  type='button'
                  onClick={() => void remove(key)}
                  disabled={busy === id}
                  style={{
                    marginLeft: 'auto',
                    border: 0,
                    background: 'none',
                    color: 'var(--m-danger)',
                    fontSize: 11.5,
                    padding: 0,
                    display: 'inline-flex',
                    alignItems: 'center',
                    gap: 4,
                  }}
                >
                  <Icon name='trash' style={{ width: 13, height: 13 }} />
                  {t('删除')}
                </button>
              </div>
            </div>
          </div>
        )
      })}

      <div className='m-section' style={{ paddingBottom: 8 }}>
        <div className='m-card m-card-pad' style={{ fontSize: 11.5, color: 'var(--m-text-3)', lineHeight: 1.7 }}>
          {t('新建 Key 与设置可用范围（提供商 / 模型白名单）在桌面端完成。')}
        </div>
      </div>
    </div>
  )
}

function scopeText(key: GatewayKey): string {
  const providers = key.allowedProviders || []
  const models = key.allowedModels || []
  if (!providers.length && !models.length) return t('不限制提供商与模型')
  const parts: string[] = []
  if (providers.length) parts.push(providers.map(id => providerLabel(id)).join('、'))
  if (models.length) parts.push(t('{n} 个模型', { n: models.length }))
  return t('限制：{text}', { text: parts.join(' / ') })
}

function maskKey(key?: string): string {
  if (!key) return '—'
  if (key.length <= 10) return '••••••'
  return `${key.slice(0, 7)}••••••${key.slice(-4)}`
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

