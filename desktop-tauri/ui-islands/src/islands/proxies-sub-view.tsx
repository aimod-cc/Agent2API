/**
 * Agent2API · 「订阅管理」弹窗 + 内核状态条（机场/节点订阅 → mihomo 内核本地监听器）。
 *
 * 与 Clash Verge 的订阅导入同一形态：粘贴订阅链接（或从图片解二维码）导入，
 * 另支持本地 YAML 文件导入（`kind: "local"`，内容随表单 base64 上传，服务端
 * 校验后入库；没有链接也就没有「定时刷新」——更新 = 编辑重选文件），
 * 内核拉取订阅、把每个节点起成本地 mixed 监听器，节点镜像进代理池
 * （`source: "subscription"` 的只读条目，账号按 id 引用）。存储与内核管理在
 * Rust 侧：`core::subscriptions`（订阅）与 `core::sub_kernel`（内核）是唯一
 * 事实来源，HTTP 契约见 `api::subscriptions` 的模块头。
 *
 * ── 为什么订阅在弹窗里（页面只有一张表）────────────────────────
 * 改造前订阅区块是代理池面板**上方**的第二个面板：两张表叠在同一页，
 * 页面被拉得很长，而订阅表多半是「配完就不常动」的低频内容，真正天天用的是
 * 下面那张池表。现在订阅表整体收进本弹窗（入口是代理池面板头的「订阅管理」
 * 按钮，带订阅条数），页面只剩一张表；内核状态条则提到页面级（见 `KernelBar`）
 * —— 它讲的是「本机内核 / 节点端口」这件全局的事，不该跟着订阅表一起被收起来。
 *
 * ── 数据层在隔壁 ─────────────────────────────────────────────
 * 本文件只放视图层（内核状态条 + 弹窗内的订阅表 + 新增/编辑弹窗）；状态、取数、
 * 动作都在 `proxies-sub-state.ts`（普通模块，与 proxies-state.ts 同一拆分）。
 *
 * ── 内核命令是异步的（界面轮询）────────────────────────────
 * 写操作后端投一条内核命令就立即返回（内核重启 + 拉订阅要几秒到几十秒），
 * 这里在每次写操作后轮询内核状态几轮（见 proxies-sub-state 的
 * pollKernelAfterChange），节点数 / 运行状态随之刷新；下载内核的进度条
 * 随轮询的 received/total 变。
 *
 * ── 二维码导入 ──────────────────────────────────────────────
 * 桌面端没有摄像头：入口是「从图片识别」按钮（文件选择器）与弹窗内的
 * 粘贴事件（Ctrl+V 剪贴板里的截图）。解码在服务端（rqrr），这里只做
 * base64 上传与回填链接；识别到单节点分享链接时后端明确报「暂不支持」。
 *
 * ── 样式 ────────────────────────────────────────────────────
 * 表格骨架（.models-table-wrap / .models-table）与其余页共用全局样式；
 * 内核状态条用既有令牌（border-border / bg-surface-2 / text-subtle），
 * 不新增 CSS 文件。
 */

import * as React from 'react'
import {
  Badge,
  Button,
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Input,
  Label,
  Progress,
  SegmentedControl,
  Switch,
} from '@ui'
import {
  addSubscription, decodeQrImage, errorMessage, kernel, loadSection,
  readFileAsBase64, refreshSubscription, removeKernel, removeSubscription, restartKernel, saveSubscription,
  stopKernel, subs, subscribe,
  getSnapshot, toggleSubscription, type SubscriptionItem,
} from './proxies-sub-state'
import { KernelDownloadDialog } from './proxies-kernel-dialog'
import {
  formatTime, loadSettings,
  // 代理池那份 store 与同步动作（本弹窗底部的「同步 Clash Verge」用；
  // 与上面订阅 store 的同名成员靠别名区分）
  getSnapshot as getPoolSnapshot, subscribe as subscribePool, syncClash,
} from './proxies-state'
import { t } from '../i18n'
// 赞助位（ProxyLane）：外链与 Logo 的唯一定义点在 sponsor.ts，
// 外链经 data-external 委托交系统浏览器打开
import { handleExternalClick } from './update-shared'
import { PROXYLANE_LOGO_DARK, PROXYLANE_LOGO_LIGHT, PROXYLANE_URL } from './sponsor'

