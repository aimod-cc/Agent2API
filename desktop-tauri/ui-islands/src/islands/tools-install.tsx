import * as React from 'react'
import {
  Alert, Button, Checkbox, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader,
  DialogTitle, Empty, Field, FieldDescription, FieldLabel, Select, SelectContent, SelectItem,
  SelectTrigger, SelectValue, SegmentedControl, Spinner,
} from '@ui'
import { t } from '../i18n'
import { toast } from './assistant-state'
import {
  fetchApiKeys, fetchInstallTargets, installMcp, installSkill, uninstallMcp, uninstallSkill,
  type ApiKeyOption, type InstallOutcome, type InstallTarget, type ToolService,
} from './tools-data'

type Mode = 'skill' | 'mcp'

function targetLabel(target: InstallTarget): string {
  if (!target.detected) return t('未检测到')
  return target.installed ? t('已安装') : t('已检测到')
}

function KeyPicker({ keys, value, onChange }: { keys: ApiKeyOption[]; value: string; onChange: (value: string) => void }) {
  const selected = keys.find(key => key.id === value)
  const label = selected ? `${selected.name}（${selected.masked}）` : keys.length ? t('请选择 API Key') : t('没有可用的网关 Key')
  return <Field>
    <FieldLabel>{t('调用用的 API Key')}</FieldLabel>
    <Select value={value} onValueChange={next => { if (typeof next === 'string') onChange(next) }}>
      <SelectTrigger className='w-full' disabled={!keys.length}><SelectValue>{label}</SelectValue></SelectTrigger>
      <SelectContent>{keys.map(key => <SelectItem key={key.id} value={key.id}>{key.name}（{key.masked}）</SelectItem>)}</SelectContent>
    </Select>
    <FieldDescription>{t('Key 明文只会写入所选客户端的本机配置文件。')}</FieldDescription>
  </Field>
}

function TargetList({ targets, selected, onToggle }: { targets: InstallTarget[]; selected: string[]; onToggle: (key: string) => void }) {
  if (!targets.length) return <Empty>{t('当前没有可用的安装目标')}</Empty>
  return <div className='flex max-h-[320px] flex-col gap-1 overflow-auto rounded-md border border-border p-2'>
    {targets.map(target => <label key={target.key} className='flex cursor-pointer items-center gap-2 rounded-sm px-2 py-1.5 text-sm hover:bg-nav-hover'>
      <Checkbox checked={selected.includes(target.key)} onCheckedChange={() => onToggle(target.key)} />
      <span className='min-w-0 flex-1'>
        <span className='font-medium'>{target.label}</span>
        <span className='ml-2 text-xs text-subtle'>{targetLabel(target)}</span>
        <span className='block truncate text-[11px] text-subtle' title={target.path}>{target.path}</span>
      </span>
    </label>)}
  </div>
}

function outcomesMessage(results: InstallOutcome[]): { ok: number; failed: InstallOutcome[] } {
  return { ok: results.filter(item => item.ok).length, failed: results.filter(item => !item.ok) }
}

