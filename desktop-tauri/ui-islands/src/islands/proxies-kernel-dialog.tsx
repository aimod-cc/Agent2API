/**
 * Agent2API · 「下载内核」弹窗（内核状态条上那颗「下载内核」打开的）。
 *
 * ── 为什么把两个选择摆在下载之前 ──────────────────────────────
 * 内核要从 GitHub 拉（国内直连常常不通）、几十 MB 还可能想放到别的盘上 ——
 * 这两件事必须在**开始之前**定，下到一半再改没有任何意义（半截文件已经落在
 * 旧目录里了）。于是弹窗分两态：配置态（出口 + 存放位置 + 下载）→ 进度态
 * （进度条 + 停止下载）。两态由**后端快照的 `download.state`** 决定，不是本地
 * 开关：关掉窗口再点开、或者在别处发起下载，看到的仍是真实进行中的那一次。
 *
 * ── 关闭窗口不等于停止（这条要写在界面上）─────────────────────
 * 下载是后端任务，关窗只是不看它了；要停下必须点「停止下载」。所以进度态
 * 里常驻这句话，并在停止时如实报三件事之一：停下来了（临时文件已删）/ 没赶上
 * （刚好装完）/ 根本没在下载 —— 见 `cancelKernelDownload` 的说明。
 *
 * ── 出口下拉的选项只有「直连 + 代理池条目」─────────────────────
 * 与「更新设置」弹窗**同一个构造器**（`buildProxyPick`）：出口统一在「网络
 * 代理」页维护（配 / 命名 / 测速 / 同步 Clash），这里只做「选哪一条」。
 * 值域也一致（`pool:<id>`），当前值不在池里时补一条占位项而不是回落成「直连」。
 *
 * 注意：订阅节点此刻多半**还没有端口**（它们本身就是内核起了监听器之后才有
 * 的），所以下拉里能实际用的通常是 Clash Verge 镜像条目与手动条目 ——
 * 这不是 bug，是「先用 Clash 把内核拉下来」这条顺序本身。
 */

import * as React from 'react'
import {
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
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  cn,
} from '@ui'
import type { PoolItem } from './accounts-shared'
import { POOL_VALUE_PREFIX } from './accounts-shared'
import { PROXY_OTHER_CURRENT, buildProxyPick } from './update-shared'
import {
  cancelKernelDownload, downloadKernel, errorMessage, getSnapshot, kernel, subscribe, toast, wb,
  type KernelDownloadChoice,
} from './proxies-sub-state'
import { t } from '../i18n'

/** 字节 → MB 的一位数读数（与内核状态条同一格式） */
function mb(bytes: number | undefined): string {
  return (Math.max(0, Number(bytes) || 0) / 1024 / 1024).toFixed(1)
}

/**
 * 像不像一条绝对路径（`D:\x` / `\\server\share\x` / `/x`）。
 *
 * 只管「明显写错」这一步 —— 真正的判据在后端（`api::subscriptions` 用
 * `Path::is_absolute`），这里挡一道只是为了不让用户白等一次往返。
 */
function looksAbsolute(text: string): boolean {
  return /^[A-Za-z]:[\\/]/.test(text) || text.startsWith('\\\\') || text.startsWith('/')
}

