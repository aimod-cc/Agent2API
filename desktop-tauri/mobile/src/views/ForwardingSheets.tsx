import { useEffect, useState } from 'react'
import {
  createKey,
  fetchKeys,
  fetchManageModels,
  removeModelMapping,
  saveModelMapping,
  testModelOnline,
  updateKey,
  type GatewayKey,
  type ManageMapping,
  type ManageModel,
  type ModelTestResponse,
  type ProviderSummary,
} from '../core/api'
import { formatDuration, providerDisplayName } from '../core/format'
import { store } from '../core/store'
import {
  BottomSheet,
  CommonCard,
  ConnectedButtonGroup,
  EmptyBlock,
  RecordCard,
  SearchField,
  SectionHeader,
  TonalChip,
} from '../ui/components'
import { GlyphIcon, ProviderAvatar } from '../ui/icons'
import { PickerSheet, type PickerOption } from '../ui/pickers'

type ProtocolType = 'chat_completions' | 'responses' | 'anthropic'

const PROTOCOL_OPTIONS: ReadonlyArray<{ id: ProtocolType; label: string }> = [
  { id: 'chat_completions', label: 'OpenAI Chat' },
  { id: 'responses', label: 'Responses' },
  { id: 'anthropic', label: 'Anthropic' },
]

/* ─── 1. 在线模型测试抽屉 ────────────────────────────────── */

export function ModelTestSheet({
  model,
  onClose,
}: {
  model: ManageModel
  onClose: () => void
}) {
  const modelId = model.id || model.name || ''
  const provider = model.provider || ''
  const [protocol, setProtocol] = useState<ProtocolType>('chat_completions')
  const [prompt, setPrompt] = useState('你好，请用一句话回答1+1等于几？')
  const [busy, setBusy] = useState(false)
  const [result, setResult] = useState<ModelTestResponse | null>(null)

  const handleRunTest = async () => {
    if (!modelId || !provider || busy) return
    setBusy(true)
    setResult(null)
    try {
      const res = await testModelOnline({
        provider,
        model: modelId,
        prompt: prompt.trim() || '你好',
        protocol,
      })
      setResult(res)
    } catch (err) {
      setResult({
        ok: false,
        error: err instanceof Error ? err.message : '请求失败',
      })
    } finally {
      setBusy(false)
    }
  }

  const succeeded = Boolean(result && (result.ok || result.success || result.reply || result.content))

  return (
    <BottomSheet open title='在线模型连通测试' onClose={onClose}>
      <CommonCard>
        <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
          <ProviderAvatar provider={provider} label={model.providerLabel} size={42} />
          <div style={{ minWidth: 0, flex: 1 }}>
            <div
              className='fc-mono'
              style={{
                fontSize: 14.5,
                fontWeight: 700,
                overflow: 'hidden',
                textOverflow: 'ellipsis',
                whiteSpace: 'nowrap',
              }}
            >
              {modelId}
            </div>
            <div style={{ fontSize: 12.5, color: 'var(--fc-text-secondary)' }}>
              {providerDisplayName(provider, model.providerLabel)}
            </div>
          </div>
        </div>

        <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
          <span style={{ fontSize: 12, fontWeight: 600, color: 'var(--fc-text-secondary)' }}>
            调用协议
          </span>
          <ConnectedButtonGroup
            inCard
            options={PROTOCOL_OPTIONS}
            value={protocol}
            onChange={setProtocol}
          />
        </div>

        <textarea
          className='fc-textarea'
          rows={2}
          value={prompt}
          onChange={e => setPrompt(e.target.value)}
          placeholder='输入测试提示词…'
        />

        <button
          type='button'
          className='fc-btn is-primary'
          disabled={busy}
          onClick={handleRunTest}
        >
          <GlyphIcon name='bolt' size={16} />
          <span>{busy ? '正在请求上游模型…' : '发送测试请求'}</span>
        </button>
      </CommonCard>

      {result ? (
        <CommonCard>
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8 }}>
            <span style={{ fontSize: 14, fontWeight: 700 }}>测试结果</span>
            <div style={{ display: 'flex', gap: 6 }}>
              {result.durationMs !== undefined ? (
                <TonalChip>{formatDuration(result.durationMs)}</TonalChip>
              ) : null}
              <TonalChip tone={succeeded ? 'success' : 'danger'}>
                {succeeded ? '连通正常' : '调用失败'}
              </TonalChip>
            </div>
          </div>

          {result.accountName || result.accountId ? (
            <div style={{ fontSize: 12, color: 'var(--fc-text-secondary)' }}>
              命中账号：{result.accountName || result.accountId}
            </div>
          ) : null}

          {result.error ? (
            <div
              className='fc-inner-block'
              style={{ color: 'var(--fc-danger)', fontSize: 13, wordBreak: 'break-word' }}
            >
              {result.error}
            </div>
          ) : null}

          {result.reasoning ? (
            <div
              className='fc-inner-block'
              style={{
                fontSize: 12.5,
                color: 'var(--fc-text-secondary)',
                whiteSpace: 'pre-wrap',
                wordBreak: 'break-word',
              }}
            >
              思考过程：{result.reasoning}
            </div>
          ) : null}

          {result.reply || result.content ? (
            <div
              className='fc-inner-block'
              style={{
                fontSize: 13.5,
                color: 'var(--fc-text)',
                whiteSpace: 'pre-wrap',
                wordBreak: 'break-word',
              }}
            >
              {result.reply || result.content}
            </div>
          ) : null}
        </CommonCard>
      ) : null}
    </BottomSheet>
  )
}