export function ToolInstallDialog({ service, mode, onClose, onDone }: {
  service: ToolService; mode: Mode; onClose: () => void; onDone: () => void
}) {
  const [targets, setTargets] = React.useState<InstallTarget[]>([])
  const [keys, setKeys] = React.useState<ApiKeyOption[]>([])
  const [selected, setSelected] = React.useState<string[]>([])
  const [keyId, setKeyId] = React.useState('')
  const [variant, setVariant] = React.useState(service.recommended_variant === 'python' ? 'python' : 'node')
  const [mcpMode, setMcpMode] = React.useState<'bridge' | 'http'>(service.id === 'vision' ? 'bridge' : 'http')
  const [loading, setLoading] = React.useState(true)
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')
  const [results, setResults] = React.useState<InstallOutcome[] | null>(null)

  React.useEffect(() => {
    let alive = true
    setLoading(true); setError('')
    Promise.all([fetchInstallTargets(service.id, mode), fetchApiKeys()])
      .then(([targetsResult, keysResult]) => {
        if (!alive) return
        setTargets(targetsResult.targets || [])
        setKeys(keysResult.keys || [])
        setSelected((targetsResult.targets || []).filter(target => target.detected).map(target => target.key))
        setKeyId(keysResult.keys?.[0]?.id || '')
      })
      .catch(failure => { if (alive) setError(failure instanceof Error ? failure.message : String(failure)) })
      .finally(() => { if (alive) setLoading(false) })
    return () => { alive = false }
  }, [service.id, mode])

  const toggle = (key: string) => setSelected(current => current.includes(key) ? current.filter(item => item !== key) : [...current, key])
  const allSelected = targets.length > 0 && targets.every(target => selected.includes(target.key))
  const toggleAll = () => setSelected(allSelected ? [] : targets.map(target => target.key))

  const submit = async () => {
    if (!selected.length) { setError(t('请至少选择一个目标')); return }
    if (!keyId) { setError(t('请先选择 API Key')); return }
    setBusy(true); setError(''); setResults(null)
    try {
      const response = mode === 'skill'
        ? await installSkill({ variant, api_key_id: keyId, clients: selected })
        : await installMcp({ service_id: service.id, mode: mcpMode, api_key_id: keyId, clients: selected })
      const { ok, failed } = outcomesMessage(response.results || [])
      setResults(response.results || [])
      if (ok > 0) {
        toast(mode === 'skill' ? t('已安装到 {count} 个客户端', { count: ok }) : t('已写入 {count} 个客户端配置', { count: ok }), 'ok')
        onDone()
        if (!failed.length) onClose()
      }
      if (failed.length) setError(failed.map(item => `${item.label}：${item.error || t('写入失败')}`).join('\n'))
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
    } finally {
      setBusy(false)
    }
  }

  const serviceLabel = service.id === 'vision' ? t('视觉分析') : t('网络搜索')
  const title = mode === 'skill'
    ? t('安装 {name}', { name: serviceLabel })
    : t('{name} 接入客户端', { name: serviceLabel })

  return <Dialog open onOpenChange={(open, details) => { if (!open) { if (busy) details.cancel(); else onClose() } }}>
    <DialogContent className='max-w-2xl'>
      <DialogHeader><DialogTitle>{title}</DialogTitle></DialogHeader>
      <DialogBody className='flex flex-col gap-3'>
        <p className='text-xs text-subtle'>{mode === 'skill'
          ? t('选择安装目标、运行时与调用用的 API Key。Skill 包与密钥只写入所选客户端的本机目录。')
          : t('选择要写入 MCP 配置的客户端。配置只动本服务那一项，客户端原有的其它 MCP 服务保持不变。')}</p>
        {loading ? <div className='flex items-center gap-2 py-6 text-sm text-subtle'><Spinner />{t('正在加载安装目标…')}</div> : <>
          <div className='flex items-center gap-2'>
            <span className='text-sm font-medium'>{t('安装目标')}</span>
            <span className='text-xs text-subtle'>{t('已选择 {count} / {total} 个', { count: selected.length, total: targets.length })}</span>
            <Button variant='ghost' size='sm' className='ml-auto' onClick={toggleAll}>{allSelected ? t('取消全选') : t('全选')}</Button>
          </div>
          <TargetList targets={targets} selected={selected} onToggle={toggle} />
          {mode === 'skill' && <Field>
            <FieldLabel>{t('运行时')}</FieldLabel>
            <SegmentedControl value={variant} onValueChange={value => { if (typeof value === 'string') setVariant(value) }} aria-label={t('运行时')} options={[
              { value: 'node', label: 'Node.js' }, { value: 'python', label: 'Python' },
            ]} />
            <FieldDescription>{t('Node.js 无需额外依赖，推荐使用；Python 需要本机有 python。')}</FieldDescription>
          </Field>}
          {mode === 'mcp' && service.id === 'vision' && <Field>
            <FieldLabel>{t('接入方式')}</FieldLabel>
            <SegmentedControl value={mcpMode} onValueChange={value => { if (value === 'bridge' || value === 'http') setMcpMode(value) }} aria-label={t('接入方式')} options={[
              { value: 'bridge', label: t('本地桥接') }, { value: 'http', label: t('远程 HTTP') },
            ]} />
            <FieldDescription>{mcpMode === 'bridge'
              ? t('本地桥接会在本机运行一个 node 脚本，支持直接传本地图片路径，推荐在桌面端使用。')
              : t('远程 HTTP 只接受图片 URL 或 data URI，不需要本机安装 node。')}</FieldDescription>
          </Field>}
          <KeyPicker keys={keys} value={keyId} onChange={setKeyId} />
          {service.timeout_seconds ? <Alert>{t('将按服务的工具执行超时（{seconds} 秒）同步写入客户端配置。', { seconds: service.timeout_seconds })}</Alert> : null}
          {results && <Alert variant='success'>{t('已完成 {count} 个目标', { count: results.filter(item => item.ok).length })}</Alert>}
          {error && <Alert variant='destructive' className='whitespace-pre-wrap'>{error}</Alert>}
        </>}
      </DialogBody>
      <DialogFooter>
        <Button variant='outline' disabled={busy} onClick={onClose}>{t('取消')}</Button>
        <Button disabled={busy || loading || !selected.length || !keyId} onClick={() => void submit()}>
          {busy ? <Spinner /> : null}{mode === 'skill' ? t('安装') : t('写入配置')}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
}

