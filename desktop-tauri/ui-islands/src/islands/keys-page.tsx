import * as React from 'react'
import { mountOnPage } from '../island-lazy'
import { createRoot } from 'react-dom/client'
import {
  Badge,
  Button,
  Switch,
  Card,
  CardHeader,
  Table,
  TableHeader,
  TableRow,
  TableHead,
  TableBody,
  TableCell,
} from '@ui'
import { TableFooter, useClientPaging } from './table-shell'
import { t } from '../i18n'
import { KeyModal } from './keys/keys-modal'
import { UsageCell } from './keys/keys-usage-limit'
import {
  errorMessage,
  escapeHtml,
  providerLabel,
  shared,
  toast,
  type Align,
  type ColSettingsHandle,
  type ColumnDecl,
  type KeyEntry,
  type KeysPayload,
  type ProviderOption,
} from './keys/keys-types'

/**
 * Agent2API · 网关 Key 页（列表 / 新建 / 启停 / 删除 / 可用范围与用量限制）—— React 岛。
 * 对外接口：`window.wbKeysPanel = { load, render, visibleColumns }`。
 */

const COLUMNS: readonly ColumnDecl[] = [
  { key: 'name', label: t('名称') },
  { key: 'key', label: 'Key' },
  { key: 'usage', label: t('用量限制') },
  { key: 'time', label: t('创建时间') },
  { key: 'state', label: t('启用') },
  { key: 'act', label: t('操作'), align: 'right' },
]

const CELL_CLASS: Record<string, string> = {
  name: 'cell-name',
  key: 'cell-key',
  usage: 'cell-usage',
  time: 'cell-time',
  state: 'cell-state state',
  act: 'cell-act r',
}

const SECTION = '.page[data-page="keys"]'

let colSettings: ColSettingsHandle | null = null
let onColumnsChanged: (() => void) | null = null

function visibleColumns(): (ColumnDecl & { align: Align })[] {
  if (colSettings) return colSettings.apply([...COLUMNS])
  return COLUMNS.map(column => ({ ...column, align: column.align ?? 'left' }))
}

function syncHead(): void {
  shared().wbColSettings?.syncStaticHead?.('keys', document.querySelector('table.keys-table'))
}

function setupColumns(): void {
  if (colSettings) return
  const handle = shared().wbColSettings?.register({
    id: 'keys',
    label: t('网关 Key 表'),
    columns: COLUMNS,
    mount: () => document.querySelector('.page[data-page="keys"] .panel-head .head-actions'),
    onChange: () => {
      syncHead()
      onColumnsChanged?.()
    },
  })
  if (!handle) return
  colSettings = handle
  syncHead()
}

function keysOf(payload: KeysPayload | null): KeyEntry[] {
  return Array.isArray(payload?.keys) ? payload.keys : []
}

function providersOf(payload: KeysPayload | null): ProviderOption[] {
  return Array.isArray(payload?.providers) ? payload.providers : []
}

function modelsByProviderOf(payload: KeysPayload | null): Record<string, string[]> {
  const map = payload?.modelsByProvider
  return map && typeof map === 'object' ? map : {}
}

function restrictionText(k: KeyEntry): string {
  const providers = Array.isArray(k.allowedProviders) ? k.allowedProviders : []
  const models = Array.isArray(k.allowedModels) ? k.allowedModels : []
  if (!providers.length && !models.length) return t('不限制')
  const parts: string[] = []
  if (providers.length) parts.push(providers.map(providerLabel).join(t('、')))
  if (models.length) parts.push(t('{n} 个模型', { n: models.length }))
  return t('限制：{text}', { text: parts.join(' / ') })
}

function restrictionTitle(k: KeyEntry): string {
  const providers = Array.isArray(k.allowedProviders) ? k.allowedProviders : []
  const models = Array.isArray(k.allowedModels) ? k.allowedModels : []
  if (!providers.length && !models.length) {
    return t('这把 Key 不限制提供商与模型（可用全部上游与全部对外模型）')
  }
  const lines: string[] = []
  if (providers.length) lines.push(t('可用提供商：{names}', { names: providers.map(providerLabel).join(t('、')) }))
  if (models.length) lines.push(t('可用模型：{names}', { names: models.join(t('、')) }))
  return lines.join('\n')
}

type PageHandle = {
  load(): Promise<void>
  render(): void
}

let handle: PageHandle | null = null

async function load(): Promise<void> {
  await handle?.load()
}

function render(): void {
  handle?.render()
}

