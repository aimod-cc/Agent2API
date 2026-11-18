import * as React from 'react'
import { Alert, Badge, Button, Card, CardContent, CardHeader, CardTitle, Label, Switch } from '@ui'
import { errorMessage, shared, toast } from './settings-model'
import { t } from '../i18n'

type ToolSettings = { enabled: boolean; providers: Array<{ id: string; label: string; models: unknown[] }> }

function parse(value: unknown): ToolSettings {
  if (!value || typeof value !== 'object' || typeof (value as ToolSettings).enabled !== 'boolean'
    || !Array.isArray((value as ToolSettings).providers)) throw new Error(t('未能读取网页工具调用设置'))
  return value as ToolSettings
}

export function WebToolsPanel() {
  const [saved, setSaved] = React.useState<ToolSettings | null>(null)
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')
  const pending = React.useRef(false)
  const id = React.useId()

  const load = React.useCallback(async () => {
    if (pending.current) return
    pending.current = true
    setBusy(true)
    setError('')
    try { setSaved(parse(await shared().workbuddyDesktop?.getWebToolEmulation?.())) }
    catch (failure) { setError(errorMessage(failure)) }
    finally { pending.current = false; setBusy(false) }
  }, [])

  React.useEffect(() => { void load() }, [load])

  async function save(enabled: boolean) {
    if (pending.current) return
    pending.current = true
    setBusy(true)
    setError('')
    try {
      setSaved(parse(await shared().workbuddyDesktop?.saveWebToolEmulation?.(enabled)))
      toast(t('网页工具调用设置已保存'))
    } catch (failure) { setError(errorMessage(failure)) }
    finally { pending.current = false; setBusy(false) }
  }

  return <Card render={<section />} className='panel'>
    <CardHeader className='flex-row items-center justify-between gap-3'>
      <CardTitle>{t('网页工具调用')}</CardTitle>
      <Button variant='ghost' size='sm' disabled={busy} onClick={() => void load()}>{t('刷新')}</Button>
    </CardHeader>
    <CardContent className='panel-body flex flex-col gap-3'>
      <Label htmlFor={id} className='flex items-center justify-between gap-3'>
        <span>{t('为明确声明支持的网页模型启用工具模拟')}</span>
        <Switch id={id} checked={saved?.enabled === true} disabled={busy || !saved}
          onCheckedChange={next => void save(next)} />
      </Label>
      <p className='m-0 text-xs leading-relaxed text-subtle'>
        {t('默认开启，仅作用于插件明确声明的模型。网关通过提示词与响应校验转换工具调用，实际操作仍由客户端执行；原生工具调用和没有工具的普通聊天不受影响。')}
      </p>
      <div className='flex flex-wrap gap-2'>
        {saved?.providers.map(provider => <Badge key={provider.id} variant='outline'>{provider.label}</Badge>)}
        {saved && !saved.providers.length ? <span className='text-xs text-subtle'>{t('当前没有声明此能力的插件模型，请更新网页插件并刷新模型目录。')}</span> : null}
      </div>
      <p className='m-0 text-xs leading-relaxed text-subtle'>
        {t('工具模式下，正文与工具参数在整轮正常结束并校验后下发，思考内容仍可流式返回。工具说明或历史超过网页预算时明确报错，不会静默裁剪；格式错误、缺失参数和强制调用失败不会自动重发。')}
      </p>
      {error ? <Alert variant='destructive'>{error}</Alert> : null}
    </CardContent>
  </Card>
}