export function KernelDownloadDialog({ onClose }: { onClose: () => void }) {
  const snapshot = React.useSyncExternalStore(subscribe, getSnapshot)
  const status = kernel()
  const download = status?.download
  const downloading = download?.state === 'downloading'
  const percent =
    downloading && download && download.total > 0
      ? Math.round((download.received / download.total) * 100)
      : null

  // 代理池（下拉选项的唯一来源）：与「更新设置」弹窗同一条链，打开时拉一次
  const [pool, setPool] = React.useState<PoolItem[] | null>(null)
  const [poolError, setPoolError] = React.useState('')
  // 用户改过就以此为准（null = 还没动过 → 显示后端存的那条）
  const [proxyValue, setProxyValue] = React.useState<string | null>(null)
  const [dir, setDir] = React.useState(status?.kernelDir || '')
  const [busy, setBusy] = React.useState(false)
  const [stopping, setStopping] = React.useState(false)
  const [formError, setFormError] = React.useState('')

  React.useEffect(() => {
    let alive = true
    void (async () => {
      try {
        const items = (await wb().workbuddyDesktop?.getProxyPool?.())?.items
        // 桥异常时可能 resolve 出 undefined 而不是 reject：不校验会伪装成「池里一条都没有」
        if (!Array.isArray(items)) throw new Error(t('代理池响应异常'))
        if (alive) setPool(items)
      } catch (error) {
        if (!alive) return
        setPool([])
        setPoolError(errorMessage(error))
      }
    })()
    return () => { alive = false }
  }, [])

  // 装好了就没什么可下载的：下载完成 / 别处装上了，弹窗自己收起来
  // （`installed` 是后端的实际判定，不是本地「我刚点了下载」的记忆）
  React.useEffect(() => {
    if (status?.installed) onClose()
  }, [status?.installed])

  const pick = buildProxyPick({ proxyChoice: status?.downloadProxy ?? null, pool, poolError })
  const currentProxy = proxyValue ?? pick.current
  const selectedProxy = pick.items.find(item => item.value === currentProxy) ?? pick.selected
  const defaultDir = status?.kernelDirDefault || ''

  /** 开始下载：先把两个选择交给后端（它先落配置再下载），进度由全局快照推进 */
  async function start(): Promise<void> {
    if (busy) return
    const trimmed = dir.trim()
    if (trimmed && !looksAbsolute(trimmed)) {
      setFormError(t('存放位置要填绝对路径，例如 D:\\proxy-kernel'))
      return
    }
    setFormError('')
    setBusy(true)
    try {
      const payload: KernelDownloadChoice = { dir: trimmed }
      // 占位项（存量里直接引用 Clash 出口 / 自定义形状）不代表用户的新选择：
      // 不带上 proxy 字段 = 后端维持已存的那份，与「选它等于维持现状」一致
      if (currentProxy !== PROXY_OTHER_CURRENT) {
        payload.proxy = currentProxy === ''
          ? null
          : { source: 'pool', proxyId: currentProxy.slice(POOL_VALUE_PREFIX.length) }
      }
      // **不 await**：下载最长几分钟，弹窗靠快照渲染进度，等它会把按钮卡住
      void downloadKernel(payload)
    } finally {
      setBusy(false)
    }
  }

  async function stop(): Promise<void> {
    if (stopping) return
    setStopping(true)
    try {
      // 「停下来了」由 downloadKernel 那个轮询循环播报（它一直在跑，弹窗关着
      // 也照样报）—— 这里只报循环**不知道**的三种情况：没赶上 / 没在下载 /
      // 没收敛。两处都报会让同一条「已停止下载内核」弹两遍（实测撞过）
      const outcome = await cancelKernelDownload()
      if (outcome === 'done') {
        toast(t('下载已经完成，停止没赶上（内核已安装）'))
      } else if (outcome === 'none') {
        toast(t('当前没有正在进行的下载'), 'err')
      } else if (outcome === 'stalled') {
        toast(t('停止请求已发出，但下载还没收手，稍后再看一次进度'), 'err')
      }
    } finally {
      setStopping(false)
    }
  }

  /**
   * 「选择目录…」：走桌面壳的系统文件夹选择框。
   *
   * 只有**壳**能弹系统框（web 版会回 `unsupported`，桥缺失时干脆没有这个方法）：
   * 那种环境下按钮点了没反应最让人困惑，所以直接说一句「手动填路径」—— 输入框
   * 本来就支持手敲。
   */
  async function chooseDir(): Promise<void> {
    const bridge = wb().workbuddyDesktop
    if (typeof bridge?.pickFolder !== 'function') {
      toast(t('当前环境不支持选择文件夹，直接填写绝对路径即可'), 'err')
      return
    }
    try {
      const picked = await bridge.pickFolder(dir.trim() || defaultDir || null)
      if (picked?.unsupported) {
        toast(t('当前环境不支持选择文件夹，直接填写绝对路径即可'), 'err')
        return
      }
      if (picked?.canceled || !picked?.path) return
      setDir(picked.path)
    } catch (error) {
      toast(t('打开文件夹选择框失败：{reason}', { reason: errorMessage(error) }), 'err')
    }
  }

  return (
    <Dialog open onOpenChange={open => { if (!open) onClose() }}>
      <DialogContent className='w-[min(520px,calc(100vw-48px))]'>
        <DialogHeader>
          <DialogTitle>{downloading ? t('正在下载内核') : t('下载内核')}</DialogTitle>
          <span className='text-[11.5px] text-subtle'>
            {t('内核 {version}（{asset}）', { version: status?.version || '—', asset: status?.asset || '—' })}
          </span>
        </DialogHeader>

        {downloading ? (
          <DialogBody className='flex flex-col gap-3'>
            <Progress value={percent ?? null} />
            <div className='flex items-center justify-between text-[12px]'>
              <span className='text-text-2'>
                {percent !== null
                  ? t('{percent}%（{received} / {total} MB）', {
                      percent,
                      received: mb(download?.received),
                      total: mb(download?.total),
                    })
                  : t('正在连接 GitHub…')}
              </span>
              <span className='text-subtle'>
                {t('存放位置 {dir}', { dir: status?.kernelDir || '—' })}
              </span>
            </div>
            <p className='text-xs leading-[1.65] text-subtle'>
              {t('关掉这个窗口不会中断下载（它在后台继续跑）—— 要停下就点下面的「停止下载」，未下载完的临时文件会被删除。')}
            </p>
          </DialogBody>
        ) : (
          <DialogBody className='flex flex-col gap-3.5'>
            <div className='flex flex-col gap-1.5'>
              <Label htmlFor='kernel-proxy'>{t('下载线路')}</Label>
              <Select value={currentProxy} onValueChange={value => setProxyValue(String(value))}>
                {/* 线路不可用时描边标红（与「更新设置」弹窗、账号页代理列同一口径） */}
                <SelectTrigger id='kernel-proxy' className={cn('w-full', pick.broken && 'border-destructive-bd')}>
                  <SelectValue className='min-w-0 truncate'>{selectedProxy?.label || t('直连')}</SelectValue>
                </SelectTrigger>
                <SelectContent>
                  {pick.items.map(item => (
                    <SelectItem key={item.value} value={item.value} disabled={item.disabled}>{item.label}</SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <div className='text-[12px] text-subtle'>
                {pick.broken
                  ? t('当前线路不可用：{error}；重新选一条或切回直连', {
                      error: String(status?.downloadProxy?.error || ''),
                    })
                  : t('选项来自「网络代理」页（Clash Verge 同步的出口在这里可用；订阅节点要等内核起来才有端口）。选「直连」时连不上会自动借 Clash 混合端口重试一次；选定某条线路后只走它。')}
              </div>
            </div>

            <div className='flex flex-col gap-1.5'>
              <Label htmlFor='kernel-dir'>{t('存放位置')}</Label>
              <div className='flex items-center gap-2'>
                <Input id='kernel-dir' className='min-w-0 flex-1 font-mono text-[12px]'
                  autoComplete='off' spellCheck={false} value={dir}
                  placeholder={defaultDir}
                  onChange={event => setDir(event.currentTarget.value)} />
                <Button variant='outline' size='sm' className='flex-none'
                  onClick={() => void chooseDir()}>{t('选择目录…')}</Button>
                {defaultDir && dir.trim() !== defaultDir ? (
                  <Button variant='ghost' size='sm' className='flex-none' title={defaultDir}
                    onClick={() => setDir(defaultDir)}>{t('恢复默认')}</Button>
                ) : null}
              </div>
              <div className='text-[12px] text-subtle'>
                {t('内核二进制 / 配置 / 端口映射都会放在这里，默认 {dir}；目录不存在会在下载开始时自动创建。', {
                  dir: defaultDir || '—',
                })}
              </div>
            </div>

            {/* 上一次下载的结局：失败说原因，停止说「临时文件已删」——
                不然「我刚点过下载，怎么什么都没有」只能靠猜 */}
            {download?.state === 'error' && download.error ? (
              <span className='text-[12px] text-destructive'>{t('上次下载失败：{reason}', { reason: download.error })}</span>
            ) : null}
            {download?.state === 'canceled' ? (
              <span className='text-[12px] text-subtle'>{t('上次下载已停止，未下载完的临时文件已删除')}</span>
            ) : null}
            {formError ? (
              <span className='text-[12px] text-destructive'>{formError}</span>
            ) : null}
          </DialogBody>
        )}

        <DialogFooter>
          {downloading ? (
            <>
              <div className='mr-auto' />
              <Button variant='outline' onClick={onClose}>{t('后台继续')}</Button>
              <Button variant='destructive' disabled={stopping} onClick={() => void stop()}>
                {stopping ? t('正在停止…') : t('停止下载')}
              </Button>
            </>
          ) : (
            <>
              <div className='mr-auto' />
              <Button variant='outline' onClick={onClose}>{t('取消')}</Button>
              {/* pending 是别的内核动作（重启 / 删除）在途：那期间下载会和它抢队列 */}
              <Button variant='default' disabled={busy || snapshot.pending} onClick={() => void start()}>
                {busy ? t('正在开始…') : t('下载')}
              </Button>
            </>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
