import * as React from 'react'
import {
  Badge,
  Button,
  Dialog,
  DialogBody,
  DialogContent,
  DialogHeader,
  DialogTitle,
  Input,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Switch,
  cn,
} from '@ui'
import {
  buildProxyPick, errorMessage, fetchProxySelection, openExternal, saveProxy,
  shared, toast, type ProxySelection, type UpdateTokenStatus,
  EMPTY_PROXY_SELECTION,
} from './update-shared'
import { t } from '../i18n'

/**
 * 「更新设置」弹窗（设置页「软件更新」面板头部那颗按钮打开）。
 *
 * ── 三块设置，各自的保存时机 ────────────────────────────────
 *   · 自动检查更新：开关**拨动即保存**；间隔填完按「保存」（或回车）——
 *     数字输入有校验与「改没改」之分，配上明确确认键；关闭后面板上的
 *     「检查更新」仍可手动触发，只是不再到点自动查。
 *   · 出网代理：下拉**选中即保存**（与账号页代理列同一交互，省一个确认键），
 *     检查更新与下载安装包立即走新线路；
 *   · GitHub 令牌：粘贴 → 点「保存」（有明确输入动作的设置就该有明确确认）。
 *
 * 「自动检查更新」就是原定时任务页的「软件版本检查」（同一条后端任务，
 * id 'updateCheck'，走同一组 /api/scheduled-tasks 接口）—— 配置入口收进本
 * 弹窗、定时任务页不再渲染那一行，版本相关的设置收在一个地方。
 *
 * ── 令牌「已填写」的口径 ─────────────────────────────────────
 * 保存成功后**界面永远不再显示令牌本体**（后端 `/api/update/token` 只回
 * filled / origin，想回显也拿不到）—— 重新打开弹窗看到的只有一枚「已填写」
 * 徽章。想换令牌就再粘一次（覆盖保存），想撤销用「清除」。
 *
 * ── 排版 ──────────────────────────────────────────────────
 * 弹窗只放控件与一行状态说明，成段的解释全部收进悬停提示与面板顶部的 tip：
 * 弹窗是「改设置」的地方，不是读文档的地方。底部也不放「关闭」—— 组件库
 * Dialog 自带右上角 ✕，Esc / 点遮罩同样能关。
 *
 * ── 状态归属 ───────────────────────────────────────────────
 * 三块设置的读数都是本组件的本地状态（每次打开现拉），不进 update-panel 的
 * 模块快照：面板那套快照服务的是「检查 / 下载」的命令式流程，弹窗跟着开跟着
 * 关，塞进去只会让两棵树多一层没必要的耦合。类型与读写函数在 update-shared.ts。
 */

/** 「自动检查更新」在后端注册表里的任务 id（`config::KEY_UPDATE_CHECK`） */
const UPDATE_CHECK_TASK_ID = 'updateCheck'
/**
 * 检查间隔的边界（分钟）—— 与后端 `INTERVAL_MIN_MINUTES` / `INTERVAL_MAX_MINUTES`
 * 一致；后端本来随任务下发 min/max，这里只取开关与间隔两个值，边界抄一份常量
 * 并注明出处（改后端时这两处要一起动）。
 */
const CHECK_INTERVAL_MIN_MINUTES = 1
const CHECK_INTERVAL_MAX_MINUTES = 1440

/**
 * 快捷跳转：GitHub 令牌创建页，query 预填「备注」。
 *
 * **刻意不预选任何权限**（不带 scopes）：无权限（no scopes）的经典令牌就能读
 * 公开仓库的发布信息 —— 本项目检查的正是它 —— 并把限额提到 5000 次/小时；
 * 预选 repo 反而是过度授权（那是私有仓库的完整读写）。用户打开页面直接点
 * 底部的 Generate token 即可。
 */
const GITHUB_TOKENS_URL = 'https://github.com/settings/tokens/new?description='
  + encodeURIComponent('Agent2API 更新检查')