function KeysPage() {
  const [data, setData] = React.useState<KeysPayload | null>(null)
  const [revealed, setRevealed] = React.useState<ReadonlySet<string>>(() => new Set())
  const [pending, setPending] = React.useState<ReadonlySet<string>>(() => new Set())
  const [modal, setModal] = React.useState<{ key: KeyEntry | null } | null>(null)
  const [, setVersion] = React.useState(0)

  const pendingRef = React.useRef<Set<string>>(new Set())
  const loadingRef = React.useRef(false)

  const applyData = React.useCallback((next: KeysPayload | null) => {
    setData(next)
  }, [])

  const accept = React.useCallback((next: KeysPayload | null | undefined) => {
    if (next && Array.isArray(next.keys)) applyData(next)
  }, [applyData])

  const loadPanel = React.useCallback(async (): Promise<void> => {
    if (loadingRef.current) return
    loadingRef.current = true
    try {
      const api = shared().workbuddyDesktop
      if (!api) throw new Error(t('后端桥不可用'))
      applyData((await api.getKeys()) ?? null)
    } catch (error) {
      toast(t('读取 Key 列表失败：{reason}', { reason: errorMessage(error) }), 'err')
    } finally {
      loadingRef.current = false
    }
  }, [applyData])

  async function runRowAction(
    id: string,
    run: () => Promise<KeysPayload | null | undefined>,
    doneText?: string,
  ): Promise<void> {
    if (pendingRef.current.has(id)) return
    pendingRef.current.add(id)
    setPending(new Set(pendingRef.current))
    try {
      accept(await run())
      if (doneText) toast(doneText)
    } catch (error) {
      toast(t('操作失败：{reason}', { reason: errorMessage(error) }), 'err')
    } finally {
      pendingRef.current.delete(id)
      setPending(new Set(pendingRef.current))
    }
  }

  function toggleReveal(id: string): void {
    setRevealed(prev => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }

  async function removeKey(k: KeyEntry): Promise<void> {
    const api = shared().workbuddyDesktop
    const ask = shared().wbConfirm?.ask
    if (!api || !ask) return
    const label = escapeHtml(k.name || k.masked || k.id)
    const confirmed = await ask({
      title: t('删除网关 Key'),
      html: t('确定删除 Key「<strong>{name}</strong>」？使用它的客户端会立刻无法访问。', { name: label }),
      okText: t('删除'),
      okClass: 'danger',
    })
    if (!confirmed) return
    void runRowAction(k.id, () => api.deleteKey(k.id), t('Key 已删除'))
  }

  function toggleEnabled(k: KeyEntry, next: boolean): void {
    const api = shared().workbuddyDesktop
    if (!api) return
    void runRowAction(
      k.id,
      () => api.updateKey(k.id, { enabled: next }),
      next ? t('Key 已启用') : t('Key 已停用'),
    )
  }

  React.useLayoutEffect(() => {
    handle = {
      load: loadPanel,
      render: () => setVersion(version => version + 1),
    }
    onColumnsChanged = () => setVersion(version => version + 1)
    setupColumns()
    setVersion(version => version + 1)
    return () => {
      handle = null
      onColumnsChanged = null
    }
  }, [loadPanel])

  React.useEffect(() => {
    if (shared().wbApp?.currentPage === 'keys') void loadPanel()
  }, [loadPanel])

  React.useEffect(() => {
    shared().wbApp?.renderTopbarStatus?.()
  }, [data])

  const keys = keysOf(data)
  const paging = useClientPaging(keys.length, 'keys')
  const list = paging.paged ? paging.slice(keys) : keys
  const columns = visibleColumns()
  const enabledCount = data?.usableKeyCount ?? keys.filter(item => item.enabled).length
  const hasUsableKey = enabledCount > 0
  const badgeText = hasUsableKey
    ? t('已启用鉴权 · {n} 把 Key 生效', { n: enabledCount })
    : t('没有可用的 API Key，请先在「网关 Key」页创建并启用')

  function cell(columnKey: string, k: KeyEntry, busyRow: boolean): React.ReactNode {
    switch (columnKey) {
      case 'name':
        return (
          <>
            <div className='mid'><span className='t'>{k.name || t('未命名')}</span></div>
            <div className='mname' title={restrictionTitle(k)}>{restrictionText(k)}</div>
          </>
        )
      case 'key': {
        const shown = revealed.has(k.id)
        return (
          <div className='keycell'>
            <code className='kv'>{shown ? k.key : k.masked}</code>
            <Button size='sm' variant='ghost' onClick={() => toggleReveal(k.id)}>
              {shown ? t('隐藏') : t('显示')}
            </Button>
            <Button size='sm' variant='ghost' data-copy={k.key} title={t('复制 Key')}>{t('复制')}</Button>
          </div>
        )
      }
      case 'usage':
        return <UsageCell entry={k} />
      case 'time':
        return (
          <span className='muted'>{k.createdAt ? shared().wbApp?.formatTime?.(k.createdAt) : '—'}</span>
        )
      case 'state':
        return (
          <Switch
            checked={k.enabled === true}
            disabled={busyRow}
            aria-label={t('启用「{name}」', { name: k.name || t('未命名') })}
            onCheckedChange={next => toggleEnabled(k, next)}
          />
        )
      case 'act':
        return (
          <div className='row-actions'>
            <Button size='sm' variant='ghost' disabled={busyRow} onClick={() => setModal({ key: k })}>
              {t('编辑')}
            </Button>
            <Button
              size='sm'
              variant='destructive'
              disabled={busyRow}
              onClick={() => void removeKey(k)}
            >
              {t('删除')}
            </Button>
          </div>
        )
      default:
        return null
    }
  }

  return (
    <Card render={<section />} className='panel'>
      <CardHeader className='panel-head'>
        <h2>{t('API Key 列表')}</h2>
        <Badge
          id='keys-status'
          variant={hasUsableKey ? 'success' : 'warning'}
          data-tone={hasUsableKey ? 'ok' : 'warn'}
        >
          {badgeText}
        </Badge>
        <div className='head-actions'>
          <Button variant='default' onClick={() => setModal({ key: null })}>
            {t('＋ 新建 Key')}
          </Button>
        </div>
      </CardHeader>

      <div className='models-table-wrap'>
        <Table container={false} className='models-table keys-table'>
          <colgroup>
            <col className='k-name' data-col='name' />
            <col className='k-key' data-col='key' />
            <col className='k-usage' data-col='usage' />
            <col className='k-time' data-col='time' />
            <col className='k-state' data-col='state' />
            <col className='k-act' data-col='act' />
          </colgroup>
          <TableHeader>
            <TableRow>
              <TableHead data-col='name'>{t('名称')}</TableHead>
              <TableHead data-col='key'>Key</TableHead>
              <TableHead data-col='usage'>{t('用量限制')}</TableHead>
              <TableHead data-col='time'>{t('创建时间')}</TableHead>
              <TableHead data-col='state'>{t('启用')}</TableHead>
              <TableHead className='r' data-col='act'>{t('操作')}</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {list.length ? list.map(k => {
              const busyRow = pending.has(k.id)
              return (
                <TableRow key={k.id} className={k.enabled ? '' : 'off'} data-id={k.id}>
                  {columns.map(column => (
                    <TableCell key={column.key} className={`${CELL_CLASS[column.key] ?? ''} ta-${column.align}`}>
                      {cell(column.key, k, busyRow)}
                    </TableCell>
                  ))}
                </TableRow>
              )
            }) : (
              <TableRow>
                <TableCell colSpan={columns.length} className='empty'>
                  {data ? t('没有可用的 API Key，请先在「网关 Key」页创建并启用') : t('加载中…')}
                </TableCell>
              </TableRow>
            )}
          </TableBody>
        </Table>
      </div>

      <TableFooter
        leading={(
          <span>
            {t('客户端请求需带 ')}
            <code>{'Authorization: Bearer <key>'}</code>
            {t(' 或 ')}
            <code>{'x-api-key: <key>'}</code>
            {t('；修改后立即生效。模型测试、助手和日活保活必须选择 Key，受限 Key 不能访问管理接口。每把 Key 可单独限制')}
            <b>{t('可用提供商')}</b>
            {t('、')}
            <b>{t('可用模型')}</b>
            {t('与')}
            <b>{t('用量限制')}</b>
            {t('（支持按 Token 用量或请求次数设置日/周/月及累计上限）。')}
          </span>
        )}
        total={keys.length}
        range={paging.paged ? { start: paging.rangeStart, end: paging.rangeEnd } : null}
        page={paging.page}
        pageCount={paging.pageCount}
        size={paging.size}
        onSizeChange={paging.setSize}
        onPageChange={paging.goto}
      />

      {modal ? (
        <KeyModal
          target={modal.key}
          providers={providersOf(data)}
          modelsByProvider={modelsByProviderOf(data)}
          onClose={() => setModal(null)}
          onSaved={(next, revealId) => {
            if (revealId) setRevealed(prev => new Set(prev).add(revealId))
            accept(next)
          }}
        />
      ) : null}
    </Card>
  )
}

let mounted = false

function mount(): void {
  if (mounted) return
  const section = document.querySelector<HTMLElement>(SECTION)
  if (!section) return
  mounted = true
  section.replaceChildren()
  createRoot(section).render(<KeysPage />)
}

mountOnPage('keys', SECTION, mount)

declare global {
  interface Window {
    wbKeysPanel?: {
      load(): Promise<void>
      render(): void
      visibleColumns(): (ColumnDecl & { align: Align })[]
    }
  }
}

window.wbKeysPanel = { load, render, visibleColumns }
