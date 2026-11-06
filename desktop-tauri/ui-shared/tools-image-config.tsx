import * as React from 'react'
import {
  Alert, Button, Card, CardContent, CardHeader, CardTitle, Field, FieldDescription, FieldLabel,
  Input, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, Spinner, Switch,
} from '@ui'
import { t } from '../ui-kit/src/i18n'
import { emptyImageTarget, type ImageConfigView, type ImageGenerationConfig, type ImageTarget, type ImageToolApi } from './tools-image-types'
import { ToolImageHistory } from './tools-image-history'

function TargetFields({ target, view, editing, onChange, disabled }: {
  target: ImageTarget; view: ImageConfigView; editing: boolean; onChange: (target: ImageTarget) => void; disabled: boolean
}) {
  const supported = view.models.filter(model => editing ? model.editing : model.generation)
  const providers = [...new Map(supported.map(model => [model.provider, model.provider_label || model.provider])).entries()]
  const models = supported.filter(model => model.provider === target.provider)
  const accounts = view.accounts.filter(account => account.provider === target.provider && account.enabled)
  const accountLabel = accounts.find(account => account.id === target.account_id)?.name
    || (target.account_id ? t('已绑定账号不可用，请重新选择') : t('自动选择账号'))
  return <div className='grid grid-cols-1 gap-3 md:grid-cols-3'>
    <Field>
      <FieldLabel>{t('上游提供商')}</FieldLabel>
      <Select value={target.provider} onValueChange={value => { if (typeof value === 'string') onChange({ provider: value, model: '', account_id: '' }) }}>
        <SelectTrigger className='w-full' disabled={disabled}><SelectValue>{providers.find(([id]) => id === target.provider)?.[1] || target.provider || t('请选择提供商')}</SelectValue></SelectTrigger>
        <SelectContent>
          {target.provider && !providers.some(([id]) => id === target.provider) ? <SelectItem value={target.provider} disabled>{target.provider}（{t('已失效')}）</SelectItem> : null}
          {providers.map(([id, label]) => <SelectItem key={id} value={id}>{label}</SelectItem>)}
        </SelectContent>
      </Select>
    </Field>
    <Field>
      <FieldLabel>{editing ? t('编辑模型') : t('生图模型')}</FieldLabel>
      <Select value={target.model} onValueChange={value => { if (typeof value === 'string') onChange({ ...target, model: value }) }}>
        <SelectTrigger className='w-full' disabled={disabled || !target.provider}><SelectValue>{target.model || t('请选择模型')}</SelectValue></SelectTrigger>
        <SelectContent>
          {target.model && !models.some(model => model.id === target.model) ? <SelectItem value={target.model} disabled>{target.model}（{t('已失效')}）</SelectItem> : null}
          {models.map(model => <SelectItem key={model.id} value={model.id}>{model.id}</SelectItem>)}
        </SelectContent>
      </Select>
    </Field>
    <Field>
      <FieldLabel>{t('生图账号')}</FieldLabel>
      <Select value={target.account_id || '__auto__'} onValueChange={value => { if (typeof value === 'string') onChange({ ...target, account_id: value === '__auto__' ? '' : value }) }}>
        <SelectTrigger className='w-full' disabled={disabled || !target.provider}><SelectValue>{accountLabel}</SelectValue></SelectTrigger>
        <SelectContent>
          <SelectItem value='__auto__'>{t('自动选择账号')}</SelectItem>
          {target.account_id && !accounts.some(account => account.id === target.account_id) ? <SelectItem value={target.account_id} disabled>{accountLabel}</SelectItem> : null}
          {accounts.map(account => <SelectItem key={account.id} value={account.id}>{account.name}</SelectItem>)}
        </SelectContent>
      </Select>
    </Field>
  </div>
}

