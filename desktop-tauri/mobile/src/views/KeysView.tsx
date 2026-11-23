import { useEffect, useMemo, useState } from 'react'
import { deleteKey, fetchKeys, updateKey, type GatewayKey, type KeysPayload } from '../core/api'
import { copyText, providerDisplayName, syncProviderLabels } from '../core/format'
import { t } from '../core/i18n'
import { serverIdentity } from '../core/servers'
import { store, useStore } from '../core/store'
import {
  CommonCard,
  CommonScaffold,
  CommonSwitch,
  EmptyBlock,
  RecordCard,
  SearchField,
  TonalChip,
  TonalIconButton,
} from '../ui/components'
import { GlyphIcon } from '../ui/icons'

export function KeysView() {
  const { server, revision, navTabs } = useStore()
  const [keysData, setKeysData] = useState<KeysPayload | null>(null)
  const [revealedIds, setRevealedIds] = useState<Record<string, boolean>>({})
  const [keyword, setKeyword] = useState('')

  const loadKeys = async (signal?: AbortSignal) => {
    const id = serverIdentity()
    if (!server?.base) return
    try {
      const res = await fetchKeys(signal)
      if (serverIdentity() !== id) return
      syncProviderLabels(res.providers)
      setKeysData(res)
    } catch {
      /* ignore */
    }
  }

  useEffect(() => {
    const ac = new AbortController()
    void loadKeys(ac.signal)
    return () => ac.abort()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [revision, server?.base])

  const allKeys = keysData?.keys || []
  const filteredKeys = useMemo(() => {
    const q = keyword.trim().toLowerCase()
    if (!q) return allKeys
    return allKeys.filter(k => {
      const name = (k.name || '').toLowerCase()
      const masked = (k.masked || k.key || '').toLowerCase()
      return name.includes(q) || masked.includes(q)
    })
  }, [allKeys, keyword])

  const handleToggleKey = async (item: GatewayKey, next: boolean) => {
    if (!item.id) return
    try {
      const updated = await updateKey(item.id, { enabled: next })
      setKeysData(updated)
      store.toast(next ? t('已启用 Key') : t('已停用 Key'), 'success')
    } catch (err) {
      store.toast(err instanceof Error ? err.message : t('更新 Key 失败'), 'danger')
    }
  }

  const handleDeleteKey = async (item: GatewayKey) => {
    if (!item.id) return
    try {
      const updated = await deleteKey(item.id)
      setKeysData(updated)
      store.toast(t('已删除 Key'), 'success')
    } catch (err) {
      store.toast(err instanceof Error ? err.message : t('删除 Key 失败'), 'danger')
    }
  }

  const stickyHeader =
    allKeys.length > 4 ? (
      <SearchField
        value={keyword}
        onChange={setKeyword}
        placeholder={t('搜索 Key 名称或内容…')}
      />
    ) : undefined

  return (
    <CommonScaffold
      title={t('Key')}
      badge={<TonalChip tone='primary'>{allKeys.length}</TonalChip>}
      onBack={navTabs.includes('keys') ? undefined : () => store.returnFromSubPage()}
      stickyHeader={stickyHeader}
      actions={
        <>
          <TonalIconButton
            icon='refresh'
            title={t('刷新')}
            onClick={() => void loadKeys()}
          />
          <TonalIconButton
            icon='plus'
            label={t('新建 Key')}
            primary
            onClick={() => store.openSheet({ kind: 'key-editor' })}
          />
        </>
      }
    >
      <CommonCard>
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 10 }}>
          <div>
            <div style={{ fontSize: 14, fontWeight: 700 }}>{t('网关接口鉴权')}</div>
            <div style={{ fontSize: 12, color: 'var(--fc-text-secondary)', marginTop: 2 }}>
              {keysData?.authRequired
                ? t('已开启鉴权，调用接口须携带已启用的 Key')
                : t('当前未强制校验密钥，创建并启用后可分发给客户端')}
            </div>
          </div>
          <TonalChip tone={keysData?.authRequired ? 'success' : 'neutral'}>
            {keysData?.authRequired ? t('需 Key') : t('开放')}
          </TonalChip>
        </div>
      </CommonCard>

      {filteredKeys.length === 0 ? (
        <EmptyBlock
          icon='key'
          title={allKeys.length === 0 ? t('还没有创建网关 API Key') : t('没有匹配的 Key')}
          description={t('支持限制每把 Key 允许访问的提供商或模型范围')}
          action={
            allKeys.length === 0 ? (
              <button
                type='button'
                className='fc-btn is-primary is-pill'
                onClick={() => store.openSheet({ kind: 'key-editor' })}
              >
                <GlyphIcon name='plus' size={16} />
                <span>{t('创建第一把 Key')}</span>
              </button>
            ) : null
          }
        />
      ) : (
        filteredKeys.map((item, idx) => {
          const id = item.id || `key-${idx}`
          const revealed = Boolean(revealedIds[id])
          const secret = item.key || item.masked || ''
          const displaySecret = revealed
            ? secret
            : item.masked || (secret ? `${secret.slice(0, 6)}••••${secret.slice(-4)}` : '••••')
          const provList = item.allowedProviders || []

          return (
            <RecordCard key={id}>
              <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 10 }}>
                <div style={{ minWidth: 0, flex: 1 }}>
                  <div style={{ fontSize: 15, fontWeight: 700 }}>{item.name || t('未命名 Key')}</div>
                  <div
                    className='fc-mono'
                    style={{
                      fontSize: 12,
                      color: 'var(--fc-text-secondary)',
                      marginTop: 2,
                      overflow: 'hidden',
                      textOverflow: 'ellipsis',
                      whiteSpace: 'nowrap',
                    }}
                  >
                    {displaySecret}
                  </div>
                </div>
                <CommonSwitch checked={item.enabled !== false} onChange={next => void handleToggleKey(item, next)} />
              </div>

              <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8, flexWrap: 'wrap' }}>
                <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6 }}>
                  {provList.length > 0 ? (
                    <TonalChip tone='primary'>
                      {provList.slice(0, 2).map(p => providerDisplayName(p)).join('、')}
                      {provList.length > 2 ? ` +${provList.length - 2}` : ''}
                    </TonalChip>
                  ) : (
                    <TonalChip>{t('全提供商可用')}</TonalChip>
                  )}
                  {(item.allowedModels?.length || 0) > 0 ? (
                    <TonalChip>{t('限 {n} 模型', { n: item.allowedModels?.length || 0 })}</TonalChip>
                  ) : null}
                </div>

                <div style={{ display: 'flex', gap: 6 }}>
                  {item.key ? (
                    <button
                      type='button'
                      className='fc-btn is-sm is-pill'
                      onClick={() => setRevealedIds(prev => ({ ...prev, [id]: !revealed }))}
                      aria-label={revealed ? t('隐藏密钥') : t('显示密钥')}
                    >
                      <GlyphIcon name={revealed ? 'eye-off' : 'eye'} size={14} />
                    </button>
                  ) : null}
                  {item.key ? (
                    <button
                      type='button'
                      className='fc-btn is-tonal is-sm is-pill'
                      onClick={async () => {
                        const ok = await copyText(item.key || '')
                        store.toast(ok ? t('已复制 API Key') : t('复制失败'), ok ? 'success' : 'danger')
                      }}
                    >
                      <GlyphIcon name='copy' size={13} />
                      <span>{t('复制')}</span>
                    </button>
                  ) : null}
                  <button
                    type='button'
                    className='fc-btn is-sm is-pill'
                    onClick={() => store.openSheet({ kind: 'key-editor', item })}
                    aria-label={t('编辑')}
                  >
                    <GlyphIcon name='edit' size={14} />
                  </button>
                  <button
                    type='button'
                    className='fc-btn is-danger is-sm is-pill'
                    onClick={() => void handleDeleteKey(item)}
                    aria-label={t('删除')}
                  >
                    <GlyphIcon name='trash' size={14} />
                  </button>
                </div>
              </div>
            </RecordCard>
          )
        })
      )}
    </CommonScaffold>
  )
}
