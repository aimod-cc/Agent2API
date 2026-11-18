import { useEffect, useId, useRef, useState } from 'react'
import { Alert, Badge, Button, Label, Switch } from '@ui'
import { apiCall } from '../api/client'
import { serverIdentity } from '../lib/servers'
import { store } from '../state/store'
import { t } from '../i18n'

type Settings = { enabled: boolean; providers: Array<{ id: string; label: string }> }

export function WebToolsSettings() {
  const [saved, setSaved] = useState<Settings | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const pending = useRef(false)
  const identity = useRef(serverIdentity())
  const id = useId()

  async function load(enabled?: boolean) {
    if (pending.current) return
    pending.current = true
    setBusy(true)
    setError('')
    try {
      const result = await apiCall<Settings>('/api/web-tool-emulation', enabled === undefined ? {} : {
        method: 'PUT', body: { enabled },
      })
      if (serverIdentity() !== identity.current) return
      if (typeof result.enabled !== 'boolean' || !Array.isArray(result.providers)) throw new Error(t('未能读取网页工具调用设置'))
      setSaved(result)
      if (enabled !== undefined) store.toast(t('网页工具调用设置已保存'))
    } catch (failure) {
      if (serverIdentity() === identity.current) setError(failure instanceof Error ? failure.message : String(failure))
    } finally { pending.current = false; setBusy(false) }
  }

  useEffect(() => { void load() }, [])

  return <div className='flex flex-col gap-4'>
    <Label htmlFor={id} className='flex items-center justify-between gap-3'>
      <span>{t('为明确声明支持的网页模型启用工具模拟')}</span>
      <Switch id={id} checked={saved?.enabled === true} disabled={busy || !saved} onCheckedChange={next => void load(next)} />
    </Label>
    <p className='m-0 text-xs leading-relaxed text-subtle'>
      {t('默认开启，仅作用于插件明确声明的模型。网关通过提示词与响应校验转换工具调用，实际操作仍由客户端执行；原生工具调用和没有工具的普通聊天不受影响。')}
    </p>
    <div className='flex flex-wrap gap-2'>{saved?.providers.map(provider => <Badge key={provider.id} variant='outline'>{provider.label}</Badge>)}</div>
    {saved && !saved.providers.length ? <p className='m-0 text-xs text-subtle'>{t('当前没有声明此能力的插件模型，请更新网页插件并刷新模型目录。')}</p> : null}
    <p className='m-0 text-xs leading-relaxed text-subtle'>
      {t('工具模式下，正文与工具参数在整轮正常结束并校验后下发，思考内容仍可流式返回。工具说明或历史超过网页预算时明确报错，不会静默裁剪；格式错误、缺失参数和强制调用失败不会自动重发。')}
    </p>
    {error ? <Alert variant='destructive'>{error}</Alert> : null}
    <Button variant='outline' disabled={busy} onClick={() => void load()}>{t('刷新')}</Button>
  </div>
}
