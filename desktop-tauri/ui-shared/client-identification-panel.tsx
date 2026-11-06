import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react'
import {
  Alert, Button, Card, CardContent, CardHeader, Collapsible, CollapsibleContent, CollapsibleTrigger,
  Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, Input, Switch, Table, TableBody,
  TableCell, TableHead, TableHeader, TableRow, cn,
} from '@ui'
import type { Translate } from './assistant-read-types'
import { ClientRuleEditor } from './client-identification-editor'
import {
  clientSettingsError, IDENTIFICATION_CHANGED, notifyIdentificationChanged,
  type ClientIdentificationApi, type ClientIdentificationView, type ClientRule, type ClientSettings,
} from './client-identification'

/**
 * 「设置 → 网关 → 客户端识别」。
 *
 * ── 版式（2026-11 改版）──────────────────────────────────────
 * 参照系统「设置」那种分组列表：面板里铺一层灰画布，内容用**白底分组卡**分块，
 * 卡内每行之间只有一条 1px 细线；「哪里是一组」由两层底色 + 留白表达，线条只留在
 * 卡片内部当行分隔。改版前是「一段段说明文字 + 每个元素各描一圈边」，
 * 宽屏时右半屏整片空着、开关离说明隔了半屏；现在右侧放「内置客户端」目录，
 * 左栏内容限宽 720px。
 *
 *   · 画布 = --ui-rail-bg（浅色 0.955 灰 / 深色 0.24 灰），卡片 = --ui-surface
 *     （浅色纯白 / 深色 0.192）—— 两套主题都是稳定的「浅一层 / 深一层」关系，
 *     所以不需要为深浅各写一份样式
 *   · 分组 = 13px 半粗标题 + 一张**无描边**圆角卡（底色已分层，再描边就是双线）；
 *     行高统一 px-4，行内左开关、中名称、右侧状态或操作
 *   · 行分隔线用 --ui-hairline，且左右各内缩 16px（不贴卡片边缘），像系统列表
 *   · 序号、计数、客户端标识一律等宽 + 次级灰；整个面板只留「添加规则」一处主色
 *
 * ── 数据口径 ─────────────────────────────────────────────────
 * 与旧版完全一致：`api.load / save / preview` 三个调用，保存带 revision 做并发校验，
 * 规则顺序即匹配优先级（上移 / 下移直接落盘），改完派 `IDENTIFICATION_CHANGED`
 * 让设置页别处跟着刷新。区别只在于「怎么看」，不在「怎么算」。
 */

/** 识别规则的匹配方式 → 界面文案。四种取值各自写死字面量，方便 i18n 扫描器取键 */
function matchLabel(t: Translate, matchType: ClientRule['matchType']): string {
  switch (matchType) {
    case 'product': return t('产品名')
    case 'prefix': return t('前缀')
    case 'contains': return t('包含')
    default: return t('完全相同')
  }
}

/** 等宽小条（请求头名 / 匹配内容）：只比卡片深一档的底，不加边框 */
const CHIP = 'inline-block break-all rounded-xs bg-surface-3 px-1.5 py-px font-mono text-[11.5px] text-subtle'

type Confirmation = { title: string; message: string; config: ClientSettings; revision: string }

