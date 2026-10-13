/**
 * Agent2API · 网络代理页（代理池：列表 / 新增 / 编辑 / 删除 / 启停 / 连通性测试
 * / 同步 Clash Verge / 勾选批量测试与批量删除）。
 *
 * 对照 OmniProxy 的 ProxiesPage（那张带 CRUD + 测试 + Clash 同步 + 批量的代理表），
 * 存储与解析在 Rust 侧：`core::proxy_pool` 是唯一事实来源（`kv` 的 `proxyPool`
 * 键），HTTP 契约见 `api::proxies` 的模块头，桥方法见 `src/bridge.rs` 的
 * 「代理池」一段。
 *
 * ── 数据层在隔壁 ─────────────────────────────────────────────
 * 本文件只放**视图层**（页面骨架 + 表格 + 批量条 + 两个弹窗 + 挂载）；
 * 状态、取数、单条与批量动作都在 `proxies-state.ts`（普通模块，不是岛 ——
 * 文件名是 .ts，不被 src/index.tsx 的 `islands/*.tsx` glob 加载）。
 *
 * ── 这一页解决什么（为什么不能只在账号弹窗里配代理）────────────
 * 改造前代理只能逐个账号配：同一出口用在三个账号上要填三遍、改端口要改三处，
 * 也看不到「这出口通不通、出口 IP 是什么」。现在出口在这里配一次：
 * 账号弹窗的「出网代理」多一档「已保存的代理」，选这里的一条即可。
 *
 * ── 两种条目（只读语义照 OmniProxy）──────────────────────────
 *   · 手动：这里新建 / 编辑 / 删除，字段全在本地；
 *   · Clash Verge 同步来的：**只读镜像** —— 名称 / 端口 / 启用状态全部跟随
 *     Clash，本页不给编辑与删除入口（按钮禁用 + title 说明），要改去 Clash
 *     Verge 改、要删去 Clash Verge 删（下次同步自然摘掉）。这条语义与
 *     OmniProxy 的 `imported_clash_listeners` 分支逐条对齐。
 *     同步有两处触发：进页面时的列表接口（后端自动同步）与右上角
 *     「同步 Clash Verge」按钮（手动，会报本次变更数）。
 *     同步集合包含 Clash 的**混合端口**（「按规则分流」那个入口）—— 与
 *     OmniProxy 的一处有意差异，理由见 `core::proxy_pool` 模块头。
 *
 * ── 勾选与批量（照账号页那一套）────────────────────────────
 * 表头一列勾选框 + 行内勾选框，勾选后在面板头下方浮出一条 `.batch-bar`
 * （类名与账号页共用，见 page-accounts.css）：批量测试、批量删除、取消选择。
 *   · 批量测试**包含**只读的 Clash 条目（读操作不受只读语义限制）；
 *   · 批量删除**跳过** Clash 条目（会拒绝），并在确认框里说明跳过了几条；
 *   · 勾选不随列表刷新清空，只有条目从列表里消失（被删 / Clash 侧删了出口）
 *     才收敛 —— 完整口径见 `proxies-state.ts` 的模块头。
 * 批量逐个执行而不是并发：每条删除都会整份重写池（kv 一个键），并发写会互相
 * 覆盖（丢更新）；测试是真实的上游请求，串行也避免瞬间占满建连额度。
 *
 * ── 测试的判据与账号弹窗同一套 ────────────────────────────────
 * 判据是「能否连上**上游**」（不是能否访问第三方站点）、出口 IP 是附加信息、
 * 失败也返回 200 + `success:false`，见 `core::egress::test_connectivity`。
 * 结果会落进条目（`lastTest`）：重启 / 换设备打开都还能看到上次测出来是什么。
 * 它只是展示用的历史事实，**不参与转发选路**。
 *
 * ── 为什么没接列设置 / 列宽拖动 ────────────────────────────────
 * 那两套（table-col-settings / table-columns.js）服务的表都有 5~11 列、且列
 * 含义随使用场景浮动；本页 7 列都是核心信息，少一列就没法用，列宽也由内容
 * 长度天然定死（名称 / 地址长度相近）。不接省掉两个注册点，页面不因此变差。
 *
 * ── 页面骨架的类名照旧 ────────────────────────────────────────
 * `.panel` / `.panel-head` / `.models-table-wrap` / `.models-table` / `.panel-foot`
 * 都是既有全局样式（layout.css / page-gateway.css），批量条用账号页那组
 * `.batch-bar` 类 —— 这一页没有新增任何 CSS 文件。
 */

