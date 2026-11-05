import * as React from 'react'
import {
  Alert, Badge, Button, Card, CardContent, CardHeader, CardTitle, Field, FieldLabel, Input,
  SegmentedControl, Spinner, Switch,
} from '@ui'
import { t } from '../i18n'
import { toast } from './assistant-state'
import { WebSearchConfig, VisionConfigPanel } from './tools-config'
import {
  PROTOCOL_DESC, PROTOCOL_LABELS, SERVICE_DESC, SERVICE_NAME, fetchInstallInfo, patchServer,
  type InstallInfo, type ToolProtocol, type ToolService,
} from './tools-data'

const CAPABILITY_DESCRIPTIONS: Record<string, { title: string; desc: string; params: Array<{ name: string; type: string; desc: string; required: boolean }> }[]> = {
  'web-search': [
    {
      title: t('批量网页搜索'),
      desc: t('批量搜索 1～10 条查询，并发调用后台选中的提供商后合并重复来源。'),
      params: [{ name: 'queries', type: 'string[]', desc: t('要搜索的查询，每条并发执行（1～10 条）'), required: true }],
    },
    {
      title: t('批量网页抓取'),
      desc: t('批量抓取 1～10 个 URL，使用后台选中的单个抓取提供商。'),
      params: [{ name: 'urls', type: 'string[]', desc: t('要抓取并提取正文的网页 URL（1～10 个）'), required: true }],
    },
  ],
  vision: [
    {
      title: t('图片分析'),
      desc: t('把 1～10 张图片与提示词交给后台配置的视觉模型，返回模型回复文本。'),
      params: [
        { name: 'images', type: 'string[]', desc: t('要分析的图片（1～10 张），每张为 http(s) URL 或图片 data URI'), required: true },
        { name: 'prompt', type: 'string', desc: t('提示词，描述要让模型在图片中查看的内容'), required: true },
      ],
    },
  ],
}

function copyText(text: string) {
  if (navigator.clipboard?.writeText) {
    return navigator.clipboard.writeText(text).then(() => true).catch(() => false)
  }
  return Promise.resolve(false)
}

function McpConnectCard({ info, protocol }: { info: InstallInfo; protocol: ToolProtocol }) {
  if (protocol !== 'mcp') return null
  const snippet = JSON.stringify({
    mcpServers: {
      [info.mcp.server_name]: {
        type: 'http',
        url: info.mcp.url,
        headers: { Authorization: 'Bearer <Agent2API API Key>' },
      },
    },
  }, null, 2)
  return <Card>
    <CardHeader><CardTitle>{t('MCP 接入信息')}</CardTitle></CardHeader>
    <CardContent className='flex flex-col gap-3'>
      <div>
        <div className='text-sm font-medium'>{t('服务地址')}</div>
        <div className='mt-1 flex items-center gap-2'>
          <code className='min-w-0 flex-1 break-all rounded-sm bg-surface-2 px-2 py-1 text-xs'>{info.mcp.url}</code>
          <Button size='sm' variant='outline' onClick={() => void copyText(info.mcp.url).then(ok => toast(ok ? t('已复制') : t('复制失败，请手动选择后复制'), ok ? 'ok' : 'err'))}>{t('复制')}</Button>
        </div>
        <p className='mt-1 text-[11.5px] text-subtle'>{t('在支持远程 MCP 的客户端中填写该地址，并使用网关 Key 作为 Bearer 令牌。')}</p>
      </div>
      <div>
        <div className='text-sm font-medium'>{t('暴露的工具')}</div>
        <div className='mt-1 flex flex-wrap gap-1.5'>{info.mcp.tools.map(name => <Badge key={name}>{name}</Badge>)}</div>
      </div>
      <div>
        <div className='flex items-center gap-2'>
          <div className='text-sm font-medium'>{t('客户端配置示例')}</div>
          <Button size='sm' variant='outline' className='ml-auto' onClick={() => void copyText(snippet).then(ok => toast(ok ? t('已复制') : t('复制失败，请手动选择后复制'), ok ? 'ok' : 'err'))}>{t('复制')}</Button>
        </div>
        <p className='mt-1 text-[11.5px] text-subtle'>{t('把下面这段合并到客户端配置的 mcpServers 里，并把 Key 换成你自己的。')}</p>
        <pre className='mt-2 max-h-64 overflow-auto rounded-md bg-surface-2 p-3 text-[11.5px] leading-relaxed'>{snippet}</pre>
      </div>
    </CardContent>
  </Card>
}