/* ─── 内核状态条 ─────────────────────────────── */

/**
 * 内核状态条：徽标 + 版本 / 控制端口 / 监听器数 + 内核操作（下载 / 启动重启 /
 * 关闭 / 删除）。
 *
 * 两处使用：页面顶部（`showPortRange`，顺带显示节点端口范围 —— 范围改了要重启
 * 内核才对新节点生效，这两件事读在一起才有意义）与订阅管理弹窗内。
 *
 * 动作按「内核装没装、跑没跑」切换：未装只有「下载内核」；装了是「启动 / 重启
 * 内核」+「删除内核」，其中运行中再多一颗「关闭内核」（只停进程，订阅与端口
 * 注册表保留，见 proxies-sub-state 的 stopKernel）—— 停止后它消失，头一颗
 * 按钮从「重启内核」变回「启动内核」，正好是用户要的回程。
 */
export function KernelBar({ showPortRange = false }: { showPortRange?: boolean }) {
  const snapshot = React.useSyncExternalStore(subscribe, getSnapshot)
  const status = kernel()
  const [range, setRange] = React.useState<{ base: number; end: number } | null>(null)
  /** 「下载内核」弹窗（出口 / 存放位置 / 进度 / 停止都在里面，见 proxies-kernel-dialog） */
  const [downloadOpen, setDownloadOpen] = React.useState(false)

  // 端口范围是低频配置：挂载时静默读一次，读不到就不显示这一段（不影响内核状态本身）
  React.useEffect(() => {
    let alive = true
    void (async () => {
      try {
        const data = await loadSettings()
        if (!alive || !data) return
        const base = Number(data.listenerPortBase)
        const end = Number(data.listenerPortEnd)
        if (Number.isFinite(base) && Number.isFinite(end) && base > 0 && end > 0) setRange({ base, end })
      } catch {
        /* 设置读不到只是少一条读数，静默 */
      }
    })()
    return () => { alive = false }
  }, [])

  const downloading = status?.download.state === 'downloading'
  const percent =
    downloading && status!.download.total > 0
      ? Math.round((status!.download.received / status!.download.total) * 100)
      : null

  return (
    /* 紧凑状态条：一行放下徽标 / 读数 / 操作，进度条只在下载中展开；
       未安装是个高频空态，说明文字收在一句话里，不占额外一行 */
    <div className='flex flex-col gap-2 rounded-md border border-border bg-surface-2 px-3 py-2'>
      <div className='flex flex-wrap items-center gap-2'>
        <Badge variant={status?.running ? 'default' : 'secondary'}>
          {status?.running
            ? t('内核运行中')
            : status?.installed
              ? status.halted
                ? t('内核已手动关闭')
                : t('内核已停止')
              : t('内核未安装')}
        </Badge>
        <span className='text-[12.5px] text-text-2'>
          {/* 版本号是 t 的形参（占位符不传参会原样显示 "{version}"）；
              未安装时不显示版本段，只留按需下载的说明 */}
          {status?.installed
            ? t('内核 {version}', { version: status.version }) + '　·　'
            : ''}
          {status?.running
            ? t('控制端口 {port} · {n} 个节点监听器', { port: status?.controllerPort ?? '—', n: status?.listenerCount ?? 0 })
            : status?.installed
              ? status.halted
                ? t('内核已手动关闭（点「启动内核」拉起）')
                : t('内核已停止，点「启动内核」拉起')
              : t('按需下载（约 20-40MB）')}
        </span>
        {showPortRange && range ? (
          <span className='text-[11.5px] text-subtle'>
            {t('节点端口范围 {base}–{end}', { base: range.base, end: range.end })}
          </span>
        ) : null}
        <div className='ml-auto flex items-center gap-2'>
          {!status?.installed ? (
            /* 点它开**下载弹窗**（选出口 / 选目录 / 看进度 / 停下载都在那里）——
               不再直接开始下载：GitHub 直连不通时，「点了没反应」没有任何补救
               余地，而现在第一件事就是选一条线路。
               下载中按钮**保持可点**（打开弹窗看进度、停止）：`pending` 在下载
               全程为 true，若照旧禁用掉，用户把弹窗一关就再也回不到进度界面了 */
            <Button size='sm' variant='outline' disabled={snapshot.pending && !downloading}
              onClick={() => setDownloadOpen(true)}>
              {downloading ? t('下载中…') : t('下载内核')}
            </Button>
          ) : (
            <>
              <Button size='sm' variant='outline' disabled={snapshot.pending || downloading}
                onClick={() => void restartKernel()}>
                {status?.running ? t('重启内核') : t('启动内核')}
              </Button>
              {/* 关闭：只在运行中摆出来（停止后这颗按钮消失，上面的那一颗
                  已经变回「启动内核」）。只停进程 —— 订阅与端口注册表原样
                  保留，「启动内核」后原位恢复（见 proxies-sub-state 的
                  stopKernel）；与破坏性的「删除内核」是两回事 */}
              {status?.running ? (
                <Button size='sm' variant='outline' disabled={snapshot.pending || downloading}
                  onClick={() => void stopKernel()}>
                  {t('关闭内核')}
                </Button>
              ) : null}
              {/* 删除：装过内核之后才有意义（没装时摆了也只是个永远灰着的按钮），
                  破坏性且低频 —— 摆在启动 / 重启右侧、用危险语义，点开还有一道
                  确认（见 proxies-sub-state 的 removeKernel） */}
              <Button size='sm' variant='destructive' disabled={snapshot.pending || downloading}
                onClick={() => void removeKernel()}>
                {t('删除内核')}
              </Button>
            </>
          )}
        </div>
      </div>
      {downloading ? (
        <div className='flex items-center gap-3'>
          <Progress className='min-w-[200px] flex-1' value={percent ?? null} />
          <span className='text-[11px] text-subtle'>
            {percent !== null
              ? t('{percent}%（{received} / {total} MB）', {
                  percent,
                  received: mb(status!.download.received),
                  total: mb(status!.download.total),
                })
              : t('下载中…')}
          </span>
        </div>
      ) : null}
      {status?.lastError ? (
        <span className='text-[12px] text-destructive'>{status.lastError}</span>
      ) : null}
      {downloadOpen ? <KernelDownloadDialog onClose={() => setDownloadOpen(false)} /> : null}
    </div>
  )
}