export function ClientIdentificationPanel({ api, t, active = true }: { api: ClientIdentificationApi; t: Translate; active?: boolean }) {
  const [view, setView] = useState<ClientIdentificationView | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [editor, setEditor] = useState<{ rule?: ClientRule } | null>(null)
  const [rename, setRename] = useState<{ id: string; name: string } | null>(null)
  const [confirmation, setConfirmation] = useState<Confirmation | null>(null)
  /** 内置客户端目录默认展开：它就是这条面板的参考信息，收起后右栏会重新空掉 */
  const [directoryOpen, setDirectoryOpen] = useState(true)
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
  const rules = view?.config.rules || []
  return <Card render={<section />} className='panel'>
    <CardHeader className='panel-head'>
      <h2>{t('客户端识别')}</h2>
      <div className='head-actions flex gap-2'>
        <Button variant='ghost' size='sm' disabled={busy || Boolean(editor)} onClick={() => void load()}>{t('刷新')}</Button>
        <Button size='sm' disabled={!view || busy || view.config.rules.length >= view.maxRules} onClick={() => setEditor({})}>{t('添加规则')}</Button>
      </div>
    </CardHeader>
    <CardContent className='panel-body bg-rail-bg'>
      <p className='max-w-[78ch] text-[12.5px] leading-[1.7] text-muted-foreground'>{t('按每条请求的软件标识识别客户端，不依赖 API Key。规则修改只影响新请求，内置规则随软件更新，自定义规则参与备份与云同步。')}</p>
      {error ? <Alert variant='destructive'>{error}</Alert> : null}
      {!view ? <p className='text-[13px] text-muted-foreground'>{error ? t('客户端识别设置读取失败') : t('加载中…')}</p> : <>
        <div className='grid grid-cols-1 gap-6 xl:grid-cols-[minmax(0,1fr)_minmax(340px,500px)]'>
          {/* ── 左栏：识别设置（限宽 720px：宽屏下面板能到 1500px 以上，
                 不限宽的话「规则名 → 右侧操作」会隔出近千像素的空白）── */}
          <div className='flex min-w-0 max-w-[720px] flex-col gap-6'>
            <Section title={t('来源声明')}>
              <div className='flex items-center gap-3 px-4 py-3'>
                <Switch checked={view.config.acceptDeclarations} disabled={busy} aria-label={t('接受调用方来源声明')} onCheckedChange={enabled => {
                  if (!enabled) { void save({ ...view.config, acceptDeclarations: false }); return }
                  setConfirmation({ revision: view.revision, title: t('开启来源声明'), message: t('开启后，调用方可以用专用请求头声明已登记的客户端名称。这不是身份认证，只用于日志和统计，不会获得额外权限。'), config: { ...view.config, acceptDeclarations: true } })
                }} />
                <span className='text-[13px]'>{t('接受调用方来源声明')}</span>
                <span className='ml-auto text-[12px] text-muted-foreground'>{view.config.acceptDeclarations ? t('已开启') : t('已关闭')}</span>
              </div>
              <div className='px-4 py-3'>
                <p className='max-w-[64ch] text-[12px] leading-[1.7] text-muted-foreground'>{t('开启后，调用方可以用专用请求头声明已登记的客户端名称。这不是身份认证，只用于日志和统计，不会获得额外权限。')}</p>
                <div className='mt-2.5 flex flex-wrap items-center gap-2 text-[12px] text-muted-foreground'>
                  <span>{t('示例请求头')}</span>
                  <code className={CHIP}>{view.declarationHeader}: hermes</code>
                </div>
              </div>
            </Section>

            <Section title={t('识别顺序')}>
              <PriorityRow index={1} title={t('来源声明')} note={view.config.acceptDeclarations ? t('已开启') : t('已关闭')} dim={!view.config.acceptDeclarations} />
              <PriorityRow index={2} title={t('自定义识别规则')} note={t('按列表自上而下匹配')} />
              <PriorityRow index={3} title={t('内置规则')} note={t('随软件更新')} />
            </Section>

            <Section title={t('自定义识别规则')} note={rules.length > 0 ? t('{n} 条', { n: rules.length }) : undefined}>
              {!rules.length
                ? <div className='px-4 py-8 text-center'>
                  <p className='text-[12.5px] leading-[1.7] text-muted-foreground'>{t('暂无自定义规则，正在使用内置识别。也可以从请求详情直接添加规则。')}</p>
                </div>
                : rules.map((rule, index) => <div key={rule.id} className='flex flex-wrap items-center gap-x-3 gap-y-2 px-4 py-2.5'>
                  <Switch checked={rule.enabled} disabled={busy} aria-label={t('启用规则')} onCheckedChange={enabled => void save({ ...view.config, rules: view.config.rules.map(item => item.id === rule.id ? { ...item, enabled } : item) })} />
                  <div className='min-w-0 flex-1 basis-52'>
                    <div className='truncate text-[13px]' title={names.get(rule.clientId) || rule.clientId}>{names.get(rule.clientId) || rule.clientId}</div>
                    <div className='mt-0.5 flex flex-wrap items-center gap-x-1.5 gap-y-1 text-[12px] text-muted-foreground'>
                      <code className={CHIP}>{rule.header}</code>
                      <span>{matchLabel(t, rule.matchType)}</span>
                      <code className={CHIP}>{rule.pattern}</code>
                    </div>
                  </div>
                  <div className='flex shrink-0 items-center gap-0.5'>
                    <Button size='xs' variant='ghost' disabled={busy || index === 0} onClick={() => move(index, -1)}>{t('上移')}</Button>
                    <Button size='xs' variant='ghost' disabled={busy || index === rules.length - 1} onClick={() => move(index, 1)}>{t('下移')}</Button>
                    <Button size='xs' variant='ghost' disabled={busy} onClick={() => setEditor({ rule })}>{t('编辑')}</Button>
                    <Button size='xs' variant='ghost' disabled={busy} onClick={() => setConfirmation({ revision: view.revision, title: t('删除识别规则'), message: t('删除后，新请求不再使用这条规则；历史记录保持不变。'), config: { ...view.config, rules: view.config.rules.filter(item => item.id !== rule.id) } })}>{t('删除')}</Button>
                  </div>
                </div>)}
            </Section>

            {view.config.clients.length ? <Section title={t('自定义客户端分类')}>
              <div className='px-4 py-3'>
                <p className='max-w-[64ch] text-[12px] leading-[1.7] text-muted-foreground'>{t('重命名只更新分类显示名，不拆分历史用量，也不重新归类历史请求。')}</p>
              </div>
              {view.config.clients.map(client => <div key={client.id} className='flex flex-wrap items-center gap-x-3 gap-y-2 px-4 py-2.5'>
                <span className='min-w-0 flex-1 basis-40 truncate text-[13px]'>{client.name}</span>
                <div className='flex shrink-0 items-center gap-0.5'>
                  <Button variant='ghost' size='xs' disabled={busy} onClick={() => setRename({ ...client })}>{t('重命名')}</Button>
                  <Button variant='ghost' size='xs' disabled={busy || view.config.rules.some(rule => rule.clientId === client.id)} onClick={() => setConfirmation({ revision: view.revision, title: t('删除客户端分类'), message: t('删除后不再接受这个分类的来源声明，历史记录保留原来的名称和用量。'), config: { ...view.config, clients: view.config.clients.filter(item => item.id !== client.id) } })}>{t('删除')}</Button>
                </div>
              </div>)}
            </Section> : null}

            <p className='text-[12px] leading-[1.7] text-muted-foreground'>{t('中转丢失原始标识时，无法自动推断背后的软件。')}</p>
          </div>

          {/* ── 右栏：内置客户端目录 ── */}
          <Collapsible open={directoryOpen} onOpenChange={setDirectoryOpen}
            render={<section className='flex min-w-0 flex-col gap-2 self-start xl:col-start-2 xl:row-start-1' />}>
            <div className='flex items-baseline gap-2'>
              <h3 className='text-[13px] font-semibold'>{t('内置客户端')}</h3>
              <span className='text-[12px] tabular-nums text-muted-foreground'>{view.builtins.length}</span>
              <CollapsibleTrigger render={<Button variant='ghost' size='xs' className='ml-auto' />}>
                {directoryOpen ? t('收起') : t('查看内置客户端（{n}）', { n: view.builtins.length })}
              </CollapsibleTrigger>
            </div>
            <div className='overflow-hidden rounded-lg bg-surface'>
              <div className='flex flex-col gap-2 px-4 py-3'>
                <p className='text-[12px] leading-[1.7] text-muted-foreground'>{t('这些名称可以直接写进来源声明，不能编辑。')}</p>
                <CollapsibleContent className='border-t border-hairline pt-2.5'>
                  {/* 目录不再套一层描边框：卡片就是它的外框；表头吸顶用的底色与卡片一致，
                      滚动时不会出现一道色带 */}
                  <div className='max-h-[420px] overflow-y-auto overscroll-contain'>
                    <Table container={false}>
                      <TableHeader>
                        <TableRow>
                          <TableHead className='w-[128px]'>{t('客户端')}</TableHead>
                          <TableHead>{t('可识别标识')}</TableHead>
                        </TableRow>
                      </TableHeader>
                      <TableBody>
                        {view.builtins.map(client => <TableRow key={client.id}>
                          <TableCell className='align-top text-[13px] whitespace-normal'>{client.name}</TableCell>
                          <TableCell className='align-top font-mono text-[11px] leading-relaxed break-all whitespace-normal text-muted-foreground'>{client.products.join(' / ')}</TableCell>
                        </TableRow>)}
                      </TableBody>
                    </Table>
                  </div>
                </CollapsibleContent>
              </div>
            </div>
          </Collapsible>
        </div>
      </>}
    </CardContent>
    {editor && view ? <ClientRuleEditor api={api} view={view} rule={editor.rule} t={t} onClose={() => setEditor(null)} onSaved={next => { setView(next); setEditor(null) }} /> : null}
    <Dialog open={Boolean(rename)} onOpenChange={open => { if (!open && !busy) setRename(null) }}><DialogContent>
      <DialogHeader><DialogTitle>{t('重命名客户端分类')}</DialogTitle></DialogHeader>
      <DialogBody><Input value={rename?.name || ''} maxLength={80} disabled={busy} onChange={event => setRename(previous => previous ? { ...previous, name: event.currentTarget.value } : null)} aria-label={t('客户端名称')} />{error ? <Alert variant='destructive'>{error}</Alert> : null}</DialogBody>
      <DialogFooter><Button variant='ghost' disabled={busy} onClick={() => setRename(null)}>{t('取消')}</Button><Button disabled={busy || !rename?.name.trim()} onClick={() => { if (view && rename) void save({ ...view.config, clients: view.config.clients.map(client => client.id === rename.id ? { ...client, name: rename.name.trim() } : client) }) }}>{t('保存')}</Button></DialogFooter>
    </DialogContent></Dialog>
    <Dialog open={Boolean(confirmation)} onOpenChange={open => { if (!open && !busy) setConfirmation(null) }}><DialogContent>
      <DialogHeader><DialogTitle>{confirmation?.title}</DialogTitle></DialogHeader>
      <DialogBody><p className='text-[13px] leading-[1.7] text-muted-foreground'>{confirmation?.message}</p>{error ? <Alert variant='destructive'>{error}</Alert> : null}</DialogBody>
      <DialogFooter><Button variant='ghost' disabled={busy} onClick={() => setConfirmation(null)}>{t('取消')}</Button><Button disabled={busy} onClick={() => { if (confirmation) void save(confirmation.config, confirmation.revision) }}>{t('确认')}</Button></DialogFooter>
    </DialogContent></Dialog>
  </Card>
}

