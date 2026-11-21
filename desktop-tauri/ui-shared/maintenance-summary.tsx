import { Button, Card, Spinner, Table, TableBody, TableCell, TableRow } from '@ui'
import { t } from '../ui-kit/src/i18n'
import { RunCounts, RunStatus } from './maintenance-shared'
import { duration, maintenanceActionLabel, maintenanceDescription, maintenanceLabel, sourceLabel, stamp } from './maintenance-format'
import type { MaintenanceRun, MaintenanceTask, MaintenanceTaskId } from './maintenance-types'

/**
 * 概览条：每项自动维护任务各占一行。
 *
 * 保留紧凑表格/窄屏堆叠结构，确保首屏留出充足空间给下方的执行记录。
 */
type TaskRow = {
  task: MaintenanceTask
  run: MaintenanceRun | undefined
  running: boolean
  stalled: boolean
}

export function MaintenanceSummary({ tasks, latest, busy, compact, onRun, onShowLatest }: {
  tasks: MaintenanceTask[]
  latest: Partial<Record<MaintenanceTaskId, MaintenanceRun>>
  busy: MaintenanceTaskId | ''
  compact: boolean
  onRun(id: MaintenanceTaskId): void
  onShowLatest(id: MaintenanceTaskId): void
}) {
  const rows: TaskRow[] = tasks.map(task => {
    const run = latest[task.id]
    const running = task.running || busy === task.id
    return { task, run, running, stalled: running || run?.status === 'running' || run?.status === 'interrupted' }
  })
  const schedule = ({ task }: TaskRow) => {
    const follow = task.id === 'pluginCatalogCheck' && task.checkMode === 'follow'
    if (!task.enabled) return follow ? t('跟随软件（已关闭）') : t('自动维护已关闭')
    return <>
      {follow ? <>{t('跟随软件')}<span className='maintenance-sep'>·</span></> : null}
      {t('每 {n} 分钟', { n: task.interval })}
      <span className='maintenance-sep'>·</span>
      {t('下次执行 {time}', { time: stamp(task.nextRunAt) })}
    </>
  }
  const cooldown = ({ task }: TaskRow) => task.enabled && task.retryAt && task.retryAt > Date.now()
    ? <span className='maintenance-cooldown'>{t('冷却至 {time}', { time: stamp(task.retryAt) })}</span>
    : null
  const lastRun = ({ task, run }: TaskRow) => run
    ? <>{t('最近一次')}<span className='maintenance-sep'>·</span>{stamp(run.startedAt)}<span className='maintenance-sep'>·</span>{sourceLabel(run.source)}<span className='maintenance-sep'>·</span>{duration(run.durationMs)}</>
    : <span title={task.lastResult ?? undefined}>{task.lastResult ? t('此前仅有汇总，尚无逐项记录') : t('尚未执行维护')}</span>

  if (compact) {
    return <Card className='maintenance-summary'>
      {rows.map((row, index) => <div key={row.task.id} className={index ? 'maintenance-summary-block maintenance-divided' : 'maintenance-summary-block'}>
        <div className='maintenance-summary-block-head'>
          <span className='maintenance-summary-name' title={maintenanceDescription(row.task.id)}>{maintenanceLabel(row.task.id)}</span>
          <RunStatus status={row.running ? 'running' : row.run?.status} pulse={row.running} />
        </div>
        <p className='maintenance-summary-line'>{lastRun(row)}</p>
        {row.run ? <RunCounts run={row.run} /> : null}
        <p className='maintenance-summary-line'>{schedule(row)}{cooldown(row)}</p>
        <div className='maintenance-summary-block-actions'>
          <Button size='sm' variant='ghost' disabled={!row.run} onClick={() => onShowLatest(row.task.id)}>{t('查看结果')}</Button>
          <Button size='sm' disabled={Boolean(busy) || row.task.running} onClick={() => onRun(row.task.id)}>
            {row.running ? <Spinner /> : null}{row.running ? t('执行中…') : maintenanceActionLabel(row.task.id)}
          </Button>
        </div>
      </div>)}
    </Card>
  }

  return <Card className='maintenance-summary'>
    <Table className='maintenance-summary-table'>
      <TableBody>
        {rows.map(row => <TableRow key={row.task.id}>
          <TableCell className='maintenance-summary-name' title={maintenanceDescription(row.task.id)}>{maintenanceLabel(row.task.id)}</TableCell>
          <TableCell><RunStatus status={row.running ? 'running' : row.run?.status} pulse={row.running} /></TableCell>
          <TableCell className='maintenance-summary-line'>{lastRun(row)}</TableCell>
          <TableCell>{row.run ? <RunCounts run={row.run} /> : null}</TableCell>
          <TableCell className='maintenance-summary-line'>{schedule(row)}{cooldown(row)}</TableCell>
          <TableCell className='maintenance-summary-actions'>
            <Button size='sm' variant='ghost' disabled={!row.run} onClick={() => onShowLatest(row.task.id)}>{t('查看结果')}</Button>
            <Button size='sm' disabled={Boolean(busy) || row.task.running} onClick={() => onRun(row.task.id)}>
              {row.running ? <Spinner /> : null}{row.running ? t('执行中…') : maintenanceActionLabel(row.task.id)}
            </Button>
          </TableCell>
        </TableRow>)}
      </TableBody>
    </Table>
  </Card>
}
