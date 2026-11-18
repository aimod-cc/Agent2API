import * as React from 'react'
import { Button, Input, Spinner } from '@ui'
import { t } from '../ui-kit/src/i18n'
import type { ProviderKeyDraft, ProviderKeyRow, ProviderKeyStatus, SearchProviderRow } from './tools-provider-keys'

export function providerKeyStatusText(status: ProviderKeyStatus): string {
  switch (status.status) {
    case 'quota_exhausted': return t('额度已用完')
    case 'rate_limited': return t('暂时限流')
    case 'invalid_provider_key': return t('密钥无效')
    default: return t('可用')
  }
}

function recoveryText(status: ProviderKeyStatus): string {
  if (status.status === 'ready' || status.status === 'invalid_provider_key') return ''
  return status.reset_at
    ? t('恢复时间：{time}', { time: new Date(status.reset_at).toLocaleString() })
    : t('上游未提供恢复时间')
}

function KeyStatus({ status, label }: { status: ProviderKeyStatus; label?: string }) {
  return <div className={status.status === 'ready' ? 'text-subtle' : 'text-warning'}>
    <span>{label ? `${label} · ` : ''}{providerKeyStatusText(status)}</span>
    {recoveryText(status) ? <div className='mt-0.5'>{recoveryText(status)}</div> : null}
  </div>
}

function KeyStates({ row }: { row: ProviderKeyRow }) {
  const same = row.search.status === row.fetch.status && row.search.reset_at === row.fetch.reset_at
    && row.search.scope === row.fetch.scope
  return same ? <KeyStatus status={row.search} /> : <>
    <KeyStatus label={t('搜索')} status={row.search} />
    <KeyStatus label={t('抓取')} status={row.fetch} />
  </>
}

export function ProviderKeyList({ provider }: { provider: SearchProviderRow }) {
  if (!provider.keys?.length) return <div>{t('密钥提示')}: {provider.key_hint || '-'}</div>
  return <div className='flex max-h-64 flex-col gap-2 overflow-y-auto'>
    <div>{t('可用密钥：{available} / {total}', { available: provider.available_key_count ?? 0, total: provider.keys.length })}</div>
    {provider.keys.map(row => <div key={row.id} className='rounded-md border border-border p-2'>
      <div className='flex flex-wrap items-center gap-x-2 gap-y-1'>
        <span className='font-medium text-foreground'>{t('优先级 {index}', { index: row.priority })}</span>
        <span className='break-all font-mono text-foreground'>{row.key_hint}</span>
      </div>
      {row.id === provider.active_key_id ? <div className='mt-0.5 text-primary'>{t('搜索优先使用')}</div> : null}
      {row.id === provider.active_fetch_key_id && row.id !== provider.active_key_id ? <div className='mt-0.5 text-primary'>{t('抓取优先使用')}</div> : null}
      <div className='mt-1'><KeyStates row={row} /></div>
    </div>)}
  </div>
}

type EditorProps = {
  value: ProviderKeyDraft[]
  onChange: (value: ProviderKeyDraft[]) => void
  disabled?: boolean
  testing?: string
  maxKeys?: number
  onTest: (id: string, operation: 'search' | 'fetch') => void
}

export function ProviderKeysEditor({ value, onChange, disabled, testing, maxKeys = 100, onTest }: EditorProps) {
  const nextId = React.useRef(0)
  const add = () => onChange([...value, { localId: `new-${nextId.current++}`, apiKey: '' }])
  const move = (index: number, offset: number) => {
    const next = [...value]
    const other = index + offset
    const item = next[index]
    next[index] = next[other]
    next[other] = item
    onChange(next)
  }
  return <div className='flex flex-col gap-3 text-xs'>
    <p className='text-subtle'>{t('按列表从上到下使用密钥。额度用完后自动切换；恢复时间到了，下次请求会重新优先使用排在前面的密钥。')}</p>
    <p className='text-subtle'>{t('同一账号下的密钥可能共享额度；备用密钥也必须有可用额度。')}</p>
    {value.map((draft, index) => <div key={draft.localId} className='flex flex-col gap-2 rounded-md border border-border p-3'>
      <div className='flex flex-wrap items-center gap-2'>
        <span className='font-medium'>{t('优先级 {index}', { index: index + 1 })}</span>
        {draft.row ? <span className='break-all font-mono text-subtle'>{draft.row.key_hint}</span> : null}
        <div className='ml-auto flex gap-1'>
          <Button size='xs' variant='ghost' disabled={disabled || index === 0} onClick={() => move(index, -1)} aria-label={t('上移密钥')}>{t('上移')}</Button>
          <Button size='xs' variant='ghost' disabled={disabled || index === value.length - 1} onClick={() => move(index, 1)} aria-label={t('下移密钥')}>{t('下移')}</Button>
          <Button size='xs' variant='ghost' disabled={disabled} onClick={() => onChange(value.filter((_, item) => item !== index))}>{t('移除')}</Button>
        </div>
      </div>
      <Input type='password' autoComplete='new-password' value={draft.apiKey} disabled={disabled}
        aria-label={t('优先级 {index} 的 API Key', { index: index + 1 })}
        placeholder={draft.id ? t('输入新密钥；留空保持不变') : t('输入 API Key')}
        onChange={event => onChange(value.map((item, itemIndex) => itemIndex === index ? { ...item, apiKey: event.currentTarget.value } : item))} />
      {draft.row && !draft.apiKey.trim() ? <>
        <KeyStates row={draft.row} />
        <div className='flex flex-wrap gap-2'>
          <Button size='sm' variant='outline' disabled={disabled || Boolean(testing)} onClick={() => onTest(draft.id!, 'search')}>
            {testing === `${draft.id}:search` ? <Spinner /> : null}{t('检查搜索')}
          </Button>
          {draft.row.fetch.status !== 'ready' && draft.row.fetch.scope !== 'all' ? <Button size='sm' variant='outline'
            disabled={disabled || Boolean(testing)} onClick={() => onTest(draft.id!, 'fetch')}>
            {testing === `${draft.id}:fetch` ? <Spinner /> : null}{t('检查抓取')}
          </Button> : null}
        </div>
      </> : null}
    </div>)}
    <div><Button size='sm' variant='dashed' disabled={disabled || value.length >= maxKeys} onClick={add}>{t('添加密钥')}</Button></div>
    <p className='text-subtle'>{t('已保存的密钥留空保持不变，调整顺序或移除后点击保存生效。')}</p>
    <p className='text-subtle'>{t('上游未提供恢复时间时不会自动重试。充值后可检查该密钥，检查成功会清除额度标记。检查会消耗少量额度。')}</p>
  </div>
}
