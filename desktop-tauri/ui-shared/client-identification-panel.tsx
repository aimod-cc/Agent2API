import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react'
import {
  Alert, Button, Card, CardContent, CardHeader, Collapsible, CollapsibleContent, CollapsibleTrigger, StatusDot,
  Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, Input, SearchSelect, Switch,
  Table, TableBody, TableCell, TableHead, TableHeader, TableRow, cn,
} from '@ui'
import type { Translate } from './assistant-read-types'
import { ClientRuleEditor } from './client-identification-editor'
import {
  clientSettingsError, IDENTIFICATION_CHANGED, notifyIdentificationChanged,
  randomId, type ClientIdentificationApi, type ClientIdentificationView, type ClientRule, type ClientSettings, type ProviderRule,
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
 *   · 序号、计数、客户端标识一律等宽 + 次级灰；整个面板只留「新增」一处主色
 *
 * ── 数据口径 ─────────────────────────────────────────────────
 * `api.load / save / preview` 三个调用，保存带 revision 做并发校验，改完派
 * `IDENTIFICATION_CHANGED` 让设置页别处跟着刷新。两个规则层各管一段：
 * 「头匹配规则」按请求头识别真实客户端（顺序即匹配优先级，上移 / 下移直接
 * 落盘），「供应商归属规则」在记账时按路由供应商改写最终归属，未命中回落
 * 默认归属客户端。
 */

/** 头匹配规则的匹配方式 → 界面文案。四种取值各自写死字面量，方便 i18n 扫描器取键 */
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

/** 折叠入口上的小箭头：跟随外层 `panel-fold` 折叠组的开合状态旋转 */
function FoldChevron() {
  return <svg viewBox='0 0 24 24' width={15} height={15} aria-hidden='true' fill='none' stroke='currentColor'
    strokeWidth='1.8' strokeLinecap='round' strokeLinejoin='round'
    className='flex-none text-muted-foreground transition-transform duration-[150ms] group-data-open/fold:rotate-90'>
    <path d='M10 6.5 15.5 12 10 17.5' />
  </svg>
}

type Confirmation = { title: string; message: string; config: ClientSettings; revision: string }

export function ClientIdentificationPanel({ api, t, active = true }: { api: ClientIdentificationApi; t: Translate; active?: boolean }) {
  const [view, setView] = useState<ClientIdentificationView | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [editor, setEditor] = useState<{ rule?: ClientRule } | null>(null)
  /** 新增行的本地草稿：两列都选好才提交保存，中途取消即丢弃 */
  const [draftRule, setDraftRule] = useState<ProviderRule | null>(null)
  /** 新增自定义客户端分类的小弹窗 */
  const [creating, setCreating] = useState(false)
  const [newName, setNewName] = useState('')
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
  const save = async (config: ClientSettings, revision = view?.revision): Promise<boolean> => {
    if (!view || busy || !revision) return false
    const invalid = clientSettingsError(config, view, t)
    if (invalid) { setError(invalid); return false }
    ++seq.current; setBusy(true); setError('')
    try {
      const next = await api.save({ config, revision })
      notifyIdentificationChanged()
      if (alive.current) { setView(next); setRename(null); setConfirmation(null) }
      return true
    } catch (failure) {
      if (alive.current) setError(failure instanceof Error ? failure.message : String(failure))
      return false
    } finally { if (alive.current) setBusy(false) }
  }
  // 供应商归属规则：提交行（配置里）即时保存；新增行先做本地草稿，两列都选好才落盘
  const updateProviderRule = (index: number, patch: Partial<ProviderRule>) => {
    if (!view) return
    void save({ ...view.config, providerRules: view.config.providerRules.map((rule, i) => i === index ? { ...rule, ...patch } : rule) })
  }
  const removeProviderRule = (index: number) => {
    if (!view) return
    void save({ ...view.config, providerRules: view.config.providerRules.filter((_, i) => i !== index) })
  }
  const commitDraftRule = async (rule: ProviderRule) => {
    if (!view) return
    const ok = await save({ ...view.config, providerRules: [...view.config.providerRules, { ...rule, provider: rule.provider.trim() }] })
    if (ok) setDraftRule(null)
  }
  const addCustomClient = async (): Promise<boolean> => {
    if (!view) return false
    return await save({ ...view.config, clients: [...view.config.clients, { id: `custom:${randomId()}`, name: newName.trim() }] })
  }
  const move = (index: number, direction: number) => {
    if (!view) return
    const rules = [...view.config.rules]
    const target = index + direction
    if (target < 0 || target >= rules.length) return
    ;[rules[index], rules[target]] = [rules[target], rules[index]]
    void save({ ...view.config, rules })
  }
  // 两列下拉的候选：内置家在前、自定义家带「自定义 · 」前缀居后（与模型管理左栏同一套词汇）
  const providerOptions = (view?.providers || []).map(provider => ({
    value: provider.id, label: provider.custom ? t('自定义 · {name}', { name: provider.label }) : provider.label,
  }))
  const clientOptions = view ? [
    ...view.builtins.map(client => ({ value: client.id, label: client.name })),
    ...view.config.clients.map(client => ({ value: client.id, label: client.name })),
  ] : []
  const rules = view?.config.rules || []
  const names = view ? new Map([...view.builtins, ...view.config.clients].map(client => [client.id, client.name])) : new Map<string, string>()
  return <Collapsible render={<Card />} className='panel panel-fold group/fold'>
    <CardHeader className='panel-head'>
      <CollapsibleTrigger className='h-auto flex-1 justify-start gap-2 px-0 text-left'>
        <FoldChevron />
        <h2>{t('客户端识别')}</h2>
      </CollapsibleTrigger>
      {view ? <span className='flex items-center gap-1.5 text-[12px] text-muted-foreground'>
        <StatusDot variant={view.config.providerRules.length || view.config.defaultClientId ? 'success' : 'muted'} />
        {view.config.providerRules.length
          ? t('{n} 条', { n: view.config.providerRules.length })
          : view.config.defaultClientId ? t('默认归属客户端') : t('未设置')}
      </span> : null}
      <div className='head-actions flex gap-2'>
        <Button variant='ghost' size='sm' disabled={busy} onClick={() => void load()}>{t('刷新')}</Button>
      </div>
    </CardHeader>
    <CollapsibleContent keepMounted>
      <CardContent className='panel-body bg-rail-bg'>
      <p className='max-w-[78ch] text-[12.5px] leading-[1.7] text-muted-foreground'>{t('按每条请求的软件标识识别真实客户端，不依赖 API Key；归属规则再按路由供应商把请求改写到所选客户端，未命中时用默认归属客户端。修改只影响新请求，内置规则随软件更新，自定义数据参与备份与云同步。')}</p>
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
              <PriorityRow index={1} title={t('供应商归属规则')} note={view.config.providerRules.length ? t('{n} 条', { n: view.config.providerRules.length }) : t('未设置')} />
              <PriorityRow index={2} title={t('默认归属客户端')} note={view.config.defaultClientId ? t('已设置') : t('未设置')} dim={!view.config.defaultClientId} />
              <PriorityRow index={3} title={t('来源声明')} note={view.config.acceptDeclarations ? t('已开启') : t('已关闭')} dim={!view.config.acceptDeclarations} />
              <PriorityRow index={4} title={t('自定义头匹配规则')} note={t('按列表自上而下匹配')} />
              <PriorityRow index={5} title={t('内置规则')} note={t('随软件更新')} />
              <p className='px-4 py-3 text-[12px] leading-[1.7] text-muted-foreground'>{t('按列表顺序决定最终归属：命中供应商规则用规则客户端，未命中用默认归属客户端；两者都未设置时，回落到按请求头识别的真实客户端。')}</p>
            </Section>

            <Section title={t('供应商归属规则')} note={view.config.providerRules.length ? t('{n} 条', { n: view.config.providerRules.length }) : undefined}>
              <div className='flex flex-wrap items-center gap-x-3 gap-y-2 px-4 py-3'>
                <span className='text-[13px]'>{t('默认归属客户端')}</span>
                <span className='text-[12px] text-muted-foreground'>{t('未命中供应商规则的请求统一归属到它')}</span>
                <div className='ml-auto flex items-center gap-1'>
                  <SearchSelect value={view.config.defaultClientId} options={clientOptions} disabled={busy}
                    placeholder={t('未设置')} searchPlaceholder={t('搜索客户端')} aria-label={t('默认归属客户端')}
                    className='w-[min(240px,100%)]'
                    onValueChange={value => { if (view) void save({ ...view.config, defaultClientId: value }) }} />
                  {view.config.defaultClientId ? <Button size='xs' variant='ghost' disabled={busy} onClick={() => { if (view) void save({ ...view.config, defaultClientId: '' }) }}>{t('清除')}</Button> : null}
                </div>
              </div>
              {view.config.providerRules.map((rule, index) => <div key={`${rule.provider}:${index}`} className='grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto] items-center gap-2 px-4 py-2.5'>
                <SearchSelect value={rule.provider} options={providerOptions} disabled={busy} aria-label={t('供应商')}
                  placeholder={t('选择供应商')} searchPlaceholder={t('搜索供应商')} className='min-w-0'
                  onValueChange={value => updateProviderRule(index, { provider: value })} />
                <SearchSelect value={rule.clientId} options={clientOptions} disabled={busy} aria-label={t('归属客户端')}
                  placeholder={t('选择归属客户端')} searchPlaceholder={t('搜索客户端')} className='min-w-0'
                  onValueChange={value => updateProviderRule(index, { clientId: value })} />
                <Button size='xs' variant='ghost' disabled={busy} onClick={() => removeProviderRule(index)}>{t('删除')}</Button>
              </div>)}
              {draftRule ? <div className='grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto] items-center gap-2 px-4 py-2.5'>
                <SearchSelect value={draftRule.provider} options={providerOptions} disabled={busy} aria-label={t('供应商')}
                  placeholder={t('选择供应商')} searchPlaceholder={t('搜索供应商')} className='min-w-0'
                  onValueChange={value => {
                    const next = { ...draftRule, provider: value }
                    if (next.clientId) { void commitDraftRule(next) } else { setDraftRule(next) }
                  }} />
                <SearchSelect value={draftRule.clientId} options={clientOptions} disabled={busy} aria-label={t('归属客户端')}
                  placeholder={t('选择归属客户端')} searchPlaceholder={t('搜索客户端')} className='min-w-0'
                  onValueChange={value => {
                    const next = { ...draftRule, clientId: value }
                    if (next.provider.trim()) { void commitDraftRule(next) } else { setDraftRule(next) }
                  }} />
                <Button size='xs' variant='ghost' disabled={busy} onClick={() => setDraftRule(null)}>{t('取消')}</Button>
              </div> : null}
              <div className='flex flex-wrap items-center justify-between gap-3 px-4 py-3'>
                <p className='m-0 max-w-[64ch] text-[12px] leading-[1.7] text-muted-foreground'>{t('选中的供应商，其请求在记账时归属到所选客户端；请求头识别的真实客户端保留为依据。')}</p>
                <Button size='sm' variant='outline' disabled={busy || Boolean(draftRule) || view.config.providerRules.length >= view.maxProviderRules}
                  onClick={() => setDraftRule({ provider: '', clientId: '' })}>{t('新增规则')}</Button>
              </div>
            </Section>

            <Section title={t('自定义头匹配规则')} note={rules.length > 0 ? t('{n} 条', { n: rules.length }) : undefined}>
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
              <div className='flex flex-wrap items-center justify-between gap-3 px-4 py-3'>
                <p className='m-0 max-w-[64ch] text-[12px] leading-[1.7] text-muted-foreground'>{t('按列表自上而下匹配，位于来源声明之后、内置规则之前；可用于给难认的标识钉规则。')}</p>
                <Button size='sm' variant='outline' disabled={!view || busy || view.config.rules.length >= view.maxRules}
                  onClick={() => setEditor({})}>{t('添加规则')}</Button>
              </div>
            </Section>

            <Section title={t('自定义客户端分类')} note={view.config.clients.length ? t('{n} 个', { n: view.config.clients.length }) : undefined}>
              <div className='flex flex-wrap items-center justify-between gap-3 px-4 py-3'>
                <p className='m-0 max-w-[64ch] text-[12px] leading-[1.7] text-muted-foreground'>{t('重命名只更新分类显示名，不拆分历史用量，也不重新归类历史请求。')}</p>
                <Button size='sm' variant='outline' disabled={busy || view.config.clients.length >= view.maxClients}
                  onClick={() => { setNewName(''); setCreating(true) }}>{t('新增客户端')}</Button>
              </div>
              {!view.config.clients.length ? <p className='px-4 pb-4 text-[12px] text-muted-foreground'>{t('暂无自定义客户端分类，用「新增客户端」创建后即可在归属规则里选择。')}</p>
                : view.config.clients.map(client => <div key={client.id} className='flex flex-wrap items-center gap-x-3 gap-y-2 px-4 py-2.5'>
                  <span className='min-w-0 flex-1 basis-40 truncate text-[13px]'>{client.name}</span>
                  <div className='flex shrink-0 items-center gap-0.5'>
                    <Button variant='ghost' size='xs' disabled={busy} onClick={() => setRename({ ...client })}>{t('重命名')}</Button>
                    <Button variant='ghost' size='xs' disabled={busy || view.config.rules.some(rule => rule.clientId === client.id)
                      || view.config.providerRules.some(rule => rule.clientId === client.id) || view.config.defaultClientId === client.id}
                      onClick={() => setConfirmation({ revision: view.revision, title: t('删除客户端分类'), message: t('删除后不再接受这个分类的来源声明，历史记录保留原来的名称和用量。'), config: { ...view.config, clients: view.config.clients.filter(item => item.id !== client.id) } })}>{t('删除')}</Button>
                  </div>
                </div>)}
            </Section>

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
    </CollapsibleContent>
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
    <Dialog open={creating} onOpenChange={open => { if (!open && !busy) setCreating(false) }}><DialogContent className='w-[min(480px,calc(100vw-24px))]'>
      <DialogHeader><DialogTitle>{t('新增客户端分类')}</DialogTitle></DialogHeader>
      <DialogBody><Input value={newName} maxLength={80} disabled={busy} onChange={event => setNewName(event.currentTarget.value)}
        onKeyDown={event => { if (event.key === 'Enter' && newName.trim()) void addCustomClient().then(ok => { if (ok) setCreating(false) }) }}
        aria-label={t('客户端名称')} placeholder={t('客户端名称')} />{error ? <Alert variant='destructive'>{error}</Alert> : null}</DialogBody>
      <DialogFooter><Button variant='ghost' disabled={busy} onClick={() => setCreating(false)}>{t('取消')}</Button>
        <Button disabled={busy || !newName.trim()} onClick={() => void addCustomClient().then(ok => { if (ok) setCreating(false) })}>{t('保存')}</Button></DialogFooter>
    </DialogContent></Dialog>
  </Collapsible>
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