import * as React from 'react'
import { createRoot } from 'react-dom/client'
import {
  Badge,
  Button,
  Checkbox,
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Input,
  Label,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Switch,
  cn,
} from '@ui'
import { poolItemAddress } from './accounts-shared'
import {
  CLASH_READONLY_HINT, batchRemove, batchTest, clearSelection, durationText, errorMessage,
  formatTime, getSnapshot, isClashItem, isPicked, items, loadPanel, pickableIds, removeItem,
  saveItem, setAllPicked, subscribe, syncClash, testItem, toggleItem, togglePick, toast, wb,
  type ProxyPoolItem,
} from './proxies-state'
import { t } from '../i18n'

/* ─── 表格列 ─────────────────────────────────── */

/** 列顺序（表头、数据行、空态 colspan 都读它） */
const COLUMNS = ['pick', 'name', 'kind', 'addr', 'state', 'test', 'act'] as const
/** 逐列的对齐（`ta-*` 是既有全局类，见 table-col-settings.css） */
const CELL = ['ta-center', '', '', '', 'ta-center', '', 'ta-right']

/* ─── 页面 ───────────────────────────────────── */

function ProxiesPage() {
  const snapshot = React.useSyncExternalStore(subscribe, getSnapshot)
  const [modal, setModal] = React.useState<{ item: ProxyPoolItem | null } | null>(null)
  const rows = items()
  const ready = snapshot.data !== null
  const selectedCount = snapshot.selected.size
  const selectedIds = pickableIds()
  const allPicked = rows.length > 0 && rows.every(item => snapshot.selected.has(item.id))
  const somePicked = selectedCount > 0 && !allPicked
  const clashPicked = rows.filter(item => snapshot.selected.has(item.id) && isClashItem(item)).length
  const progress = snapshot.batchProgress

  function testCell(item: ProxyPoolItem): React.ReactNode {
    const last = item.lastTest
    if (!last || !last.at) return <span className='text-subtle'>{t('未测试')}</span>
    const suffix = durationText(last.durationMs)
    return (
      <div className='flex flex-col gap-0.5 leading-[1.5]'>
        {/* 失败原因是上游错误原文（后端数据）原样透出；耗时只有数字单位，不包 */}
        <span className={last.success ? 'text-success' : 'text-destructive'}
          title={last.success ? '' : String(last.error || t('连接失败'))}>
          {last.success ? t('✅ {ip}', { ip: last.ip || t('连接成功') }) : t('❌ 连接失败')}
          {suffix ? `　${suffix}` : ''}
        </span>
        <span className='text-[11px] text-subtle'>{formatTime(last.at)}</span>
      </div>
    )
  }

  function cell(column: string, item: ProxyPoolItem, busy: boolean, picked: boolean): React.ReactNode {
    const used = item.usedBy || []
    // 批量在跑时行内一切写入口都停掉：批量测试会把 testing 指向「当前那一条」，
    // 这时用户点编辑 / 删除 / 启停会和正在跑的流程抢同一份数据
    const locked = busy || snapshot.batchBusy
    switch (column) {
      case 'pick':
        return (
          <Checkbox checked={picked} title={t('勾选后可批量操作')}
            aria-label={t('勾选「{name}」', { name: item.name || t('未命名') })}
            onCheckedChange={next => togglePick(item.id, next)} />
        )
      case 'name':
        return (
          <div className='flex flex-col gap-0.5 leading-[1.5]'>
            <b>{item.name || t('未命名')}</b>
            {used.length ? (
              <span className='text-[11px] text-subtle'
                title={used.map(entry => String(entry.name || entry.id || '')).join(t('、'))}>
                {t('{n} 个账号在用', { n: used.length })}
              </span>
            ) : null}
          </div>
        )
      case 'kind':
        return item.source === 'clash' ? (
          <Badge variant='secondary' title={CLASH_READONLY_HINT}>Clash Verge</Badge>
        ) : (
          <Badge title={t('手动填写的地址与端口')}>{t('手动')}</Badge>
        )
      case 'addr':
        // 地址统一成「协议 主机:端口」（与账号侧下拉同一口径，见
        // accounts-shared 的 poolItemAddress）：resolved.label 对 Clash 条目是
        // 「监听器名（:端口）」、对混合端口是「Clash 混合端口 7892」—— 都不含主机，
        // 形态还随来源变。主机与端口在这里是核对「这条连的是哪儿」的唯一读数
        return item.resolveError ? (
          <span className='text-destructive' title={item.resolveError}>{t('解析失败')}</span>
        ) : (
          <span className='font-mono text-[12px] text-text-2'>{poolItemAddress(item) || '—'}</span>
        )
      case 'state': {
        // 只读镜像：开关照常显示真实状态（Clash 里禁用了这里也看得见），但不给改
        const readonly = isClashItem(item)
        return (
          <Switch checked={item.enabled} disabled={locked || readonly}
            aria-label={t('启用「{name}」', { name: item.name || t('未命名') })}
            title={readonly ? CLASH_READONLY_HINT : undefined}
            onCheckedChange={next => void toggleItem(item, next)} />
        )
      }
      case 'test':
        return testCell(item)
      case 'act': {
        const readonly = isClashItem(item)
        return (
          <div className='row-actions'>
            <Button size='sm' variant='ghost' disabled={locked || snapshot.testing !== null}
              onClick={() => void testItem(item)}>
              {snapshot.testing === item.id ? t('测试中…') : t('测试')}
            </Button>
            <Button size='sm' variant='ghost' disabled={locked || readonly}
              title={readonly ? CLASH_READONLY_HINT : undefined}
              onClick={() => setModal({ item })}>
              {t('编辑')}
            </Button>
            <Button size='sm' variant='destructive' disabled={locked || readonly}
              title={readonly ? CLASH_READONLY_HINT : undefined}
              onClick={() => void removeItem(item)}>
              {t('删除')}
            </Button>
          </div>
        )
      }
      default:
        return null
    }
  }

  return (
    <section className='panel'>
      <div className='panel-head'>
        <h2>{t('网络代理')}</h2>
        <span className='panel-sub'>
          {ready ? t('{n} 个出口', { n: rows.length }) : t('加载中…')}
        </span>
        <div className='head-actions'>
          {/* 同步放在刷新左边：它做的事比「刷新」多一层（把 Clash 的出口集合
              镜像进池），而进页面时的那次自动同步是静默的 —— 用户想在
              「我刚在 Clash 里加了出口」之后立刻见效时点它。
              批量在跑时两个都停：它们会把正在被批量流程改写的列表整份换掉 */}
          <Button variant='outline' disabled={snapshot.syncing || snapshot.batchBusy}
            onClick={() => void syncClash()}>
            {snapshot.syncing ? t('同步中…') : t('同步 Clash Verge')}
          </Button>
          <Button variant='outline' disabled={snapshot.loading || snapshot.batchBusy}
            onClick={() => void loadPanel()}>
            {snapshot.loading ? t('刷新中…') : t('刷新')}
          </Button>
          <Button variant='default' disabled={snapshot.batchBusy}
            onClick={() => setModal({ item: null })}>{t('＋ 新增代理')}</Button>
        </div>
      </div>

      {/* 批量条：与账号页同一套类名（page-accounts.css），勾选后整条点亮成主色底 */}
      <div className={cn('batch-bar', selectedCount > 0 && 'active')}>
        <label className='batch-select-all'>
          <Checkbox checked={allPicked} indeterminate={somePicked}
            disabled={!rows.length} aria-label={t('全选所有代理')}
            onCheckedChange={next => setAllPicked(selectedIds, next)} />
          <span>{rows.length ? t('全选（{n} 个）', { n: rows.length }) : t('没有可全选的代理')}</span>
        </label>
        <span className='batch-count'>
          {t('已选')} <b>{selectedCount}</b> {t('个 · 共')} <b>{rows.length}</b> {t('个')}
        </span>
        {clashPicked ? (
          <span className='batch-hidden'>
            {t('其中 {n} 条来自 Clash 同步：可参与批量测试，删除会跳过', { n: clashPicked })}
          </span>
        ) : null}
        <div className='batch-actions'>
          <Button variant='outline' disabled={!selectedCount || snapshot.batchBusy}
            onClick={() => void batchTest()}>
            {progress ? t('测试中 {done}/{total}', { done: progress.done, total: progress.total }) : t('批量测试')}
          </Button>
          <Button variant='destructive' disabled={!selectedCount || snapshot.batchBusy}
            onClick={() => void batchRemove()}>
            {t('批量删除')}
          </Button>
          <Button variant='outline' disabled={!selectedCount || snapshot.batchBusy}
            onClick={() => clearSelection()}>
            {t('取消选择')}
          </Button>
        </div>
      </div>

      <div className='models-table-wrap'>
        <table className='models-table'>
          <colgroup>
            {/* 勾选列用**像素**而不是百分比：它是固定尺寸的小件（14px 勾选框 +
                单元格内边距），百分比在窄窗口下会把它压到装不下 */}
            <col className='w-[36px]' />
            <col className='w-[20%]' />
            <col className='w-[9%]' />
            <col className='w-[24%]' />
            <col className='w-[8%]' />
            <col className='w-[17%]' />
            <col className='w-[18%]' />
          </colgroup>
          <thead>
            <tr>
              <th className='ta-center' />
              <th>{t('名称')}</th>
              <th>{t('类型')}</th>
              <th>{t('地址')}</th>
              <th className='ta-center'>{t('启用')}</th>
              <th>{t('上次测试')}</th>
              <th className='ta-right'>{t('操作')}</th>
            </tr>
          </thead>
          <tbody>
            {rows.length ? rows.map(item => {
              const busy = snapshot.pending.has(item.id)
              const picked = isPicked(item.id)
              return (
                <tr key={item.id} className={cn(item.enabled ? '' : 'off', picked && 'bg-primary-soft')}
                  data-id={item.id}>
                  {COLUMNS.map((column, index) => (
                    <td key={column} className={CELL[index]}>
                      {cell(column, item, busy, picked)}
                    </td>
                  ))}
                </tr>
              )
            }) : (
              <tr>
                <td colSpan={COLUMNS.length} className='empty'>
                  {ready
                    ? t('还没有代理。点右上角「＋ 新增代理」手填一个，或点「同步 Clash Verge」把 Clash 的出口导进来。')
                    : t('加载中…')}
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>

      <div className='panel-foot'>
        {/* 行内 <b> 是强调，拆片段保住它（与 docs-page 脚注同一手法）；片段的空格是原文 */}
        <span>
          {t('这里配的出口可被账号引用（账号弹窗 →「出网代理」→「已保存的代理」）—— 改一次地址、所有引用的账号一起生效。测试的判据是')}
          <b>{t('能否连上上游')}</b>
          {t('， 出口 IP 与耗时是附带读数。')}
          <b>Clash Verge</b>
          {t(' 那一类由「同步 Clash Verge」 镜像进来（进入本页时会自动同步一次）：名称、端口、启用都跟随 Clash， 在 Clash 里改或删，这里跟着变 —— 所以它们只读，改请到 Clash Verge 里改。')}
        </span>
        {/* Clash 状态常驻在页脚：同步是静默的（进页面自动跑一次），没有这句话时
            「为什么一条 Clash 条目都没有」只能靠点按钮 + 看 toast 才知道 */}
        {snapshot.data?.clash && snapshot.data.clash.available === false ? (
          <span className='text-subtle'>
            {snapshot.data.clash.error
              ? t('未检测到 Clash Verge：{reason}', { reason: snapshot.data.clash.error })
              : t('未检测到 Clash Verge')}
          </span>
        ) : null}
        {snapshot.error ? (
          <span className='text-destructive'>{t('读取失败：{reason}', { reason: snapshot.error })}</span>
        ) : null}
      </div>

      {modal ? (
        <ProxyModal
          item={modal.item}
          onClose={() => setModal(null)}
        />
      ) : null}
    </section>
  )
}

/* ─── 新增 / 编辑弹窗 ───────────────────────── */

/**
 * 新增 / 编辑**手动**条目。
 *
 * 为什么不在这里提供「Clash Verge 出口」一档：Clash 那类由同步整体镜像进来
 * （见模块头），是只读记录 —— 手填一条「不属于同步集合」的 Clash 引用会让
 * 下一次同步把它摘掉，等于用户白填一次。要引用 Clash 出口就点工具条的
 * 「同步 Clash Verge」，一次把当前出口集合全部导入。
 */
function ProxyModal({
  item,
  onClose,
}: {
  item: ProxyPoolItem | null
  onClose: () => void
}) {
  const editing = item !== null
  const [name, setName] = React.useState(item?.name || '')
  const [protocol, setProtocol] = React.useState<'http' | 'socks5'>(item?.protocol === 'socks5' ? 'socks5' : 'http')
  const [host, setHost] = React.useState(item?.host || '')
  const [port, setPort] = React.useState(item?.port ? String(item.port) : '')
  const [username, setUsername] = React.useState(item?.username || '')
  const [password, setPassword] = React.useState(item?.password || '')
  const [enabled, setEnabled] = React.useState(item ? item.enabled : true)
  const [saving, setSaving] = React.useState(false)
  const [testing, setTesting] = React.useState(false)
  const [status, setStatus] = React.useState<React.ReactNode>(null)

  /** 表单 → 归一化后的提交对象；形状非法时返回一句面向用户的错误 */
  function draftPayload(): { payload: Record<string, unknown> } | { error: string } {
    const trimmed = name.trim()
    if (!trimmed) return { error: t('请填写代理名称') }
    const hostText = host.trim()
    if (!hostText) return { error: t('请填写代理主机地址') }
    const portNumber = Number(port)
    if (!Number.isInteger(portNumber) || portNumber < 1 || portNumber > 65535) {
      return { error: t('代理端口必须是 1-65535 的整数') }
    }
    return {
      payload: {
        name: trimmed,
        source: 'manual',
        protocol,
        host: hostText,
        port: portNumber,
        username: username.trim(),
        password,
        enabled,
      },
    }
  }

  /** 用当前表单内容测一次（**未保存也能测**：先确认能通，再决定保存） */
  async function testDraft(): Promise<void> {
    if (testing) return
    const draft = draftPayload()
    if ('error' in draft) {
      setStatus(<span className='text-destructive'>❌ {draft.error}</span>)
      return
    }
    const payload = draft.payload
    const proxy = {
      source: 'custom',
      protocol: payload.protocol,
      host: payload.host,
      port: payload.port,
      username: payload.username,
      password: payload.password,
    }
    setTesting(true)
    setStatus(t('正在连接上游…'))
    try {
      const data = await wb().workbuddyDesktop?.testProxy?.({ proxy })
      if (data?.success) {
        const suffix = durationText(data.durationMs)
        setStatus(
          <span className='text-success'>
            {t('✅ 出口可用')}
            {data.ip ? t('　出口 IP {ip}', { ip: data.ip }) : ''}
            {suffix ? `　${suffix}` : ''}
          </span>,
        )
      } else {
        setStatus(<span className='text-destructive'>❌ {data?.error || t('连接失败')}</span>)
      }
    } catch (error) {
      setStatus(<span className='text-destructive'>❌ {errorMessage(error)}</span>)
    } finally {
      setTesting(false)
    }
  }

  async function save(): Promise<void> {
    if (saving) return
    const draft = draftPayload()
    if ('error' in draft) {
      setStatus(<span className='text-destructive'>❌ {draft.error}</span>)
      return
    }
    setSaving(true)
    setStatus(null)
    try {
      await saveItem(item, draft.payload)
      toast(editing ? t('已保存「{name}」', { name: name.trim() }) : t('已新增「{name}」', { name: name.trim() }))
      onClose()
    } catch (error) {
      setStatus(<span className='text-destructive'>❌ {errorMessage(error)}</span>)
      setSaving(false)
    }
  }

  return (
    <Dialog open onOpenChange={open => { if (!open && !saving) onClose() }}>
      <DialogContent className='max-w-[560px]'>
        <DialogHeader>
          <DialogTitle>{editing ? t('编辑代理') : t('新增代理')}</DialogTitle>
        </DialogHeader>
        <DialogBody className='flex flex-col gap-3.5'>
          <div className='flex flex-col gap-1.5'>
            <Label htmlFor='proxy-name'>{t('名称')}</Label>
            <Input id='proxy-name' maxLength={60} autoComplete='off' spellCheck={false}
              placeholder={t('例如：香港 HTTP 出口')}
              value={name} onChange={event => setName(event.currentTarget.value)} />
          </div>

          <div className='flex flex-row items-end gap-2.5'>
            <div className='flex flex-col gap-1.5'>
              <Label htmlFor='proxy-protocol'>{t('协议')}</Label>
              {/* 协议名 HTTP / SOCKS5 是固定叫法，不进词典 */}
              <Select value={protocol} onValueChange={value => setProtocol(value as 'http' | 'socks5')}>
                <SelectTrigger id='proxy-protocol' className='min-w-[110px]'>
                  <SelectValue>{protocol === 'socks5' ? 'SOCKS5' : 'HTTP'}</SelectValue>
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value='http'>HTTP</SelectItem>
                  <SelectItem value='socks5'>SOCKS5</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <div className='flex min-w-0 flex-1 flex-col gap-1.5'>
              <Label htmlFor='proxy-host'>{t('主机')}</Label>
              <Input id='proxy-host' placeholder='127.0.0.1' autoComplete='off' spellCheck={false}
                value={host} onChange={event => setHost(event.currentTarget.value)} />
            </div>
            <div className='flex flex-col gap-1.5'>
              <Label htmlFor='proxy-port'>{t('端口')}</Label>
              <Input id='proxy-port' type='number' min={1} max={65535} className='max-w-[110px]'
                placeholder='7890' value={port} onChange={event => setPort(event.currentTarget.value)} />
            </div>
          </div>
          <div className='flex flex-row items-end gap-2.5'>
            <div className='flex min-w-0 flex-1 flex-col gap-1.5'>
              <Label htmlFor='proxy-user'>{t('用户名')}</Label>
              <Input id='proxy-user' placeholder={t('可选')} autoComplete='off'
                value={username} onChange={event => setUsername(event.currentTarget.value)} />
            </div>
            <div className='flex min-w-0 flex-1 flex-col gap-1.5'>
              <Label htmlFor='proxy-pass'>{t('密码')}</Label>
              <Input id='proxy-pass' placeholder={t('可选')} autoComplete='off'
                value={password} onChange={event => setPassword(event.currentTarget.value)} />
            </div>
          </div>

          <div className='flex flex-row items-center gap-2.5'>
            <Switch checked={enabled} onCheckedChange={setEnabled} aria-label={t('启用这条代理')} />
            <span className='text-[12.5px]'>
              {t('启用（禁用的条目不会被账号使用，被引用时账号回退直连；仍可测试）')}
            </span>
          </div>

          <p className='text-xs leading-[1.65] text-subtle'>
            {t('想要 Clash Verge 的出口？那类条目由工具条的')}<b>{t('「同步 Clash Verge」')}</b>{t('整体 镜像进来（进本页时也会自动同步一次），名称与端口跟随 Clash、因此是只读的 —— 不必（也不能）在这里手工填。')}
          </p>

          <div className='min-h-[18px] text-xs text-subtle'>{status}</div>
        </DialogBody>
        <DialogFooter>
          <Button variant='outline' disabled={testing || saving} onClick={() => void testDraft()}>
            {testing ? t('测试中…') : t('测试出口')}
          </Button>
          <div className='mr-auto' />
          <Button variant='outline' disabled={saving} onClick={onClose}>{t('取消')}</Button>
          <Button variant='default' disabled={saving} onClick={() => void save()}>
            {saving ? t('保存中…') : t('保存')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

/* ─── 挂载与对外契约 ─────────────────────────── */

/** app.js 切到本页时调它（同名契约见 ui/app.js 的 showPage） */
declare global {
  interface Window {
    wbProxiesPanel?: { load: () => Promise<void> }
  }
}

window.wbProxiesPanel = { load: () => loadPanel() }

const PAGE_SELECTOR = '.page[data-page="proxies"]'

/**
 * 把 React root 直接建在 `.page[data-page="proxies"]` 上（不套宿主 div：页面 CSS 用
 * `.page` 的直接子选择器分配布局，中间插一层会打断它）。先清掉骨架里的静态子节点，
 * 下面的 JSX 会按同样的类名重新渲染它。
 */
function mount(): void {
  const section = document.querySelector<HTMLElement>(PAGE_SELECTOR)
  if (!section || section.dataset.mounted === '1') return
  section.dataset.mounted = '1'
  section.replaceChildren()
  createRoot(section).render(<ProxiesPage />)
  // 首屏若正停在本页就自拉一次（app.js 启动时的 showPage 早于本岛加载，
  // 那一次 load() 会丢；与 keys / models 各岛同一处兜底）
  if (wb().wbApp?.currentPage === 'proxies') void loadPanel()
}

// 脚本排在页面骨架之后（index.html 里 islands/ui.js 在各 section 之后），正常直接挂；
// 万一将来被挪到前面，退化成等 DOM 解析完再挂。
if (document.querySelector(PAGE_SELECTOR)) mount()
else document.addEventListener('DOMContentLoaded', mount, { once: true })