export function ToolDetail({ service, onBack, onChanged }: { service: ToolService; onBack: () => void; onChanged: () => void }) {
  const [info, setInfo] = React.useState<InstallInfo | null>(null)
  const [loading, setLoading] = React.useState(true)
  const [error, setError] = React.useState('')
  const [enabled, setEnabled] = React.useState(service.enabled)
  const [toggling, setToggling] = React.useState(false)
  const [timeoutDraft, setTimeoutDraft] = React.useState<number>(service.timeout_seconds)
  const [savedTimeout, setSavedTimeout] = React.useState<number>(service.timeout_seconds)
  const [savingTimeout, setSavingTimeout] = React.useState(false)
  const [tab, setTab] = React.useState<'config' | 'capabilities'>('config')

  React.useEffect(() => {
    let alive = true
    setLoading(true); setError('')
    fetchInstallInfo(service.id)
      .then(result => { if (alive) setInfo(result) })
      .catch(failure => { if (alive) setError(failure instanceof Error ? failure.message : String(failure)) })
      .finally(() => { if (alive) setLoading(false) })
    return () => { alive = false }
  }, [service.id])

  React.useEffect(() => { setTimeoutDraft(service.timeout_seconds); setSavedTimeout(service.timeout_seconds) }, [service.timeout_seconds])
  React.useEffect(() => { setEnabled(service.enabled) }, [service.enabled])

  const toggle = async (enabled: boolean) => {
    setToggling(true); setError('')
    try {
      await patchServer(service.id, { protocol: service.protocol, enabled })
      setEnabled(enabled)
      toast(t('服务状态已更新'), 'ok')
      onChanged()
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
    } finally {
      setToggling(false)
    }
  }

  const saveTimeout = async () => {
    if (!timeoutDraft) return
    setSavingTimeout(true); setError('')
    try {
      await patchServer(service.id, { protocol: service.protocol, timeout_seconds: timeoutDraft })
      setSavedTimeout(timeoutDraft)
      toast(t('工具执行超时已保存'), 'ok')
      onChanged()
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
    } finally {
      setSavingTimeout(false)
    }
  }

  const capabilities = CAPABILITY_DESCRIPTIONS[service.id] || []

  return <div className='flex flex-col gap-4'>
    <Button variant='ghost' size='sm' className='self-start' onClick={onBack}>← {t('返回列表')}</Button>

    <Card>
      <CardHeader>
        <CardTitle>{SERVICE_NAME[service.id] || service.id}</CardTitle>
        <div className='ml-2 flex flex-wrap items-center gap-2'>
          <Badge>{PROTOCOL_LABELS[service.protocol]}</Badge>
          <Badge variant={enabled ? 'success' : 'outline'}>{enabled ? t('已启用') : t('已停用')}</Badge>
          <span className='text-xs text-subtle'>{t('能力数量')}: {service.capability_count}</span>
          <span className='text-xs text-subtle'>v{service.version}</span>
        </div>
        <div className='ml-auto flex items-center gap-2'>
          <span className='text-xs text-subtle'>{t('启用 / 停用此服务')}</span>
          <Switch checked={enabled} disabled={toggling} onCheckedChange={checked => void toggle(checked === true)} aria-label={t('启用 / 停用此服务')} />
        </div>
      </CardHeader>
      <CardContent className='flex flex-col gap-3'>
        <p className='text-xs text-subtle'>{SERVICE_DESC[service.id] || ''}</p>
        <p className='text-xs text-subtle'>{PROTOCOL_DESC[service.protocol]}</p>
        <div className='flex flex-wrap items-end gap-2'>
          <Field className='w-[180px]'>
            <FieldLabel>{t('工具执行超时')}</FieldLabel>
            <Input type='number' min={5} max={600} value={String(timeoutDraft)} onChange={event => setTimeoutDraft(Number(event.currentTarget.value) || 0)} />
          </Field>
          <span className='pb-2 text-xs text-subtle'>{t('秒')}</span>
          <Button size='sm' variant='outline' disabled={loading || savingTimeout || !timeoutDraft || timeoutDraft === savedTimeout} onClick={() => void saveTimeout()}>
            {savingTimeout ? <Spinner /> : null}{t('保存')}
          </Button>
        </div>
        <p className='text-[11.5px] text-subtle'>{t('服务端强制单次工具执行的超时上限（同一服务的 Skill / MCP 共用）。安装 MCP 时会按此值写入客户端配置；修改后需在客户端重新「更新」配置并重进会话才会生效。')}</p>
        {!enabled && <Alert variant='warning'>{t('该协议已停用，已接入的客户端会收到服务停用错误。')}</Alert>}
        {error && <Alert variant='destructive' className='whitespace-pre-wrap'>{error}</Alert>}
      </CardContent>
    </Card>

    {loading ? <div className='flex items-center gap-2 text-sm text-subtle'><Spinner />{t('正在加载工具服务配置…')}</div>
      : service.protocol === 'mcp' && info ? <McpConnectCard info={info} protocol={service.protocol} />
        : service.protocol === 'skill' ? <Alert>{t('该协议通过 Agent2API 面板安装到本机客户端目录。')}</Alert> : null}

    <div className='flex items-center gap-2'>
      <SegmentedControl value={tab} onValueChange={value => { if (value === 'config' || value === 'capabilities') setTab(value) }} options={[
        { value: 'config', label: t('服务配置') }, { value: 'capabilities', label: t('服务能力') },
      ]} aria-label={t('服务配置')} />
    </div>

    {tab === 'config'
      ? (service.id === 'vision' ? <VisionConfigPanel /> : <WebSearchConfig />)
      : <Card>
        <CardHeader><CardTitle>{t('服务能力')}</CardTitle><span className='ml-auto text-xs text-subtle'>{t('共 {count} 项能力', { count: service.capability_count })}</span></CardHeader>
        <CardContent className='grid grid-cols-1 gap-3 md:grid-cols-2'>
          {capabilities.map(capability => <div key={capability.title} className='rounded-md border border-border p-3'>
            <div className='text-sm font-semibold'>{capability.title}</div>
            <p className='mt-1 text-xs text-subtle'>{capability.desc}</p>
            <div className='mt-2 flex flex-col gap-1.5'>
              {capability.params.map(param => <div key={param.name} className='flex flex-wrap items-baseline gap-2 text-[11.5px]'>
                <code className='rounded-sm bg-surface-2 px-1.5 py-0.5'>{param.name}</code>
                <span className='text-subtle'>{param.type}</span>
                <span className={param.required ? 'text-destructive' : 'text-subtle'}>{param.required ? t('必填') : t('可选')}</span>
                <span className='w-full text-subtle'>{param.desc}</span>
              </div>)}
            </div>
          </div>)}
        </CardContent>
      </Card>}
  </div>
}