/** 令牌一栏的状态徽章 + 一句说明（完整原因放徽章的悬停提示里，不占版面） */
function tokenStatus(token: UpdateTokenStatus | null): { badge: React.ReactNode; hint: string } {
  if (!token) {
    return { badge: <Badge shape='tag' variant='outline'>{t('读取中')}</Badge>, hint: '' }
  }
  if (token.error) {
    return {
      badge: <Badge shape='tag' variant='destructive' title={token.error}>{t('无法解密')}</Badge>,
      hint: t('重新粘贴保存一次即可自愈'),
    }
  }
  if (token.filled && token.origin === 'stored') {
    return {
      badge: <Badge shape='tag' variant='success'>{t('已填写')}</Badge>,
      hint: t('已加密存储在本地，不显示具体值；粘贴新令牌可覆盖'),
    }
  }
  if (token.filled) {
    return {
      badge: <Badge shape='tag' variant='secondary'>{t('环境变量')}</Badge>,
      hint: t('已配置 GITHUB_TOKEN 环境变量，界面保存的令牌优先于它'),
    }
  }
  return {
    badge: <Badge shape='tag' variant='outline'>{t('未填写')}</Badge>,
    hint: t('填写后检查限额 60 → 5000 次/小时（创建令牌无需勾选任何权限）'),
  }
}