/* ─── 订阅管理弹窗 ───────────────────────────── */

export function SubscriptionManagerDialog({ onClose }: { onClose: () => void }) {
  const snapshot = React.useSyncExternalStore(subscribe, getSnapshot)
  // 代理池那份 store：本弹窗底部的「同步 Clash Verge」读写的是它（见 footer）。
  // 与上面那份订阅 store 是**两个源**，同名成员必须别名区分（都是 subscribe /
  // getSnapshot，写混了会拿到另一份快照）。
  const pool = React.useSyncExternalStore(subscribePool, getPoolSnapshot)
  const items = subs()
  const ready = snapshot.data !== null
  const [modal, setModal] = React.useState<{ item: SubscriptionItem | null } | null>(null)

  // 打开时自拉一次：弹窗是订阅的唯一入口，不拉就会停在上一份快照上
  // （内核状态在轮询、别的窗口也可能改过订阅）
  React.useEffect(() => { void loadSection() }, [])

  return (
    <Dialog open onOpenChange={open => { if (!open) onClose() }}>
      <DialogContent className='w-[min(880px,calc(100vw-48px))]'>
        <DialogHeader>
          <DialogTitle>{t('订阅管理')}</DialogTitle>
          <span className='text-[11.5px] text-subtle'>
            {ready ? t('{n} 条订阅', { n: items.length }) : t('加载中…')}
            {'　·　'}
            {t('节点由内核起成本地监听器，镜像进代理池')}
          </span>
        </DialogHeader>
        <DialogBody className='flex flex-col gap-3'>
          <KernelBar />

          <div className='models-table-wrap'>
            <table className='models-table'>
              <colgroup>
                <col className='w-[18%]' />
                <col className='w-[31%]' />
                <col className='w-[9%]' />
                <col className='w-[14%]' />
                <col className='w-[8%]' />
                <col className='w-[20%]' />
              </colgroup>
              <thead>
                <tr>
                  <th>{t('名称')}</th>
                  <th>{t('订阅来源')}</th>
                  <th className='ta-center'>{t('节点')}</th>
                  <th>{t('上次同步')}</th>
                  <th className='ta-center'>{t('启用')}</th>
                  <th className='ta-right'>{t('操作')}</th>
                </tr>
              </thead>
              <tbody>
                {items.length ? items.map(item => {
                  const sync = kernel()?.providerSync?.[item.id]
                  return (
                    <tr key={item.id} className={item.enabled ? '' : 'off'}>
                      <td><b>{item.name || t('未命名')}</b></td>
                      {/* 来源列：链接订阅显示主机段 + hover 复制；本地文件订阅
                          显示文件名（没有链接可复制，「立即更新」对它也无意义） */}
                      <td className='group'>
                        <div className='flex min-w-0 items-center gap-1'>
                          {item.kind === 'local' ? (
                            <>
                              <span className='min-w-0 truncate font-mono text-[12px] text-text-2'
                                title={item.fileName}>{item.fileName || t('本地 YAML 文件')}</span>
                              <Badge variant='secondary'>{t('本地文件')}</Badge>
                            </>
                          ) : (
                            <>
                              <span className='min-w-0 truncate font-mono text-[12px] text-text-2'
                                title={item.url}>{shortUrl(item.url)}</span>
                              <Button variant='ghost' size='icon-xs'
                                className='copy-btn size-[21px] rounded-[var(--r-xs)] opacity-0 transition-opacity group-hover:opacity-100 focus-visible:opacity-100'
                                data-copy={item.url} title={t('复制订阅链接')}>⧉</Button>
                            </>
                          )}
                        </div>
                      </td>
                      {/* 节点列只放读数；同步时间归「上次同步」列（两列各就各位）。
                          节点数优先取内核状态里的**实时**同步结果：条目上的 nodeCount
                          是列表响应注入的快照，写操作后的轮询窗口里可能还是旧的 0
                          （徽标曾冻在「—」上 —— 列表与内核一起轮询 + 这里的实时兜底，
                          双保险） */}
                      <td className='ta-center'>
                        {liveNodeCount(sync, item) > 0 ? (
                          <Badge variant='secondary'>{t('{n} 个', { n: liveNodeCount(sync, item) })}</Badge>
                        ) : (
                          <span className='text-subtle'>{t('—')}</span>
                        )}
                      </td>
                      <td>
                        {sync?.at ? (
                          <span className='text-[12px] text-text-2'>{formatTime(sync.at)}</span>
                        ) : (
                          <span className='text-subtle'>{t('未同步')}</span>
                        )}
                      </td>
                      <td className='ta-center'>
                        <Switch checked={item.enabled} disabled={snapshot.pending}
                          aria-label={t('启用订阅「{name}」', { name: item.name || t('未命名') })}
                          onCheckedChange={next => void toggleSubscription(item, next)} />
                      </td>
                      <td>
                        <div className='row-actions'>
                          {/* 本地文件订阅没有链接可拉，「立即更新」对它无意义
                              （后端也会拒）—— 停用并说明，而不是让它白报一次错 */}
                          <Button size='sm' variant='ghost'
                            disabled={snapshot.pending || snapshot.refreshing !== null || item.kind === 'local'}
                            title={item.kind === 'local'
                              ? t('本地文件订阅无需更新（要换内容请编辑后重新选择文件）')
                              : undefined}
                            onClick={() => void refreshSubscription(item)}>
                            {snapshot.refreshing === item.id ? t('更新中…') : t('立即更新')}
                          </Button>
                          <Button size='sm' variant='ghost' disabled={snapshot.pending}
                            onClick={() => setModal({ item })}>{t('编辑')}</Button>
                          <Button size='sm' variant='destructive' disabled={snapshot.pending}
                            onClick={() => void removeSubscription(item)}>{t('删除')}</Button>
                        </div>
                      </td>
                    </tr>
                  )
                }) : (
                  <tr>
                    <td colSpan={6} className='empty'>
                      {ready ? (
                        /* 空态内嵌主操作按钮（视线不用跳到页脚）：一句话 + 按钮 +
                           一句「接下来会发生什么」的副提示 */
                        <div className='flex flex-col items-center gap-2 py-3'>
                          <span>{t('还没有订阅：粘贴订阅链接、导入本地 YAML 文件，或从图片识别二维码。')}</span>
                          <Button size='sm' variant='outline' disabled={snapshot.pending}
                            onClick={() => setModal({ item: null })}>{t('＋ 新增订阅')}</Button>
                          <span className='text-[11px] text-subtle'>
                            {t('订阅里的节点会自动同步进代理池，账号即可引用。')}
                          </span>
                        </div>
                      ) : t('加载中…')}
                    </td>
                  </tr>
                )}
              </tbody>
            </table>
          </div>

          <p className='text-xs leading-[1.65] text-subtle'>
            {t('订阅里的节点会被内核起成本地监听器，镜像到代理池（账号按 id 引用，与本页其它出口同一用法）。内核只在用到订阅时按需下载（约 20-40MB），不随安装包分发。')}
          </p>
          {snapshot.error ? (
            <span className='text-destructive'>{t('读取失败：{reason}', { reason: snapshot.error })}</span>
          ) : null}
        </DialogBody>
        <DialogFooter>
          <Button variant='default' disabled={snapshot.pending}
            onClick={() => setModal({ item: null })}>{t('＋ 新增订阅')}</Button>
          {/* 同步 Clash Verge 从页面工具条搬到这里（2026-10）：它与订阅是同一件事
              的两种来路 —— 都是「把外面的出口集合整份镜像进池」，摆在一起用户不必
              先在页面上找那一颗。批量在跑时停用：同步会把正在被批量流程改写的列表
              整份换掉（与搬过来之前同一条理由） */}
          <Button variant='outline' disabled={pool.syncing || pool.batchBusy}
            onClick={() => void syncClash()}>
            {pool.syncing ? t('同步中…') : t('同步 Clash Verge')}
          </Button>
          <div className='mr-auto' />
          <Button variant='outline' onClick={onClose}>{t('完成')}</Button>
        </DialogFooter>
      </DialogContent>

      {/* 新增 / 编辑订阅嵌在本弹窗之上：子层要自己那层遮罩（overlayForceRender），
          否则只有换了个框、点空白处也不会关闭 —— 见 ui-kit dialog.tsx 的说明 */}
      {modal ? (
        <SubModal item={modal.item} onClose={() => setModal(null)} />
      ) : null}
    </Dialog>
  )
}