/* ─── 2. 模型别名映射抽屉 ────────────────────────────────── */

export function ModelMappingSheet({ onClose }: { onClose: () => void }) {
  const [mappings, setMappings] = useState<ManageMapping[]>([])
  const [models, setModels] = useState<ManageModel[]>([])
  const [alias, setAlias] = useState('')
  const [target, setTarget] = useState('')
  const [mapSearch, setMapSearch] = useState('')
  const [targetPickerOpen, setTargetPickerOpen] = useState(false)
  const [busy, setBusy] = useState(false)

  const reload = async () => {
    try {
      const view = await fetchManageModels()
      setMappings(view.mappings || [])
      setModels(view.models || [])
      if (!target && view.models?.[0]?.id) {
        setTarget(view.models[0].id)
      }
    } catch {
      /* ignore */
    }
  }

  useEffect(() => {
    void reload()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const handleAdd = async () => {
    if (!alias.trim() || !target.trim() || busy) return
    setBusy(true)
    try {
      const matchedModel = models.find(m => (m.id || m.name) === target.trim())
      const updated = await saveModelMapping({
        alias: alias.trim(),
        target: target.trim(),
        provider: matchedModel?.provider,
        enabled: true,
        create: true,
      })
      setMappings(updated.mappings || [])
      setAlias('')
      store.toast('已添加模型别名映射', 'success')
      void store.refresh(true)
    } catch (err) {
      store.toast(err instanceof Error ? err.message : '保存映射失败', 'danger')
    } finally {
      setBusy(false)
    }
  }

  const handleRemove = async (m: ManageMapping) => {
    if (!m.alias || !m.target || busy) return
    setBusy(true)
    try {
      const updated = await removeModelMapping({
        alias: m.alias,
        target: m.target,
        provider: m.provider,
      })
      setMappings(updated.mappings || [])
      store.toast('已移除映射', 'success')
      void store.refresh(true)
    } catch (err) {
      store.toast(err instanceof Error ? err.message : '移除映射失败', 'danger')
    } finally {
      setBusy(false)
    }
  }

  const filteredMappings = mappings.filter(m => {
    const q = mapSearch.trim().toLowerCase()
    if (!q) return true
    return (m.alias || '').toLowerCase().includes(q) || (m.target || '').toLowerCase().includes(q)
  })

  const modelOptions: PickerOption[] = models
    .map(m => {
      const id = m.id || m.name || ''
      return {
        value: id,
        label: id,
        subtitle: providerDisplayName(m.provider, m.providerLabel),
      }
    })
    .filter(item => Boolean(item.value))

  return (
    <BottomSheet
      open
      title='模型别名映射'
      onClose={onClose}
      fixedHeader={
        mappings.length > 0 ? (
          <SearchField
            inSheet
            value={mapSearch}
            onChange={setMapSearch}
            placeholder='搜索别名或目标模型…'
          />
        ) : undefined
      }
    >
      <CommonCard>
        <div style={{ fontSize: 13.5, fontWeight: 700 }}>新增映射规则</div>
        <input
          type='text'
          className='fc-input'
          value={alias}
          onChange={e => setAlias(e.target.value)}
          placeholder='客户端请求的模型名（例如 gpt-4o）'
        />
        <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
          <input
            type='text'
            className='fc-input'
            style={{ flex: 1 }}
            value={target}
            onChange={e => setTarget(e.target.value)}
            placeholder='转发到的目标上游模型 ID'
          />
          {modelOptions.length > 0 ? (
            <button
              type='button'
              className='fc-btn is-tonal is-sm'
              onClick={() => setTargetPickerOpen(true)}
            >
              选择
            </button>
          ) : null}
        </div>
        <button
          type='button'
          className='fc-btn is-primary'
          disabled={!alias.trim() || !target.trim() || busy}
          onClick={handleAdd}
        >
          <GlyphIcon name='plus' size={16} />
          <span>添加映射</span>
        </button>
      </CommonCard>

      <SectionHeader title={`当前映射 (${filteredMappings.length})`} />

      {filteredMappings.length === 0 ? (
        <EmptyBlock
          icon='route'
          title={mappings.length === 0 ? '暂无自定义模型映射' : '没有匹配的映射规则'}
          description='通过别名映射可将外部工具固定的模型名无缝指向任意可用上游模型'
        />
      ) : (
        filteredMappings.map((m, idx) => (
          <RecordCard key={`${m.alias}-${m.target}-${idx}`}>
            <div
              style={{
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'space-between',
                gap: 10,
              }}
            >
              <div style={{ minWidth: 0 }}>
                <div className='fc-mono' style={{ fontSize: 14, fontWeight: 700 }}>
                  {m.alias}
                </div>
                <div
                  className='fc-mono'
                  style={{ fontSize: 12, color: 'var(--fc-text-secondary)', marginTop: 2 }}
                >
                  → {m.target}
                  {m.provider ? ` (${providerDisplayName(m.provider)})` : ''}
                </div>
              </div>
              <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                {m.dangling ? <TonalChip tone='warning'>目标未挂载</TonalChip> : null}
                <button
                  type='button'
                  className='fc-btn is-danger is-sm is-pill'
                  disabled={busy}
                  onClick={() => void handleRemove(m)}
                >
                  <GlyphIcon name='trash' size={14} />
                </button>
              </div>
            </div>
          </RecordCard>
        ))
      )}
      <PickerSheet
        open={targetPickerOpen}
        title='选择目标模型'
        searchable
        searchPlaceholder='搜索模型 ID 或提供商'
        options={modelOptions}
        value={target}
        onSelect={setTarget}
        onClose={() => setTargetPickerOpen(false)}
      />
    </BottomSheet>
  )
}

/* ─── 3. 网关 API Key 编辑 / 新建抽屉 ───────────────────── */

export function KeyEditorSheet({
  item,
  onClose,
}: {
  item?: GatewayKey
  onClose: () => void
}) {
  const isEdit = Boolean(item?.id)
  const [name, setName] = useState(item?.name || '')
  const [customKey, setCustomKey] = useState('')
  const [allowedProviders, setAllowedProviders] = useState<string[]>(
    () => item?.allowedProviders || [],
  )
  const [providers, setProviders] = useState<ProviderSummary[]>([])
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    fetchKeys()
      .then(res => setProviders(res.providers || []))
      .catch(() => {})
  }, [])

  const toggleProvider = (id: string) => {
    setAllowedProviders(prev =>
      prev.includes(id) ? prev.filter(p => p !== id) : [...prev, id],
    )
  }

  const handleSave = async () => {
    if (!name.trim() || busy) return
    setBusy(true)
    try {
      if (isEdit && item?.id) {
        await updateKey(item.id, {
          name: name.trim(),
          allowedProviders,
        })
        store.toast('已更新 API Key', 'success')
      } else {
        await createKey({
          name: name.trim(),
          key: customKey.trim() || undefined,
          allowedProviders,
          allowedModels: [],
        })
        store.toast('已创建 API Key', 'success')
      }
      void store.refresh(true)
      onClose()
    } catch (err) {
      store.toast(err instanceof Error ? err.message : '保存失败', 'danger')
    } finally {
      setBusy(false)
    }
  }

  return (
    <BottomSheet
      open
      title={isEdit ? '编辑 API Key' : '新建 API Key'}
      onClose={onClose}
      footer={
        <button
          type='button'
          className='fc-btn is-primary'
          style={{ width: '100%' }}
          disabled={!name.trim() || busy}
          onClick={handleSave}
        >
          {busy ? '保存中…' : isEdit ? '保存更改' : '立即创建'}
        </button>
      }
    >
      <CommonCard>
        <div style={{ fontSize: 13, fontWeight: 700 }}>基本信息</div>
        <input
          type='text'
          className='fc-input'
          value={name}
          onChange={e => setName(e.target.value)}
          placeholder='名称（如 Cursor 专用、手机端调用）'
        />
        {!isEdit ? (
          <input
            type='text'
            className='fc-input fc-mono'
            value={customKey}
            onChange={e => setCustomKey(e.target.value)}
            placeholder='自定义密钥串（留空则自动随机生成）'
          />
        ) : null}
      </CommonCard>

      {providers.length > 0 ? (
        <CommonCard>
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
            <span style={{ fontSize: 13, fontWeight: 700 }}>可用提供商范围</span>
            {allowedProviders.length > 0 ? (
              <button
                type='button'
                style={{
                  border: 'none',
                  background: 'transparent',
                  color: 'var(--fc-primary)',
                  fontSize: 12,
                  fontWeight: 600,
                  cursor: 'pointer',
                }}
                onClick={() => setAllowedProviders([])}
              >
                重置为全部
              </button>
            ) : (
              <TonalChip tone='success'>不限（允许全部）</TonalChip>
            )}
          </div>
          <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8 }}>
            {providers.map(p => {
              const id = p.id || ''
              if (!id) return null
              const active = allowedProviders.includes(id)
              return (
                <button
                  key={id}
                  type='button'
                  className={`fc-btn is-sm is-pill ${active ? 'is-primary' : ''}`}
                  onClick={() => toggleProvider(id)}
                >
                  {providerDisplayName(id, p.label)}
                </button>
              )
            })}
          </div>
        </CommonCard>
      ) : null}
    </BottomSheet>
  )
}