export function ToolUninstallDialog({ service, mode, onClose, onDone }: {
  service: ToolService; mode: Mode; onClose: () => void; onDone: () => void
}) {
  const [targets, setTargets] = React.useState<InstallTarget[]>([])
  const [selected, setSelected] = React.useState<string[]>([])
  const [loading, setLoading] = React.useState(true)
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')

  React.useEffect(() => {
    let alive = true
    fetchInstallTargets(service.id, mode)
      .then(result => {
        if (!alive) return
        const installed = (result.targets || []).filter(target => target.installed)
        setTargets(installed)
        setSelected(installed.map(target => target.key))
      })
      .catch(failure => { if (alive) setError(failure instanceof Error ? failure.message : String(failure)) })
      .finally(() => { if (alive) setLoading(false) })
    return () => { alive = false }
  }, [service.id, mode])

  const toggle = (key: string) => setSelected(current => current.includes(key) ? current.filter(item => item !== key) : [...current, key])

  const submit = async () => {
    if (!selected.length) { setError(t('请至少选择一个目标')); return }
    setBusy(true); setError('')
    try {
      const response = mode === 'skill'
        ? await uninstallSkill({ service_id: service.id, clients: selected })
        : await uninstallMcp({ service_id: service.id, clients: selected })
      const { ok, failed } = outcomesMessage(response.results || [])
      if (ok > 0) {
        toast(t('已处理 {count} 个目标', { count: ok }), 'ok')
        onDone()
        if (!failed.length) onClose()
      }
      if (failed.length) setError(failed.map(item => `${item.label}：${item.error || t('卸载失败')}`).join('\n'))
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
    } finally {
      setBusy(false)
    }
  }

  return <Dialog open onOpenChange={(open, details) => { if (!open) { if (busy) details.cancel(); else onClose() } }}>
    <DialogContent className='max-w-xl'>
      <DialogHeader><DialogTitle>{t('移除 {name} 配置', { name: service.id === 'vision' ? t('视觉分析') : t('网络搜索') })}</DialogTitle></DialogHeader>
      <DialogBody className='flex flex-col gap-3'>
        <p className='text-xs text-subtle'>{t('选择要从中移除的客户端，未勾选的客户端保持不变。')}</p>
        {loading ? <div className='flex items-center gap-2 py-6 text-sm text-subtle'><Spinner />{t('正在加载安装目标…')}</div>
          : targets.length ? <TargetList targets={targets} selected={selected} onToggle={toggle} /> : <Empty>{t('当前没有已安装/已配置的目标')}</Empty>}
        {error && <Alert variant='destructive' className='whitespace-pre-wrap'>{error}</Alert>}
      </DialogBody>
      <DialogFooter>
        <Button variant='outline' disabled={busy} onClick={onClose}>{t('取消')}</Button>
        <Button variant='destructive' disabled={busy || !selected.length} onClick={() => void submit()}>{t('卸载')}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
}
