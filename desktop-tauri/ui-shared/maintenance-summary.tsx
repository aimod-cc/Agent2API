import { Badge, Button, Card, CardHeader, CardTitle, Spinner, cn } from '@ui'
import { t } from '../ui-kit/src/i18n'
import { RunCounts, RunStatus } from './maintenance-shared'
import {
  duration,
  hasFailures,
  maintenanceActionLabel,
  maintenanceDescription,
  maintenanceLabel,
  shortStamp,
  stamp,
} from './maintenance-format'
import type { MaintenanceRun, MaintenanceTask, MaintenanceTaskId } from './maintenance-types'

/**
 * 左侧「维护项目」主控栏：展示全部维护任务健康度，并支持点选联动筛选右侧执行记录。
 */
type TaskRow = {
  task: MaintenanceTask
  run: MaintenanceRun | undefined
  running: boolean
  failed: boolean
}

export function MaintenanceSummary({
  tasks,
  latest,
  busy,
  selectedTaskId,
  onSelectTask,
  onRun,
  onShowLatest,
}: {
  tasks: MaintenanceTask[]
  latest: Partial<Record<MaintenanceTaskId, MaintenanceRun>>
  busy: MaintenanceTaskId | ''
  selectedTaskId: MaintenanceTaskId | ''
  onSelectTask(id: MaintenanceTaskId | ''): void
  onRun(id: MaintenanceTaskId): void
  onShowLatest(id: MaintenanceTaskId): void
}) {
  const rows: TaskRow[] = tasks.map(task => {
    const run = latest[task.id]
    const running = task.running || busy === task.id
    const failed = !running && Boolean(run) && (run!.status === 'failed' || run!.status === 'partial' || hasFailures(run!))
    return { task, run, running, failed }
  })

  const failedCount = rows.filter(row => row.failed).length

  const schedule = ({ task }: TaskRow) => {
    const follow = task.id === 'pluginCatalogCheck' && task.checkMode === 'follow'
    if (!task.enabled) return follow ? t('跟随软件（已关闭）') : t('自动维护已关闭')
    const cooling = Boolean(task.retryAt && task.retryAt > Date.now())
    return <>
      {follow ? <>{t('跟随软件')}<span className='maintenance-sep'>·</span></> : null}
      {t('每 {n} 分钟', { n: task.interval })}
      <span className='maintenance-sep'>·</span>
      {cooling
        ? <span className='maintenance-cooldown' title={stamp(task.retryAt)}>{t('冷却至 {time}', { time: shortStamp(task.retryAt) })}</span>
        : <span title={stamp(task.nextRunAt)}>{t('下次：{time}', { time: shortStamp(task.nextRunAt) })}</span>}
    </>
  }

  return (
    <Card className='maintenance-sidebar'>
      <CardHeader className='maintenance-sidebar-head'>
        <CardTitle className='text-[13px]'>{t('维护项目')}</CardTitle>
        {failedCount > 0 ? <Badge variant='destructive' shape='tag'>{t('{n} 项失败', { n: failedCount })}</Badge> : null}
      </CardHeader>

      <div className='maintenance-all-wrap'>
        <button
          type='button'
          aria-current={selectedTaskId === '' ? 'true' : undefined}
          onClick={() => onSelectTask('')}
          className={cn('maintenance-all-item', selectedTaskId === '' && 'is-active')}
        >
          <span>{t('全部维护项')}</span>
          <span className='maintenance-all-count'>{rows.length}</span>
        </button>
      </div>

      <div className='maintenance-task-list'>
        {rows.map(row => {
          const isActive = selectedTaskId === row.task.id
          return (
            <div
              key={row.task.id}
              role='button'
              tabIndex={0}
              aria-current={isActive ? 'true' : undefined}
              onClick={() => onSelectTask(row.task.id)}
              onKeyDown={event => {
                if (event.key === 'Enter' || event.key === ' ') {
                  event.preventDefault()
                  onSelectTask(row.task.id)
                }
              }}
              className={cn(
                'maintenance-task-item',
                isActive && 'is-active',
                row.failed && !isActive && 'is-failed',
              )}
            >
              <div className='maintenance-task-head'>
                <div className='flex min-w-0 items-center gap-2'>
                  <span className='maintenance-summary-name truncate' title={maintenanceDescription(row.task.id)}>
                    {maintenanceLabel(row.task.id)}
                  </span>
                  <RunStatus status={row.running ? 'running' : row.run?.status} pulse={row.running} />
                </div>
                <Button
                  size='xs'
                  variant={row.failed ? 'default' : 'outline'}
                  disabled={Boolean(busy) || row.task.running}
                  onClick={event => {
                    event.stopPropagation()
                    onRun(row.task.id)
                  }}
                >
                  {row.running ? <Spinner /> : null}
                  {row.running ? t('执行中…') : maintenanceActionLabel(row.task.id)}
                </Button>
              </div>

              <div className='maintenance-task-meta'>
                <div className='min-w-0 flex-1 overflow-hidden'>
                  {row.run
                    ? <RunCounts run={row.run} compact />
                    : <span className='maintenance-summary-line truncate' title={row.task.lastResult ?? undefined}>
                        {row.task.lastResult ? t('此前仅有汇总，尚无逐项记录') : t('尚未执行维护')}
                      </span>}
                </div>
                {row.run ? (
                  <span className='maintenance-task-time' title={stamp(row.run.startedAt)}>
                    {shortStamp(row.run.startedAt)} · {duration(row.run.durationMs)}
                  </span>
                ) : null}
              </div>

              <div className='maintenance-task-foot'>
                <div className='maintenance-task-schedule'>{schedule(row)}</div>
                <button
                  type='button'
                  className='maintenance-task-link'
                  disabled={!row.run}
                  onClick={event => {
                    event.stopPropagation()
                    onShowLatest(row.task.id)
                  }}
                >
                  {t('查看结果')}
                </button>
              </div>
            </div>
          )
        })}
      </div>
    </Card>
  )
}