/**
 * 分组：13px 半粗标题 + 一张无描边的白底圆角卡。
 * 卡内每一行自己负责左右内边距（px-4），分组只提供分隔线 —— 用 divide-y
 * 而不是给每行描边：底色已经把「卡内 / 卡外」分开了，边界只留一条细线。
 */
function Section({ title, note, children }: { title: string; note?: string; children: ReactNode }) {
  return <section className={cn('flex flex-col gap-2')}>
    <div className='flex items-baseline gap-2'>
      <h3 className='text-[13px] font-semibold'>{title}</h3>
      {note ? <span className='text-[12px] tabular-nums text-muted-foreground'>{note}</span> : null}
    </div>
    <div className='divide-y divide-hairline overflow-hidden rounded-lg bg-surface'>{children}</div>
  </section>
}

/** 识别顺序里的一行：序号列固定宽度，名称在左，说明右对齐 */
function PriorityRow({ index, title, note, dim = false }: { index: number; title: string; note: string; dim?: boolean }) {
  return <div className='flex items-baseline gap-3 px-4 py-2.5'>
    <span className='w-3.5 flex-none text-right text-[12px] tabular-nums text-muted-foreground'>{index}</span>
    <span className={dim ? 'text-[13px] text-muted-foreground' : 'text-[13px]'}>{title}</span>
    <span className='ml-auto text-[12px] text-muted-foreground'>{note}</span>
  </div>
}