/** 字节 → MB 文案（下载进度用；total 未知时是 —） */
function mb(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return '—'
  return (value / 1000 / 1000).toFixed(1)
}

/** 链接截断（表格里只显示主机段；title 里有完整链接） */
function shortUrl(url: string): string {
  try {
    const parsed = new URL(url)
    return parsed.host + (parsed.pathname !== '/' ? '/…' : '')
  } catch {
    return url.slice(0, 60)
  }
}

/**
 * 节点数：优先内核状态里的**实时**同步结果，回落条目上的注入快照。
 *
 * 两者同源（条目上的 nodeCount 由后端在列表响应里从 providerSync 注入），
 * 但时序不同：写操作后的轮询窗口里，条目快照可能还是操作前的 0，而内核
 * 状态已经拿到新结果 —— 徽标曾因此冻在「—」上，这里以实时值兜底。
 */
function liveNodeCount(
  sync: { count?: number } | undefined,
  item: SubscriptionItem,
): number {
  return sync?.count ?? item.nodeCount
}

/* ─── 新增 / 编辑订阅弹窗 ───────────────────── */

/**
 * 订阅来源（新增时可选；编辑沿用条目自己的，不提供换源）。
 *
 * `residential`（家庭宽带）是**赞助位**而不是第三种订阅：原生静态住宅 IP
 * 是按 IP 买的 HTTP / SOCKS5 出口，没有订阅链接，所以这一档只放 ProxyLane
 * 的介绍与「去哪儿买、买完怎么用」，没有可保存的内容（见 SubModal 的
 * footer 分支）。放在这里是因为「找一个干净出口」的念头恰好发生在这张表单上
 * （订阅 = 机场节点，家庭宽带 IP = 落地出口，两件事都是「给账号配出网」）。
 */
