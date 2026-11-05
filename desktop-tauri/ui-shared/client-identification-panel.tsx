import { useCallback, useEffect, useRef, useState } from 'react'
import { Alert, Badge, Button, Card, CardContent, CardHeader, Collapsible, CollapsibleContent, CollapsibleTrigger, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, Input, Label, Switch } from '@ui'
import type { Translate } from './assistant-read-types'
import { ClientRuleEditor } from './client-identification-editor'
import { clientSettingsError, IDENTIFICATION_CHANGED, notifyIdentificationChanged, type ClientIdentificationApi, type ClientIdentificationView, type ClientRule, type ClientSettings } from './client-identification'

type Confirmation = { title: string; message: string; config: ClientSettings; revision: string }

export function ClientIdentificationPanel({ api, t, active = true }: { api: ClientIdentificationApi; t: Translate; active?: boolean }) {
  const [view, setView] = useState<ClientIdentificationView | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [editor, setEditor] = useState<{ rule?: ClientRule } | null>(null)
  const [rename, setRename] = useState<{ id: string; name: string } | null>(null)
  const [confirmation, setConfirmation] = useState<Confirmation | null>(null)
  const seq = useRef(0)
  const alive = useRef(true)
  const load = useCallback(async () => {
    const token = ++seq.current
    try {
      const next = await api.load()
      if (alive.current && token === seq.current) { setView(next); setError('') }
    } catch (failure) {
      if (alive.current && token === seq.current) setError(failure instanceof Error ? failure.message : String(failure))
    }
  }, [api])
  useEffect(() => { alive.current = true; return () => { alive.current = false; ++seq.current } }, [])
  useEffect(() => {
    if (!active) return
    void load()
    const changed = () => { void load() }
    window.addEventListener(IDENTIFICATION_CHANGED, changed)
    return () => { window.removeEventListener(IDENTIFICATION_CHANGED, changed); ++seq.current }
  }, [load, active])
  const save = async (config: ClientSettings, revision = view?.revision) => {
    if (!view || busy || !revision) return
    const invalid = clientSettingsError(config, view, t)
    if (invalid) { setError(invalid); return }
    ++seq.current; setBusy(true); setError('')
    try {
      const next = await api.save({ config, revision })
      notifyIdentificationChanged()
      if (alive.current) { setView(next); setRename(null); setConfirmation(null) }
    } catch (failure) {
      if (alive.current) setError(failure instanceof Error ? failure.message : String(failure))
    } finally { if (alive.current) setBusy(false) }
  }
  const move = (index: number, direction: number) => {
    if (!view) return
    const rules = [...view.config.rules]
    const target = index + direction
    if (target < 0 || target >= rules.length) return
    ;[rules[index], rules[target]] = [rules[target], rules[index]]
    void save({ ...view.config, rules })
  }
  const names = view ? new Map([...view.builtins, ...view.config.clients].map(client => [client.id, client.name])) : new Map<string, string>()
  return <Card render={<section />} className='panel'>
    <CardHeader className='panel-head'>
      <h2>{t('客户端识别')}</h2>
      <div className='head-actions flex gap-2'>
        <Button variant='outline' size='sm' disabled={busy || Boolean(editor)} onClick={() => void load()}>{t('刷新')}</Button>
        <Button size='sm' disabled={!view || busy || view.config.rules.length >= view.maxRules} onClick={() => setEditor({})}>{t('添加规则')}</Button>
      </div>
    </CardHeader>
    <CardContent className='panel-body space-y-4'>
      <p className='text-xs text-muted-foreground'>{t('按每条请求的软件标识识别客户端，不依赖 API Key。规则修改只影响新请求，内置规则随软件更新，自定义规则参与备份与云同步。')}</p>
      {error ? <Alert variant='destructive'>{error}</Alert> : null}
      {!view ? <p className='text-sm text-muted-foreground'>{error ? t('客户端识别设置读取失败') : t('加载中…')}</p> : <>
        <div className='flex items-center justify-between gap-4'>
          <Label>{t('接受调用方来源声明')}</Label>
          <Switch checked={view.config.acceptDeclarations} disabled={busy} aria-label={t('接受调用方来源声明')} onCheckedChange={enabled => {
            if (!enabled) { void save({ ...view.config, acceptDeclarations: false }); return }
            setConfirmation({ revision: view.revision, title: t('开启来源声明'), message: t('开启后，调用方可以用专用请求头声明已登记的客户端名称。这不是身份认证，只用于日志和统计，不会获得额外权限。'), config: { ...view.config, acceptDeclarations: true } })
          }} />
        </div>
        <p className='break-all text-xs text-muted-foreground'>{view.declarationHeader}: hermes</p>
        <p className='text-xs text-muted-foreground'>{t('生效顺序：有效的来源声明优先，然后按顺序匹配自定义规则，最后使用内置规则。中转丢失原始标识时，无法自动推断背后的软件。')}</p>
        <div className='space-y-2'>
          <h3 className='text-sm font-medium'>{t('自定义识别规则')}</h3>
          {!view.config.rules.length ? <p className='text-xs text-muted-foreground'>{t('暂无自定义规则，正在使用内置识别。也可以从请求详情直接添加规则。')}</p> : null}
          {view.config.rules.map((rule, index) => <div key={rule.id} className='flex flex-wrap items-center gap-2 rounded-lg border border-hairline p-3'>
            <Switch checked={rule.enabled} disabled={busy} aria-label={t('启用规则')} onCheckedChange={enabled => void save({ ...view.config, rules: view.config.rules.map(item => item.id === rule.id ? { ...item, enabled } : item) })} />
            <div className='min-w-0 flex-1 basis-40'>
              <div className='break-all text-sm'>{names.get(rule.clientId) || rule.clientId}</div>
              <div className='break-all text-xs text-muted-foreground'>{rule.header} · {rule.matchType === 'product' ? t('产品名') : rule.matchType === 'prefix' ? t('前缀') : rule.matchType === 'contains' ? t('包含') : t('完全相同')} · {rule.pattern}</div>
            </div>
            <div className='flex flex-wrap gap-1'>
              <Button size='sm' variant='ghost' disabled={busy || index === 0} onClick={() => move(index, -1)}>{t('上移')}</Button>
              <Button size='sm' variant='ghost' disabled={busy || index === view.config.rules.length - 1} onClick={() => move(index, 1)}>{t('下移')}</Button>
              <Button size='sm' variant='outline' disabled={busy} onClick={() => setEditor({ rule })}>{t('编辑')}</Button>
              <Button size='sm' variant='ghost' disabled={busy} onClick={() => setConfirmation({ revision: view.revision, title: t('删除识别规则'), message: t('删除后，新请求不再使用这条规则；历史记录保持不变。'), config: { ...view.config, rules: view.config.rules.filter(item => item.id !== rule.id) } })}>{t('删除')}</Button>
            </div>
          </div>)}
        </div>
        {view.config.clients.length ? <div className='space-y-2'>
          <h3 className='text-sm font-medium'>{t('自定义客户端分类')}</h3>
          <p className='text-xs text-muted-foreground'>{t('重命名只更新分类显示名，不拆分历史用量，也不重新归类历史请求。')}</p>
          {view.config.clients.map(client => <div key={client.id} className='flex items-center gap-2'>
            <span className='min-w-0 flex-1 break-all text-sm'>{client.name}</span>
            <Button variant='outline' size='sm' disabled={busy} onClick={() => setRename({ ...client })}>{t('重命名')}</Button>
            <Button variant='ghost' size='sm' disabled={busy || view.config.rules.some(rule => rule.clientId === client.id)} onClick={() => setConfirmation({ revision: view.revision, title: t('删除客户端分类'), message: t('删除后不再接受这个分类的来源声明，历史记录保留原来的名称和用量。'), config: { ...view.config, clients: view.config.clients.filter(item => item.id !== client.id) } })}>{t('删除')}</Button>
          </div>)}
        </div> : null}
        <Collapsible>
          <CollapsibleTrigger render={<Button variant='outline' size='sm' />}>{t('查看内置客户端（{n}）', { n: view.builtins.length })}</CollapsibleTrigger>
          <CollapsibleContent className='pt-3'><div className='grid gap-2 sm:grid-cols-2'>
            {view.builtins.map(client => <div key={client.id} className='rounded-lg border border-hairline p-2 text-xs'>
              <Badge variant='outline'>{client.name}</Badge><div className='mt-1 break-all text-muted-foreground'>{client.products.join(' / ')}</div>
            </div>)}
          </div></CollapsibleContent>
        </Collapsible>
      </>}
    </CardContent>
    {editor && view ? <ClientRuleEditor api={api} view={view} rule={editor.rule} t={t} onClose={() => setEditor(null)} onSaved={next => { setView(next); setEditor(null) }} /> : null}
    <Dialog open={Boolean(rename)} onOpenChange={open => { if (!open && !busy) setRename(null) }}><DialogContent>
      <DialogHeader><DialogTitle>{t('重命名客户端分类')}</DialogTitle></DialogHeader>
      <DialogBody><Input value={rename?.name || ''} maxLength={80} disabled={busy} onChange={event => setRename(previous => previous ? { ...previous, name: event.currentTarget.value } : null)} aria-label={t('客户端名称')} />{error ? <Alert variant='destructive'>{error}</Alert> : null}</DialogBody>
      <DialogFooter><Button variant='outline' disabled={busy} onClick={() => setRename(null)}>{t('取消')}</Button><Button disabled={busy || !rename?.name.trim()} onClick={() => { if (view && rename) void save({ ...view.config, clients: view.config.clients.map(client => client.id === rename.id ? { ...client, name: rename.name.trim() } : client) }) }}>{t('保存')}</Button></DialogFooter>
    </DialogContent></Dialog>
    <Dialog open={Boolean(confirmation)} onOpenChange={open => { if (!open && !busy) setConfirmation(null) }}><DialogContent>
      <DialogHeader><DialogTitle>{confirmation?.title}</DialogTitle></DialogHeader>
      <DialogBody><p>{confirmation?.message}</p>{error ? <Alert variant='destructive'>{error}</Alert> : null}</DialogBody>
      <DialogFooter><Button variant='outline' disabled={busy} onClick={() => setConfirmation(null)}>{t('取消')}</Button><Button disabled={busy} onClick={() => { if (confirmation) void save(confirmation.config, confirmation.revision) }}>{t('确认')}</Button></DialogFooter>
    </DialogContent></Dialog>
  </Card>
}
