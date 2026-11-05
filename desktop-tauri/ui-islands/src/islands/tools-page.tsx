import * as React from 'react'
import { mountOnPage } from '../island-lazy'
import {
  Alert, Badge, Button, Card, CardContent, CardHeader, CardTitle, Empty, SegmentedControl, Spinner,
  Switch, Table, TableBody, TableCell, TableHead, TableHeader, TableRow,
} from '@ui'
import { t } from '../i18n'
import { toast } from './assistant-state'
import { ToolDetail } from './tools-detail'
import { ToolInstallDialog, ToolUninstallDialog } from './tools-install'
import {
  PROTOCOL_DESC, PROTOCOL_LABELS, SERVICE_DESC, SERVICE_NAME, fetchInstallTargets, fetchServices,
  isDesktopShell, patchServer, type InstallTarget, type ToolProtocol, type ToolService,
} from './tools-data'

type Tab = 'all' | 'skill' | 'mcp'

function protocolBadge(protocol: ToolProtocol) {
  const variant = protocol === 'skill' ? 'brand' : protocol === 'mcp' ? 'info' : 'outline'
  return <Badge variant={variant}>{PROTOCOL_LABELS[protocol]}</Badge>
}

export function ToolsPage() {
  const desktop = isDesktopShell()
  const [services, setServices] = React.useState<ToolService[]>([])
  const [targets, setTargets] = React.useState<Record<string, InstallTarget[]>>({})
  const [tab, setTab] = React.useState<Tab>('all')
  const [loading, setLoading] = React.useState(true)
  const [error, setError] = React.useState('')
  const [toggling, setToggling] = React.useState('')
  const [detail, setDetail] = React.useState<ToolService | null>(null)
  const [install, setInstall] = React.useState<{ service: ToolService; mode: 'skill' | 'mcp' } | null>(null)
  const [uninstall, setUninstall] = React.useState<{ service: ToolService; mode: 'skill' | 'mcp' } | null>(null)

  const load = React.useCallback(async () => {
    setLoading(true); setError('')
    try {
      const rows = await fetchServices()
      // API 是固定对外的接口，不再作为「工具服务」页的协议条目展示；
      // 说明在「文档」页，运行时路由不受影响。
      const visibleRows = rows.filter(row => row.protocol !== 'api')
      setServices(visibleRows)
      if (desktop) void loadTargets(visibleRows)
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
    } finally {
      setLoading(false)
    }
  }, [desktop])

  const loadTargets = async (rows: ToolService[]) => {
    const entries = await Promise.all(rows.filter(row => row.protocol !== 'api').map(async row => {
      try {
        const result = await fetchInstallTargets(row.id, row.protocol === 'skill' ? 'skill' : 'mcp')
        return [row.key, result.targets || []] as const
      } catch {
        return [row.key, []] as const
      }
    }))
    setTargets(Object.fromEntries(entries))
  }

  React.useEffect(() => { void load() }, [load])

  const toggle = async (service: ToolService, enabled: boolean) => {
    setToggling(service.key); setError('')
    try {
      await patchServer(service.id, { protocol: service.protocol, enabled })
      toast(t('服务状态已更新'), 'ok')
      await load()
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
      await load()
    } finally {
      setToggling('')
    }
  }

  const visible = tab === 'all' ? services : services.filter(service => service.protocol === tab)

  if (detail) return <ToolDetail service={detail} onBack={() => { setDetail(null); void load() }} onChanged={() => { void load(); setDetail(current => current ? { ...current } : null) }} />

  return <div className='flex flex-col gap-3'>
    <Card>
      <CardHeader>
        <CardTitle>{t('工具服务')}</CardTitle>
        <p className='ml-3 text-xs text-subtle'>{t('管理可供客户端安装和运行的工具服务，并配置上游提供商与能力。')}</p>
        {desktop ? <Button size='sm' variant='outline' className='ml-auto' onClick={() => (window as unknown as { wbApp?: { showPage?: (page: string) => void } }).wbApp?.showPage?.('agent')}>{t('客户端环境')}</Button> : null}
      </CardHeader>
      <CardContent className='flex flex-col gap-3'>
        <div className='flex flex-wrap items-center gap-2'>
          <SegmentedControl value={tab} onValueChange={value => setTab(value as Tab)} options={[
            { value: 'all', label: t('全部') },
            { value: 'skill', label: 'Skill' },
            { value: 'mcp', label: 'MCP' },
          ]} aria-label={t('接入协议')} />
          <Button size='sm' variant='ghost' className='ml-auto' disabled={loading} onClick={() => void load()}>{loading ? <Spinner /> : null}{t('刷新')}</Button>
        </div>
        {error && <Alert variant='destructive' className='whitespace-pre-wrap'>{error}</Alert>}
        {loading && !services.length ? <div className='flex items-center gap-2 py-8 text-sm text-subtle'><Spinner />{t('正在加载工具服务…')}</div>
          : visible.length ? <div className='overflow-x-auto'>
            <Table>
              <TableHeader><TableRow>
                <TableHead>{t('服务名称')}</TableHead>
                <TableHead>{t('接入协议')}</TableHead>
                <TableHead>{t('状态')}</TableHead>
                <TableHead>{t('能力数量')}</TableHead>
                <TableHead>{t('版本')}</TableHead>
                <TableHead>{t('服务入口')}</TableHead>
                <TableHead>{t('操作')}</TableHead>
                {desktop ? <TableHead>{t('本机接入')}</TableHead> : null}
              </TableRow></TableHeader>
              <TableBody>
                {visible.map(service => {
                  const serviceTargets = targets[service.key] || []
                  const installed = serviceTargets.filter(target => target.installed).length
                  return <TableRow key={service.key}>
                    <TableCell>
                      <div className='font-semibold'>{SERVICE_NAME[service.id] || service.id}</div>
                      <div className='max-w-[420px] text-[11.5px] text-subtle'>{SERVICE_DESC[service.id] || ''}</div>
                    </TableCell>
                    <TableCell>
                      <div className='flex flex-col gap-1'>{protocolBadge(service.protocol)}
                        <span className='max-w-[220px] text-[11px] text-subtle'>{PROTOCOL_DESC[service.protocol]}</span>
                      </div>
                    </TableCell>
                    <TableCell>
                      <div className='flex items-center gap-2'>
                        <Badge variant={service.enabled ? 'success' : 'outline'}>{service.enabled ? t('已启用') : t('已停用')}</Badge>
                        <Switch checked={service.enabled} disabled={toggling === service.key} onCheckedChange={checked => void toggle(service, checked === true)} aria-label={t('启用 / 停用此服务')} />
                      </div>
                    </TableCell>
                    <TableCell>{service.capability_count}</TableCell>
                    <TableCell><Badge variant='outline'>v{service.version}</Badge></TableCell>
                    <TableCell><code className='whitespace-nowrap text-[11.5px]'>{service.endpoint}</code></TableCell>
                    <TableCell><Button size='sm' variant='ghost' onClick={() => setDetail(service)}>{t('详情')}</Button></TableCell>
                    {desktop ? <TableCell>
                      <div className='flex flex-wrap gap-1.5'>
                        <Button size='sm' variant='outline' disabled={!service.enabled || toggling === service.key}
                          onClick={() => setInstall({ service, mode: service.protocol === 'mcp' ? 'mcp' : 'skill' })}>
                          {installed > 0 ? t('更新') : t('安装')}
                        </Button>
                        <Button size='sm' variant='ghost' disabled={!installed} onClick={() => setUninstall({ service, mode: service.protocol === 'mcp' ? 'mcp' : 'skill' })}>{t('卸载')}</Button>
                        {installed > 0 ? <span className='self-center text-[11px] text-subtle' title={t('已接入 {count} 个目标', { count: installed })}>{installed}</span> : null}
                      </div>
                    </TableCell> : null}
                  </TableRow>
                })}
              </TableBody>
            </Table>
          </div> : <Empty>{t('没有匹配的工具服务')}</Empty>}
      </CardContent>
    </Card>

    {install && <ToolInstallDialog service={install.service} mode={install.mode}
      onClose={() => setInstall(null)}
      onDone={() => { void load(); setInstall(current => current ? { ...current } : null) }} />}
    {uninstall && <ToolUninstallDialog service={uninstall.service} mode={uninstall.mode}
      onClose={() => setUninstall(null)}
      onDone={() => { void load(); setUninstall(current => current ? { ...current } : null) }} />}
  </div>
}

const PAGE_SELECTOR = '.page[data-page="tools"]'
let mounted = false

function mount(): void {
  if (mounted) return
  const section = document.querySelector<HTMLElement>(PAGE_SELECTOR)
  if (!section) return
  mounted = true
  section.replaceChildren()
  void import('react-dom/client').then(({ createRoot }) => createRoot(section).render(<ToolsPage />))
}

mountOnPage('tools', PAGE_SELECTOR, mount)