type SubSource = 'url' | 'local' | 'residential'

/** 「家庭宽带」一档的介绍卡：品牌 Logo + 赞助方给定的文案 + 怎么用 + 去获取 */
function ResidentialSponsorCard() {
  return (
    <div className='flex flex-col gap-2.5 rounded-lg border border-border bg-surface-2 px-3.5 py-3'>
      {/* Logo 两版各渲染一张，靠 dark: 变体切换（与网页面板的 <picture> 同款
          手法）：变体跟的是 data-theme，所以显式深色与「跟随系统」都对 */}
      <a
        href={PROXYLANE_URL}
        data-external={PROXYLANE_URL}
        onClick={handleExternalClick}
        className='inline-flex w-fit items-center no-underline'
      >
        <img src={PROXYLANE_LOGO_LIGHT} alt='ProxyLane' className='h-[24px] w-auto dark:hidden' />
        <img src={PROXYLANE_LOGO_DARK} alt='ProxyLane' className='hidden h-[24px] w-auto dark:block' />
      </a>
      {/* 品牌名裹成行内链接（与「新增代理」弹窗那行赞助提示同一手法）：句子里
          出现品牌名的地方就是投放位，读者不必先去找按钮 */}
      <p className='text-xs leading-[1.7] text-text-2'>
        <a
          className='font-semibold text-primary-fg no-underline hover:underline'
          href={PROXYLANE_URL}
          data-external={PROXYLANE_URL}
          onClick={handleExternalClick}
        >ProxyLane</a>
        {t(' 提供 AI 账号专用的原生静态住宅 IP（Static Residential ISP）：IP 来自真实家庭宽带运营商，Scamalytics 低欺诈评分，每个 IP 独享，不限流量，$3.90 起。')}
      </p>
      <p className='text-xs leading-[1.7] text-text-2'>
        {t('首次购买使用优惠码 AGENT2API25 享 75 折。')}
      </p>
      {/* 卡片自己的动作收在文案之后：读完就知道下一步去哪，不必回到底部找按钮 */}
      <div>
        <Button size='sm' render={
          <a href={PROXYLANE_URL} data-external={PROXYLANE_URL} onClick={handleExternalClick} />
        }>{t('去获取')}</Button>
      </div>
    </div>
  )
}

