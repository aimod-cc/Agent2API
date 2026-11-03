import * as React from 'react'
import { Button, Dialog, DialogBody, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, SegmentedControl } from '@ui'
import { t } from '../i18n'
import {
  formatLogTime, groupTime, logCategory, logContext, logLevel, logRawText, logSource,
  logSummary, logText, shared, toast, type LogEntry, type LogGroup,
} from './logs-model'
import { DIALOG_SEG_CLASS } from './dialog-segmented'
import { Empty } from '@ui'

export function LogsList({ groups, empty, loading, listRef, onScroll, onDetail }: {
  groups: LogGroup[]
  empty: string
  loading: boolean
  listRef: React.RefObject<HTMLDivElement | null>
  onScroll: () => void
  onDetail: (group: LogGroup) => void
}) {
  return (
    <div className='log-list event-log-list' id='log-list' ref={listRef} onScroll={onScroll} aria-busy={loading}>
      <div className='event-log-head' aria-hidden='true'>
        <span>{t('时间')}</span><span>{t('级别')}</span><span>{t('来源')}</span>
        <span>{t('事件摘要')}</span><span>{t('次数')}</span><span />
      </div>
      {groups.length ? groups.map(group => {
        const entry = group.entry
        const context = logContext(entry)
        return (
          <div className={`event-log-row ${entry.level || 'info'}`} key={group.id}>
            <time className='event-log-time' dateTime={entry.ts ? new Date(entry.ts).toISOString() : undefined} title={groupTime(group)}>
              {formatLogTime(group.lastTs)}
              {group.count > 1 ? <span>{formatLogTime(group.firstTs, true)}–{formatLogTime(group.lastTs, true)}</span> : null}
            </time>
            <span className={`event-log-level ${entry.level || 'info'}`}>{logLevel(entry.level, t)}</span>
            <span className='event-log-source' title={logSource(entry, t)}>{logSource(entry, t)}</span>
            <div className='event-log-message'>
              <span className='event-log-summary' title={logSummary(entry, t)}>{logSummary(entry, t)}</span>
              {context ? <span className='event-log-context' title={context}>{context}</span> : null}
            </div>
            <span className={group.count > 1 ? 'event-log-count repeated' : 'event-log-count'} title={t('原始记录 {n} 条', { n: group.count })}>
              {group.count > 1 ? `×${group.count}` : '—'}
            </span>
            <Button variant='ghost' size='xs' onClick={() => onDetail(group)}
              aria-label={t('查看日志详情：{message}', { message: logSummary(entry, t) })}>{t('详情')}</Button>
          </div>
        )
      }) : <Empty className='log-empty'>{empty}</Empty>}
    </div>
  )
}

function fields(entry: LogEntry): [string, string][] {
  return [
    [t('级别'), logLevel(entry.level, t)], [t('分类'), logCategory(entry.category, t)],
    [t('来源'), logSource(entry, t)], [t('提供商'), logText(entry, 'provider')],
    [t('账号'), logText(entry, 'accountName') || logText(entry, 'account')],
    [t('模型'), logText(entry, 'model')], [t('操作'), logText(entry, 'action')],
    [t('关联标识'), logText(entry, 'callId')], [t('状态码'), logText(entry, 'status')],
    [t('耗时'), logText(entry, 'durationMs') ? `${logText(entry, 'durationMs')} ms` : ''],
  ].filter((pair): pair is [string, string] => Boolean(pair[1]))
}

export function LogDetail({ group, onClose }: { group: LogGroup | null; onClose: () => void }) {
  const [tab, setTab] = React.useState<'detail' | 'raw'>('detail')
  React.useEffect(() => setTab('detail'), [group?.id])
  if (!group) return null
  const entry = group.entry
  const copy = async () => {
    const text = logRawText(group.entries)
    try {
      const fallback = shared().copyToClipboard
      if (fallback) {
        if (!(await fallback(text))) throw new Error()
      } else await navigator.clipboard.writeText(text)
      toast(t('已复制 {n} 条日志', { n: group.count }), 'ok')
    } catch { toast(t('复制失败'), 'err') }
  }
  return (
    <Dialog open onOpenChange={open => { if (!open) onClose() }}>
      <DialogContent className='w-[min(860px,calc(100vw-48px))]'>
        <DialogHeader><DialogTitle>{t('日志详情')}</DialogTitle></DialogHeader>
        <DialogBody>
          <DialogDescription>{logSummary(entry, t)}</DialogDescription>
          <div className='event-log-detail-meta'>{groupTime(group)} · {t('原始记录 {n} 条', { n: group.count })}</div>
          {/* 两档视图：走组件库的分段控件（弹窗档尺寸）—— 原来是两颗按钮切底色假装成分段 */}
          <SegmentedControl<'detail' | 'raw'>
            aria-label={t('日志详情视图')}
            className={DIALOG_SEG_CLASS}
            value={tab}
            onValueChange={setTab}
            options={[
              { value: 'detail', label: t('事件详情') },
              { value: 'raw', label: t('原始记录') },
            ]}
          />
          {tab === 'detail' ? <>
            <dl className='event-log-detail-grid'>{fields(entry).map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}</dl>
            {logText(entry, 'detail') ? <div className='event-log-detail-block'><h3>{t('详细信息')}</h3><pre>{logText(entry, 'detail')}</pre></div> : null}
            <div className='event-log-detail-block'><h3>{t('原始消息')}</h3><pre>{entry.message || '—'}</pre></div>
            {entry.data ? <div className='event-log-detail-block'><h3>{t('附加数据')}</h3><pre>{JSON.stringify(entry.data, null, 2)}</pre></div> : null}
            {group.count > 1 ? <div className='event-log-detail-block'><h3>{t('发生时间')}</h3>
              <div className='event-log-occurrences'>{group.entries.map((item, index) => <span key={item.id ?? index}>{formatLogTime(item.ts)}</span>)}</div>
            </div> : null}
          </> : <div className='event-log-detail-block'><pre>{group.entries.map(item => JSON.stringify(item, null, 2)).join('\n\n')}</pre></div>}
        </DialogBody>
        <DialogFooter><Button variant='outline' onClick={() => void copy()}>{t('复制原始记录')}</Button>
          <div className='spacer' /><Button variant='ghost' onClick={onClose}>{t('关闭')}</Button></DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
