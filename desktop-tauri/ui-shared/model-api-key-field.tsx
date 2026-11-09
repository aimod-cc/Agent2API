import { Alert, Button, Label } from '@ui'
import { SearchableSelect } from './searchable-select'
import { modelApiKeyLabel, type ModelApiKey } from './model-api-key'

type Props = {
  keys: ModelApiKey[]
  value: string | null
  ready: boolean
  loading: boolean
  error: string
  disabled?: boolean
  rememberSelection?: boolean
  id?: string
  t: (text: string) => string
  onChange: (id: string) => void
  onRefresh: () => void
}

export function ModelApiKeyField({ keys, value, ready, loading, error, disabled, rememberSelection, id = 'model-api-key', t, onChange, onRefresh }: Props) {
  const selected = keys.find(key => key.id === value)
  const label = selected ? modelApiKeyLabel(selected)
    : value ? t('所选 API Key 已不可用') : t('选择 API Key…')
  const warning = error || (!ready ? t('正在读取 API Key…')
    : !keys.length ? t('没有可用的 API Key，请先在「网关 Key」页创建并启用')
    : value && !selected ? t('所选 API Key 已不可用，请重新选择') : '')
  return (
    <div className='flex flex-col gap-1.5'>
      <div className='flex items-center gap-2'>
        <Label htmlFor={id}>API Key</Label>
        <Button variant='ghost' size='xs' className='ml-auto' disabled={disabled || loading} onClick={onRefresh}>{t('刷新')}</Button>
      </div>
      <SearchableSelect value={selected?.id ?? ''} disabled={disabled || !ready || !keys.length}
        onValueChange={next => { if (next) onChange(next) }} id={id} className='w-full' displayValue={label}
        options={keys.map(key => ({ value: key.id, label: modelApiKeyLabel(key) }))} />
      <p className='text-xs leading-[1.6] text-subtle'>{t('模型调用使用所选 Key 的权限，日志记录 Key 名称，不保存密钥值。')}</p>
      {rememberSelection ? <p className='text-xs leading-[1.6] text-subtle'>{t('自动记住本次选择，下次打开继续使用；Key 被停用或删除后需重新选择。')}</p> : null}
      {warning ? <Alert variant='warning' role='status'>{warning}</Alert> : null}
    </div>
  )
}