function SubModal({
  item,
  onClose,
}: {
  item: SubscriptionItem | null
  onClose: () => void
}) {
  const editing = item !== null
  // 编辑时锁定来源：两种来源的字段没有交集，可切换只会多出一堆「清掉哪个
  // 字段」的分支；真要换来源删了重加即可（id 换新，代理池条目随节点重建）
  const [source, setSource] = React.useState<SubSource>(item?.kind === 'local' ? 'local' : 'url')
  const [name, setName] = React.useState(item?.name || '')
  const [url, setUrl] = React.useState(item?.url || '')
  const [fileName, setFileName] = React.useState(item?.fileName || '')
  const [contentB64, setContentB64] = React.useState('')
  const [enabled, setEnabled] = React.useState(item ? item.enabled : true)
  const [saving, setSaving] = React.useState(false)
  const [decoding, setDecoding] = React.useState(false)
  const [reading, setReading] = React.useState(false)
  const [status, setStatus] = React.useState<React.ReactNode>(null)
  const fileInput = React.useRef<HTMLInputElement | null>(null)

  async function save(): Promise<void> {
    if (saving) return
    const nameText = name.trim()
    if (!nameText) {
      setStatus(<span className='text-destructive'>❌ {t('请填写订阅名称')}</span>)
      return
    }
    const urlText = url.trim()
    if (source === 'url' && !(urlText.startsWith('http://') || urlText.startsWith('https://'))) {
      setStatus(<span className='text-destructive'>❌ {t('订阅链接必须是 http:// 或 https:// 开头')}</span>)
      return
    }
    if (!editing && source === 'local' && !contentB64) {
      setStatus(<span className='text-destructive'>❌ {t('请选择要导入的 YAML 文件')}</span>)
      return
    }
    setSaving(true)
    try {
      if (editing && source === 'local') {
        // 没重选文件就不带 contentB64 —— 后端沿用导入时的内容
        await saveSubscription(item!, {
          name: nameText,
          enabled,
          kind: 'local',
          fileName: fileName || undefined,
          ...(contentB64 ? { contentB64 } : {}),
        })
      } else if (editing) {
        await saveSubscription(item!, { name: nameText, url: urlText, enabled, kind: 'url' })
      } else if (source === 'local') {
        await addSubscription({ name: nameText, kind: 'local', fileName: fileName || undefined, contentB64 })
      } else {
        await addSubscription({ name: nameText, kind: 'url', url: urlText })
      }
      onClose()
    } catch (error) {
      setStatus(<span className='text-destructive'>❌ {errorMessage(error)}</span>)
      setSaving(false)
    }
  }

  /** 从图片文件解二维码 → 回填链接（服务端解码，见 proxies-sub-state） */
  async function decodeFrom(file: File): Promise<void> {
    if (decoding) return
    setDecoding(true)
    setStatus(t('正在识别二维码…'))
    try {
      const decoded = await decodeQrImage(file)
      setUrl(decoded)
      setStatus(<span className='text-success'>✅ {t('已识别订阅链接')}</span>)
    } catch (error) {
      setStatus(<span className='text-destructive'>❌ {errorMessage(error)}</span>)
    } finally {
      setDecoding(false)
    }
  }

  /** 选本地 YAML 文件：只做读取与回显，形状校验交给保存时的服务端 */
  async function pickYaml(file: File): Promise<void> {
    if (reading) return
    setReading(true)
    setStatus(null)
    try {
      const base64 = await readFileAsBase64(file)
      setContentB64(base64)
      setFileName(file.name)
      setStatus(<span className='text-success'>✅ {t('已选择「{name}」，保存后导入', { name: file.name })}</span>)
    } catch (error) {
      setStatus(<span className='text-destructive'>❌ {errorMessage(error)}</span>)
    } finally {
      setReading(false)
    }
  }

  /** 剪贴板里是图片（截图的二维码）时直接识别：粘贴事件里取文件 */
  function onPaste(event: React.ClipboardEvent): void {
    if (source !== 'url') return
    const files: File[] = Array.from(event.clipboardData?.files || [])
    const image = files.find(file => file.type.startsWith('image/'))
    if (image) {
      event.preventDefault()
      void decodeFrom(image)
    }
  }

  /** 切来源：两边的草稿互不通用，状态行一并清掉 */
  function switchSource(next: SubSource): void {
    if (next === source) return
    setSource(next)
    setStatus(null)
  }

  return (
    <Dialog open onOpenChange={open => { if (!open && !saving) onClose() }}>
      <DialogContent className='max-w-[560px]' overlayForceRender onPaste={onPaste}>
        <DialogHeader>
          <DialogTitle>{editing ? t('编辑订阅') : t('新增订阅')}</DialogTitle>
        </DialogHeader>
        <DialogBody className='flex flex-col gap-3.5'>
          {/* 名称 / 启用 / 保存这一套只服务两种真订阅；「家庭宽带」是介绍卡，
              没有可保存的内容，名称那格也跟着收起（否则空着一个必填项） */}
          {source === 'residential' ? null : (
            <div className='flex flex-col gap-1.5'>
              <Label htmlFor='sub-name'>{t('名称')}</Label>
              <Input id='sub-name' maxLength={60} autoComplete='off' spellCheck={false}
                placeholder={t('例如：我的机场')} value={name}
                onChange={event => setName(event.currentTarget.value)} />
            </div>
          )}

          {/* 来源只在新增时可选（编辑锁定为条目自己的来源，见 source 的说明） */}
          {!editing ? (
            <div className='flex flex-col gap-1.5'>
              <Label>{t('订阅来源')}</Label>
              <SegmentedControl
                options={[
                  { value: 'url', label: t('订阅链接') },
                  { value: 'local', label: t('本地 YAML 文件') },
                  { value: 'residential', label: t('家庭宽带') },
                ]}
                value={source}
                onValueChange={switchSource}
                aria-label={t('订阅来源')}
              />
            </div>
          ) : null}

          {source === 'residential' ? <ResidentialSponsorCard /> : source === 'url' ? (
            <div className='flex flex-col gap-1.5'>
              <Label htmlFor='sub-url'>{t('订阅链接')}</Label>
              <Input id='sub-url' autoComplete='off' spellCheck={false}
                placeholder='https://…' value={url}
                onChange={event => setUrl(event.currentTarget.value)} />
              <div className='flex items-center gap-2'>
                <Button size='sm' variant='outline' disabled={decoding}
                  onClick={() => fileInput.current?.click()}>
                  {decoding ? t('识别中…') : t('从图片识别二维码')}
                </Button>
                <span className='text-[11px] text-subtle'>{t('也可以把二维码截图直接粘贴到弹窗里（Ctrl+V）')}</span>
                <input ref={fileInput} type='file' accept='image/*' className='hidden'
                  onChange={event => {
                    const file = event.currentTarget.files?.[0]
                    if (file) void decodeFrom(file)
                    event.currentTarget.value = ''
                  }} />
              </div>
            </div>
          ) : (
            <div className='flex flex-col gap-1.5'>
              <Label htmlFor='sub-file'>{t('YAML 文件')}</Label>
              <div className='flex min-w-0 items-center gap-2'>
                <Button size='sm' variant='outline' disabled={reading} className='shrink-0'
                  onClick={() => fileInput.current?.click()}>
                  {reading ? t('读取中…') : editing ? t('重新选择文件…') : t('选择 YAML 文件…')}
                </Button>
                <span className='min-w-0 truncate font-mono text-[12px] text-text-2'
                  title={fileName || undefined}>
                  {fileName || (editing ? t('保持原文件') : t('未选择文件'))}
                </span>
              </div>
              {/* 编辑时重选是**可选**的：不带新文件保存 = 内容不变（后端沿用
                  存储里的 YAML 原文）；这正是本地订阅唯一的更新方式 */}
              {editing ? (
                <span className='text-[11px] text-subtle'>
                  {t('本地文件不会自动更新：要更新节点，重新选择文件后保存；不重选则内容保持不变。')}
                </span>
              ) : null}
              <input ref={fileInput} type='file' accept='.yaml,.yml,text/yaml,application/yaml,text/plain'
                className='hidden'
                onChange={event => {
                  const file = event.currentTarget.files?.[0]
                  if (file) void pickYaml(file)
                  event.currentTarget.value = ''
                }} />
            </div>
          )}

          {/* 启用开关与底部说明、状态行都只服务两种真订阅（家庭宽带那一档
              全是介绍，见 ResidentialSponsorCard） */}
          {source === 'residential' ? null : (
            <>
              <div className='flex flex-row items-center gap-2.5'>
                <Switch checked={enabled} onCheckedChange={setEnabled} aria-label={t('启用这条订阅')} />
                <span className='text-[12.5px]'>{t('启用（禁用的订阅不参与内核同步，它的节点会从代理池移除）')}</span>
              </div>

              {/* 定时刷新只属于链接订阅；本地文件是导入那一刻的内容，两种说明分开写 */}
              <p className='text-xs leading-[1.65] text-subtle'>
                {source === 'url'
                  ? t('订阅内容由内核定时刷新（每 24 小时）；节点数与同步状态在保存后由轮询刷新，稍等几秒。')
                  : t('本地文件订阅的内容在导入时固定：节点会由内核起成本地监听器并镜像进代理池，节点数稍等几秒由轮询刷新。')}
              </p>

              <div className='min-h-[18px] text-xs text-subtle'>{status}</div>
            </>
          )}
        </DialogBody>
        <DialogFooter>
          <div className='mr-auto' />
          <Button variant='outline' disabled={saving} onClick={onClose}>{t('取消')}</Button>
          {/* 介绍卡没有可保存的东西，它的动作（去获取）在卡片里 —— 同一件事不在
              弹窗里放两颗按钮 */}
          {source === 'residential' ? null : (
            <Button variant='default' disabled={saving || decoding || reading} onClick={() => void save()}>
              {saving ? t('保存中…') : t('保存')}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
