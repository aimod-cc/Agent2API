import * as React from 'react'
import {
  Badge, Button, Checkbox, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader,
  DialogTitle, Input, Label,
} from '@ui'
import { t } from '../i18n'
import { patch, saveScheduleEntry, store } from './checkin-store'
import type { ScheduleEntry } from './checkin-types'
import type { ScheduleProviderRow } from '../../../ui-shared/checkin'
import { ProviderLogo } from './checkin-row'
import {
  checkinEnd, checkinRangeText, effectiveScheduleTime, groupScheduleRows, providerCheckinEnd,
  schedulePayload, validScheduleTime, type ScheduleOption,
} from './checkin-schedule-model'
import { defaultCheckinEnd, validCheckinRange } from '../../../ui-shared/checkin-window'

function ScheduleEditorForm({ entry, options, defaultRetryTime, onClose }: {
  entry: ScheduleEntry | null
  options: ScheduleOption[]
  defaultRetryTime: string
  onClose(): void
}): React.ReactElement {
  const [name, setName] = React.useState(entry?.label ?? '')
  const [time, setTime] = React.useState(entry?.time || '00:01')
  const [timeEnd, setTimeEnd] = React.useState(checkinEnd(entry?.time || '00:01', entry?.timeEnd))
  const [providers, setProviders] = React.useState(entry ? [...entry.providers] : options.filter(item => item.available).map(item => item.id))
  const [providerTimes, setProviderTimes] = React.useState<Record<string, string>>({ ...entry?.providerTimes })
  const [search, setSearch] = React.useState('')
  const rows: ScheduleProviderRow[] = options.filter(option => providers.includes(option.id)).map(option => ({
    id: option.id, label: option.label, time: effectiveScheduleTime(time, providerTimes, option),
    timeEnd: providerCheckinEnd(effectiveScheduleTime(time, providerTimes, option), time, timeEnd),
    builtin: option.builtinTime ?? null, firedToday: false,
  }))
  const groups = groupScheduleRows(rows)
  const canSave = !store.saving && validCheckinRange(time, timeEnd) && providers.length > 0
    && Object.values(providerTimes).every(value => !value || validScheduleTime(value))
  const visible = options.filter(option => t(option.label).toLocaleLowerCase().includes(search.trim().toLocaleLowerCase()))
  const toggle = (id: string, checked: boolean) => setProviders(previous => checked
    ? [...new Set([...previous, id])] : previous.filter(item => item !== id))
  const resetTime = (id: string) => setProviderTimes(previous => {
    const next = { ...previous }
    delete next[id]
    return next
  })
  const submit = () => {
    if (!canSave) return
    const times = Object.fromEntries(Object.entries(providerTimes).filter(([, value]) => value))
    void saveScheduleEntry(entry
      ? schedulePayload(entry, { label: name.trim(), time, timeEnd, providers, providerTimes: times })
      : {
          label: name.trim(), time, timeEnd, providers, providerTimes: times, enabled: true,
          retryTime: defaultRetryTime, retryGapMinutes: 10, retryLimit: 3,
        })
  }

  return <>
    <DialogHeader><DialogTitle>{entry ? t('编辑签到计划') : t('新建签到计划')}</DialogTitle></DialogHeader>
    <DialogBody className='ck-schedule-editor-body grid gap-0 p-0'>
      <div className='ck-schedule-editor-fields'>
        <section className='ck-schedule-form-section'>
          <h3>{t('基本设置')}</h3>
          <div className='ck-schedule-basic-fields'>
            <Label className='ck-schedule-field grid items-stretch'>
              <span>{t('计划名称')} <small>{t('选填')}</small></span>
              <Input value={name} disabled={store.saving} placeholder={t('例如：每日自动签到')}
                onChange={event => setName(event.currentTarget.value)} />
            </Label>
            <Label className='ck-schedule-field grid items-stretch'>
              <span>{t('随机签到开始')}</span>
              <Input type='time' value={time} disabled={store.saving} className='font-mono tabular-nums'
                onChange={event => {
                  const value = event.currentTarget.value
                  setTime(value)
                  if (!validCheckinRange(value, timeEnd)) setTimeEnd(defaultCheckinEnd(value))
                }} />
            </Label>
            <Label className='ck-schedule-field grid items-stretch'>
              <span>{t('随机签到结束')}</span>
              <Input type='time' value={timeEnd} disabled={store.saving} className='font-mono tabular-nums'
                onChange={event => setTimeEnd(event.currentTarget.value)} />
            </Label>
          </div>
          <p className='ck-schedule-hint'>{t('每个账号每天在区间内随机执行一次；开始与结束相同则按固定时刻执行。')}</p>
          {!validCheckinRange(time, timeEnd) ? <p className='ck-schedule-error'>{t('结束时间不能早于开始时间，暂不支持跨天区间。')}</p> : null}
        </section>
        <section className='ck-schedule-form-section'>
          <div className='ck-schedule-form-title'><h3>{t('平台与随机签到区间')}</h3>
            <span>{t('已选 {n} / {total} 个平台', { n: providers.length, total: options.length })}</span>
          </div>
          <div className='ck-schedule-picker-tools'>
            <Input type='search' value={search} disabled={store.saving} placeholder={t('搜索平台')}
              aria-label={t('搜索签到平台')} onChange={event => setSearch(event.currentTarget.value)} />
            <Button variant='link' size='xs' disabled={store.saving}
              onClick={() => setProviders(options.map(item => item.id))}>{t('全选')}</Button>
            <Button variant='ghost' size='xs' disabled={store.saving}
              onClick={() => setProviders([])}>{t('清空')}</Button>
          </div>
          <div className='ck-schedule-picker'>
            {visible.map(option => {
              const checked = providers.includes(option.id)
              const override = providerTimes[option.id] ?? ''
              return <div className={`ck-schedule-picker-row${checked ? ' selected' : ''}`} key={option.id}>
                <Label className='ck-schedule-picker-label'>
                  <Checkbox checked={checked} disabled={store.saving}
                    aria-label={t('选择 {name}', { name: t(option.label) })}
                    onCheckedChange={next => toggle(option.id, next === true)} />
                  <ProviderLogo id={option.id} label={t(option.label)} />
                  <span className='ck-schedule-picker-name'>{t(option.label)}
                    {!option.available ? <small>{t('插件未就绪，范围仍保留')}</small> : null}
                  </span>
                </Label>
                <div className='ck-schedule-picker-time'>
                  <Input type='time' value={override || effectiveScheduleTime(time, providerTimes, option)}
                    disabled={!checked || store.saving} className='w-[108px] font-mono tabular-nums'
                    aria-label={t('{name}签到时间', { name: t(option.label) })}
                    onChange={event => {
                      const value = event.currentTarget.value
                      setProviderTimes(previous => ({ ...previous, [option.id]: value }))
                    }} />
                  <small className='font-mono tabular-nums'>{checkinRangeText(
                    effectiveScheduleTime(time, providerTimes, option),
                    providerCheckinEnd(effectiveScheduleTime(time, providerTimes, option), time, timeEnd),
                  )}</small>
                  {override ? <Button size='xs' variant='link' disabled={store.saving} onClick={() => resetTime(option.id)}>
                    {t('恢复默认')}
                  </Button> : <span>{option.builtinTime ? t('平台专属') : t('跟随默认')}</span>}
                </div>
              </div>
            })}
            {!visible.length ? <p className='ck-schedule-hint'>{t('没有匹配的平台')}</p> : null}
          </div>
          {!providers.length ? <p className='ck-schedule-error'>{t('请至少选择一个签到平台')}</p> : null}
        </section>
        <div className='ck-schedule-editor-note'>{entry
          ? t('这里只修改定时签到；补签和活动重试配置保持原样，在「定时补签」页单独设置。')
          : t('新计划默认开启补签和活动窗口重试；保存后可到「定时补签」页单独调整。')}</div>
      </div>
      <aside className='ck-schedule-preview'>
        <h3>{t('实际签到安排')}</h3><p>{t('北京时间 · 每天重复 · 随设置更新')}</p>
        <div className='ck-schedule-preview-groups'>
          {groups.map(group => <div className='ck-schedule-preview-group' key={group.time}>
            <strong className='font-mono tabular-nums'>{checkinRangeText(group.time, group.timeEnd) || '—'}</strong>
            <Badge variant='outline' shape='tag'>{t('{n} 个平台', { n: group.rows.length })}</Badge>
            <div>{group.rows.map(row => t(row.label)).join('、')}</div>
          </div>)}
          {!groups.length ? <p>{t('选择平台后显示实际签到时间')}</p> : null}
        </div>
        <p className='ck-schedule-preview-note'>{t('专属或自定义时间作为区间起点，沿用默认区间长度；每天重新随机，重启不改变当天安排。')}</p>
      </aside>
    </DialogBody>
    <DialogFooter>
      <span className='ck-schedule-footer-note'>{t('{n} 个平台 · {groups} 组签到时间', { n: providers.length, groups: groups.length })}</span>
      <Button variant='outline' size='sm' disabled={store.saving} onClick={onClose}>{t('取消')}</Button>
      <Button size='sm' disabled={!canSave} onClick={submit}>{store.saving ? t('保存中…') : t('保存计划')}</Button>
    </DialogFooter>
  </>
}

export function CheckinScheduleEditor({ entries, options, defaultRetryTime }: {
  entries: ScheduleEntry[]
  options: ScheduleOption[]
  defaultRetryTime: string
}): React.ReactElement | null {
  const id = store.editingId
  if (!id) return null
  const entry = id === 'new' ? null : entries.find(item => item.id === id)
  if (entry === undefined) return null
  const close = () => patch({ editingId: null })
  return <Dialog open onOpenChange={open => { if (!open && !store.saving) close() }}>
    <DialogContent className='ck-schedule-editor-dialog w-[min(1000px,calc(100vw-48px))] max-[767px]:w-[calc(100vw-24px)]'>
      <ScheduleEditorForm key={id} entry={entry} options={options}
        defaultRetryTime={defaultRetryTime} onClose={close} />
    </DialogContent>
  </Dialog>
}