export function ImageGenerationPanel({ api, onChanged, showTimeout = false }: {
  api: ImageToolApi; onChanged?: () => void; showTimeout?: boolean
}) {
  const [view, setView] = React.useState<ImageConfigView | null>(null)
  const [draft, setDraft] = React.useState<ImageGenerationConfig | null>(null)
  const [loading, setLoading] = React.useState(true)
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')
  const [notice, setNotice] = React.useState('')
  const [timeout, setTimeoutValue] = React.useState(300)

  React.useEffect(() => {
    let alive = true
    api.load().then(result => {
      if (alive) { setView(result); setDraft(result.config || result.defaults); setTimeoutValue(result.timeout_seconds) }
    }).catch(failure => { if (alive) setError(t(failure instanceof Error ? failure.message : String(failure))) })
      .finally(() => { if (alive) setLoading(false) })
    return () => { alive = false }
  }, [api])

  const save = async () => {
    if (!draft) return
    if (showTimeout && (!Number.isInteger(timeout) || timeout < 5 || timeout > 600)) {
      setError(t('请求参数无效')); return
    }
    setBusy(true); setError(''); setNotice('')
    try {
      const next = await api.save(draft)
      setView(next); setDraft(next.config || next.defaults)
      onChanged?.()
      if (showTimeout && api.saveTimeout && timeout !== next.timeout_seconds) {
        await api.saveTimeout(timeout)
        setView(current => current ? { ...current, timeout_seconds: timeout } : current)
        onChanged?.()
      }
      setNotice(t('图片生成配置已保存'))
    } catch (failure) { setError(t(failure instanceof Error ? failure.message : String(failure))) }
    finally { setBusy(false) }
  }
  const check = async () => {
    setBusy(true); setError(''); setNotice('')
    try {
      const next = await api.load()
      setView(next)
      if (!draft) {
        setDraft(next.config || next.defaults)
        setTimeoutValue(next.timeout_seconds)
      }
      setNotice(t('已检查保存的配置，未发起生图；账号实际额度与上游可用性以调用结果为准。'))
    } catch (failure) { setError(t(failure instanceof Error ? failure.message : String(failure))) }
    finally { setBusy(false) }
  }
  const setNumber = (key: 'max_images' | 'max_concurrency' | 'retention_hours' | 'storage_mb', value: string) => {
    setDraft(current => current ? { ...current, [key]: Number(value) } : current)
  }

  if (loading) return <div className='flex items-center gap-2 p-4 text-sm text-subtle'><Spinner />{t('正在加载工具服务配置…')}</div>
  return <div className='flex flex-col gap-4'>
    <Card>
      <CardHeader>
        <CardTitle>{t('图片生成配置')}</CardTitle>
        <Button className='ml-auto' variant='outline' size='sm' disabled={busy} onClick={() => void check()}>{busy ? <Spinner /> : null}{t('检查配置')}</Button>
      </CardHeader>
      <CardContent className='flex flex-col gap-4'>
        <p className='text-xs text-subtle'>{t('复用已有账号生图，不改变 Agent 的对话模型。实际调用遵守接入 Key 的模型和提供商权限。')}</p>
        {!view?.config ? <Alert variant='warning'>{t('尚未配置图片生成，请先选择可用的生图模型并保存。')}</Alert> : null}
        {view?.generation_error ? <Alert variant='warning'>{t(view.generation_error)}</Alert> : null}
        {view?.editing_error ? <Alert variant='warning'>{t(view.editing_error)}</Alert> : null}
        {error ? <Alert variant='destructive'>{error}</Alert> : null}
        {notice ? <Alert variant='success'>{notice}</Alert> : null}
        {draft && view ? <>
          <TargetFields target={draft.generation} view={view} editing={false} disabled={busy}
            onChange={generation => setDraft(current => current ? { ...current, generation } : current)} />
          <p className='text-xs text-subtle'>{t('自动选择仅在所选提供商内遵守现有路由；固定账号仍受停用、冷却与额度限制，不会自动换号。')}</p>
          {!view.models.some(model => model.generation) ? <Alert variant='warning'>{t('没有可用的生图模型，请先添加账号、启用对应插件并更新模型目录。')}</Alert> : null}
          <div className='flex items-center gap-2'>
            <span className='text-sm font-medium'>{t('参考图编辑')}</span>
            <Switch checked={Boolean(draft.editing)} disabled={busy} onCheckedChange={checked => setDraft(current => current
              ? { ...current, editing: checked ? emptyImageTarget() : null } : current)} aria-label={t('参考图编辑')} />
          </div>
          {draft.editing ? <TargetFields target={draft.editing} view={view} editing disabled={busy}
            onChange={editing => setDraft(current => current ? { ...current, editing } : current)} /> : null}
          <p className='m-0 text-[11.5px] leading-relaxed text-subtle'>{t('编辑可使用不同模型；未配置可用编辑模型时，不向 Agent 提供编辑工具。')}</p>
          <div className='grid grid-cols-2 gap-3 md:grid-cols-4'>
            <Field><FieldLabel>{t('单次图片上限')}</FieldLabel><Input type='number' min={1} max={10} step={1} value={draft.max_images} disabled={busy} onChange={event => setNumber('max_images', event.currentTarget.value)} /></Field>
            <Field><FieldLabel>{t('最大生成并发')}</FieldLabel><Input type='number' min={1} max={8} step={1} value={draft.max_concurrency} disabled={busy} onChange={event => setNumber('max_concurrency', event.currentTarget.value)} /></Field>
            <Field><FieldLabel>{t('结果保留（小时）')}</FieldLabel><Input type='number' min={1} max={168} step={1} value={draft.retention_hours} disabled={busy} onChange={event => setNumber('retention_hours', event.currentTarget.value)} /></Field>
            <Field><FieldLabel>{t('存储上限（MiB）')}</FieldLabel><Input type='number' min={64} max={4096} step={1} value={draft.storage_mb} disabled={busy} onChange={event => setNumber('storage_mb', event.currentTarget.value)} /></Field>
          </div>
          <p className='text-xs text-subtle'>{t('默认只生成一张。空间不足时先拒绝请求，不消耗生图额度；只清理过期结果，已保存到本机目录的文件不受影响。')}</p>
          {showTimeout ? <Field>
            <FieldLabel>{t('工具执行超时')}（{t('秒')}）</FieldLabel>
            <Input type='number' min={5} max={600} step={1} value={timeout} disabled={busy} onChange={event => setTimeoutValue(Number(event.currentTarget.value))} />
            <FieldDescription>{t('这是后台生成任务的时间上限。单次 MCP 调用最多等待 20 秒，未完成时返回任务编号；查询不会重新生图。')}</FieldDescription>
          </Field> : null}
          <Alert>{t('生成较慢时先返回任务编号。断线后查询原任务，不自动重画；超时不代表没有扣费。')}</Alert>
          <div><Button disabled={busy || !draft.generation.model || Boolean(draft.editing && !draft.editing.model)} onClick={() => void save()}>{busy ? <Spinner /> : null}{t('保存')}</Button></div>
        </> : null}
      </CardContent>
    </Card>
    <ToolImageHistory api={api} />
  </div>
}