export function UpdateSettingsDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const [selection, setSelection] = React.useState<ProxySelection>(EMPTY_PROXY_SELECTION)
  const [token, setToken] = React.useState<UpdateTokenStatus | null>(null)
  const [draft, setDraft] = React.useState('')
  const [busy, setBusy] = React.useState(false)
  // 「自动检查更新」的读数与间隔草稿：null = 还没读到（开关禁用直到读到）
  const [checkTask, setCheckTask] = React.useState<{ enabled: boolean; interval: number } | null>(null)
  const [intervalDraft, setIntervalDraft] = React.useState('')
  const [checkBusy, setCheckBusy] = React.useState(false)
  const pick = buildProxyPick(selection)

  // 每次打开都现拉三块设置的读数：值可能被上一轮弹窗或环境改过，
  // 请求都便宜，不值得为它做缓存
  React.useEffect(() => {
    if (!open) return
    setDraft('')
    void fetchProxySelection().then(setSelection)
    void (async () => {
      try {
        setToken((await shared().workbuddyDesktop?.getUpdateToken()) ?? null)
      } catch {
        // 读不到按「未知」显示（token 保持 null →「读取中」徽章），保存动作会带出新状态
      }
    })()
    void (async () => {
      try {
        const tasks = (await shared().workbuddyDesktop?.getScheduledTasks?.())?.tasks ?? []
        const found = tasks.find(task => task.id === UPDATE_CHECK_TASK_ID)
        const enabled = found?.enabled === true
        const interval = Math.max(CHECK_INTERVAL_MIN_MINUTES, Math.round(Number(found?.interval) || 0))
        setCheckTask({ enabled, interval })
        setIntervalDraft(String(interval))
      } catch {
        // 读不到保持 null：开关与间隔保持禁用，保存动作会带出新状态
      }
    })()
  }, [open])

  /** 保存「自动检查更新」的一块配置，成功后以后端返回值为准回写草稿 */
  async function saveCheckTask(patch: { enabled?: boolean; interval?: number }): Promise<void> {
    if (checkBusy) return
    setCheckBusy(true)
    try {
      const saved = await shared().workbuddyDesktop?.saveScheduledTask?.(UPDATE_CHECK_TASK_ID, patch)
      const enabled = saved?.enabled === true
      const interval = Math.max(CHECK_INTERVAL_MIN_MINUTES, Math.round(Number(saved?.interval) || 0))
      setCheckTask({ enabled, interval })
      setIntervalDraft(String(interval))
      toast(t('✅ 自动检查更新已保存'))
    } catch (error) {
      toast(t('保存失败：{error}', { error: errorMessage(error) }), 'err')
    } finally {
      setCheckBusy(false)
    }
  }

  /** 间隔输入的保存：范围与「改没改」都在这里挡（未改时按钮本来就是禁用的） */
  async function saveCheckInterval(): Promise<void> {
    const value = Math.round(Number(intervalDraft))
    if (!Number.isFinite(value) || value < CHECK_INTERVAL_MIN_MINUTES || value > CHECK_INTERVAL_MAX_MINUTES) {
      toast(t('检查间隔必须是 {min} ~ {max} 分钟', {
        min: CHECK_INTERVAL_MIN_MINUTES,
        max: CHECK_INTERVAL_MAX_MINUTES,
      }), 'err')
      return
    }
    if (checkTask && value === checkTask.interval) return
    await saveCheckTask({ interval: value })
  }

  async function handlePick(value: string): Promise<void> {
    const result = await saveProxy(value) // 失败在内部 toast；成功也顺带播报
    if (result) setSelection(prev => ({ ...prev, proxyChoice: result.choice }))
  }

  async function saveToken(): Promise<void> {
    const value = draft.trim()
    if (!value) {
      toast(t('请先粘贴 GitHub 令牌'), 'err')
      return
    }
    setBusy(true)
    try {
      const result = await shared().workbuddyDesktop?.setUpdateToken({ token: value })
      setToken(result ?? null)
      setDraft('')
      if (result?.saved === false) toast(t('令牌已生效，但写入磁盘失败（重启后会丢失）'), 'err')
      else toast(t('✅ GitHub 令牌已保存（加密存储，界面不再显示）'))
    } catch (error) {
      toast(t('保存失败：{error}', { error: errorMessage(error) }), 'err')
    } finally {
      setBusy(false)
    }
  }

  async function clearToken(): Promise<void> {
    setBusy(true)
    try {
      const result = await shared().workbuddyDesktop?.setUpdateToken({ token: null })
      setToken(result ?? null)
      toast(t('✅ 已清除界面保存的 GitHub 令牌（环境变量若配置过则继续生效）'))
    } catch (error) {
      toast(t('清除失败：{error}', { error: errorMessage(error) }), 'err')
    } finally {
      setBusy(false)
    }
  }

  const status = tokenStatus(token)
  const stored = Boolean(token?.error) || token?.origin === 'stored'
  const placeholder = token?.filled && token?.origin !== 'env'
    ? t('已填写（粘贴新令牌可覆盖）')
    : t('粘贴 GitHub 令牌（ghp_… / github_pat_…）')

  return (
    // 受控 open（面板按 settingsOpen 条件渲染本组件）：关窗一律由 onClose 收口，
    // Esc / 点遮罩 / 右上角 ✕ 由 Base UI Dialog 内建
    <Dialog open={open} onOpenChange={next => { if (!next) onClose() }}>
      {/* 默认 620px 对这两节内容太宽（右侧一截空白），收窄成一个紧凑的设置小窗 */}
      <DialogContent className='w-[min(460px,calc(100vw-48px))]'>
        <DialogHeader>
          <DialogTitle>{t('更新设置')}</DialogTitle>
        </DialogHeader>
        {/* DialogBody 自带 gap-4，这里收紧到 gap-3.5；每节是一个子元素，
            节内间距自己控（不与 gap 叠加） */}
        <DialogBody className='gap-3.5'>
          {/* 自动检查更新：开关拨动即保存；间隔带校验与明确确认键。
              读数没到位时控件禁用（保持 null → 不猜开关状态） */}
          <div>
            <div className='flex items-center justify-between gap-2'>
              <div className='text-[13px] font-semibold text-foreground'>{t('自动检查更新')}</div>
              <Switch
                checked={checkTask?.enabled === true}
                disabled={checkBusy || !checkTask}
                onCheckedChange={next => void saveCheckTask({ enabled: next === true })}
                aria-label={t('自动检查更新')}
              />
            </div>
            <div className='mt-1.5 flex items-center gap-2'>
              <Input
                type='number'
                min={CHECK_INTERVAL_MIN_MINUTES}
                max={CHECK_INTERVAL_MAX_MINUTES}
                step={1}
                className='w-[110px]'
                disabled={checkBusy || !checkTask || checkTask.enabled !== true}
                value={intervalDraft}
                onChange={event => setIntervalDraft(event.currentTarget.value)}
                onKeyDown={event => {
                  if (event.key === 'Enter') {
                    event.preventDefault()
                    void saveCheckInterval()
                  }
                }}
                aria-label={t('检查间隔（分钟）')}
              />
              <span className='text-[12px] text-subtle'>
                {t('分钟（{min} ~ {max}）', {
                  min: CHECK_INTERVAL_MIN_MINUTES,
                  max: CHECK_INTERVAL_MAX_MINUTES,
                })}
              </span>
              <Button
                variant='outline'
                size='sm'
                disabled={
                  checkBusy || !checkTask
                  || Math.round(Number(intervalDraft)) === checkTask.interval
                }
                onClick={() => void saveCheckInterval()}
              >
                {t('保存')}
              </Button>
            </div>
            <div className='mt-1.5 text-[12px] text-subtle'>
              {t('到点自动向 GitHub 查询新版本，查到就走「检测到更新」弹窗；关闭后「检查更新」按钮仍可手动触发')}
            </div>
          </div>

          <div className='h-px bg-hairline' />

          {/* 出网代理：说明都在下拉的悬停提示里，这里只留标题与控件 */}
          <div>
            <div className='text-[13px] font-semibold text-foreground'>{t('出网代理')}</div>
            <div className='mt-1.5'>
              <Select value={pick.current} onValueChange={value => void handlePick(String(value))}>
                {/* 线路不可用时描边标红（照账号页 ProxyCell 的口径），原因看 title */}
                <SelectTrigger
                  className={cn('w-full', pick.broken && 'border-destructive-bd')}
                  title={pick.title}
                  aria-label={t('更新出网代理')}
                >
                  <SelectValue className='min-w-0 truncate'>{pick.selected?.label || t('直连')}</SelectValue>
                </SelectTrigger>
                <SelectContent>
                  {pick.items.map(item => (
                    <SelectItem key={item.value} value={item.value} disabled={item.disabled}>{item.label}</SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          </div>

          <div className='h-px bg-hairline' />

          {/* GitHub 令牌：标题行右侧就是快捷跳转（预设好备注、无需勾选权限） */}
          <div>
            <div className='flex items-center justify-between gap-2'>
              <div className='text-[13px] font-semibold text-foreground'>{t('GitHub 令牌')}</div>
              <Button
                variant='outline'
                size='xs'
                title={t('在默认浏览器中打开 GitHub 的令牌创建页（备注已预设，无需勾选任何权限，直接点 Generate token）')}
                onClick={() => void openExternal(GITHUB_TOKENS_URL)}
              >
                {t('打开 GitHub 令牌页面')}
              </Button>
            </div>
            <div className='mt-1.5 flex items-center gap-2'>
              {status.badge}
              <span className='text-[12px] text-subtle'>{status.hint}</span>
            </div>
            <div className='mt-2 flex items-center gap-2'>
              {/* 密码形态：防肩窥；不回显是后端口径，这里只是不把粘贴值展示成明文 */}
              <Input
                type='password'
                className='flex-1'
                placeholder={placeholder}
                autoComplete='new-password'
                spellCheck={false}
                value={draft}
                onChange={event => setDraft(event.currentTarget.value)}
                onKeyDown={event => {
                  if (event.key === 'Enter') {
                    event.preventDefault()
                    void saveToken()
                  }
                }}
              />
              <Button variant='default' size='sm' disabled={busy || !draft.trim()} onClick={() => void saveToken()}>
                {t('保存')}
              </Button>
              {stored ? (
                <Button variant='outline' size='sm' disabled={busy} onClick={() => void clearToken()}>
                  {t('清除')}
                </Button>
              ) : null}
            </div>
          </div>
        </DialogBody>
      </DialogContent>
    </Dialog>
  )
}
