import { StatusDot } from '@ui'
import { t } from '../ui-kit/src/i18n'
import { resultLabel, statusLabel } from './maintenance-format'
import type { MaintenanceResultStatus, MaintenanceRun, MaintenanceRunStatus } from './maintenance-types'

/**
 * 维护页的状态与汇总零件，概览条、执行记录、逐项明细三处共用。
 *
 * 为什么单独一份：这三处显示的本来就是同一件事（这次执行成没成、跳过了几项），
 * 口径与外观必须一致 —— 分散写就容易出现「概览说是部分失败、表格写已完成」这类不一致。
 *
 * 形态取「一颗小圆点 + 文字」，而不是彩色药丸：一屏里同时有状态、结果、占比三类信息，
 * 每条都套一个底色块会把表格读成色卡；圆点只占 7px，颜色照样一眼可辨。
 */

export type StatusTone = 'success' | 'warning' | 'destructive' | 'muted'

/**
 * 行尾的右向箭头：几何与项目图标集（ui/icons.js 的 caretRight）一致 ——
 * 24 画布、1.8 粗细、圆头。就地画而不是取 window.wbIcons：手机端不加载那份图标脚本，
 * 而且这颗箭头只有这一个用法，没必要为它引入跨端依赖。
 */
export function RowChevron() {
  return <svg viewBox='0 0 24 24' width='14' height='14' aria-hidden='true' focusable='false'
    fill='none' stroke='currentColor' strokeWidth={1.8} strokeLinecap='round' strokeLinejoin='round'>
    <path d='m9 5 7 7-7 7' />
  </svg>
}

export function runTone(status: MaintenanceRunStatus): StatusTone {
  if (status === 'completed') return 'success'
  if (status === 'failed') return 'destructive'
  if (status === 'partial' || status === 'interrupted') return 'warning'
  return 'muted'
}

/** 一次执行的状态（执行中 / 已完成 / 部分失败 / 失败 / 全部跳过 / 执行中断） */
export function RunStatus({ status, pulse = false }: { status: MaintenanceRunStatus | undefined; pulse?: boolean }) {
  if (!status) return <span className='maintenance-status'><StatusDot variant='muted' />{t('暂无记录')}</span>
  return <span className='maintenance-status'>
    <StatusDot variant={runTone(status)} pulse={pulse || status === 'running'} />
    {statusLabel(status)}
  </span>
}

/** 单条明细的结果（成功 / 跳过 / 失败） */
export function ItemStatus({ status }: { status: MaintenanceResultStatus }) {
  const tone: StatusTone = status === 'refreshed' ? 'success' : status === 'failed' ? 'destructive' : 'muted'
  return <span className='maintenance-status'><StatusDot variant={tone} />{resultLabel(status)}</span>
}

/**
 * 本次成果：成功 / 跳过 / 失败。
 * 执行中与执行中断没有逐项结果，给一个破折号（原因在状态列与明细里），不再重复状态文字。
 */
export function RunCounts({ run }: { run: MaintenanceRun }) {
  if (run.status === 'running' || run.status === 'interrupted') {
    return <span className='maintenance-counts' title={run.status === 'running' ? t('维护正在执行，完成后会显示逐项结果') : t('该次执行未完成，没有完整明细')}>—</span>
  }
  return <div className='maintenance-counts'>
    <span>{t('成功')} <b>{run.refreshed}</b></span>
    <span className='maintenance-counts-sep'>·</span>
    <span>{t('跳过')} <b>{run.skipped}</b></span>
    <span className='maintenance-counts-sep'>·</span>
    <span className={run.failed ? 'maintenance-counts-bad' : undefined}>{t('失败')} <b>{run.failed}</b></span>
  </div>
}
