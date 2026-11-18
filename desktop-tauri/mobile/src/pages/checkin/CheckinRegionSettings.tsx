import { useState } from 'react'
import { Button, Input, Switch, Textarea , Card, Label } from '@ui'
import { SectionHead } from '../../components/ui'
import { runTravel, saveKeepalive, saveTravel, type CheckinCenter } from '../../api/checkin'
import { store } from '../../state/store'
import { formatStamp } from '../../lib/format'
import type { CheckinAction } from '../CheckinPage'
import { t } from '../../i18n'
import { apiCall } from '../../api/client'
import { ModelApiKeyField } from '../../../../ui-shared/model-api-key-field'
import { useModelApiKeys } from '../../../../ui-shared/use-model-api-keys'
import { PluginTaskSettings } from '../../../../ui-shared/plugin-task-settings'
import { isCheckinTask } from '../../../../ui-shared/plugin-task-types'

export function CheckinRegionSettings({ center, busy, action, onChange }: {
  center: CheckinCenter
  busy: string
  action: CheckinAction
  onChange: () => Promise<void>
}) {
  const panels = new Set(center.adminPanels || [])
  return (
    <>
      {panels.has('daily_keepalive') && center.keepalive ? <KeepalivePanel center={center} busy={busy} action={action} /> : null}
      {panels.has('growth_tasks') && center.travel && center.travel.engine !== 'wasm_tasks'
        ? <TravelPanel center={center} busy={busy} action={action} /> : null}
      {(center.pluginTasks ?? []).some(isCheckinTask) ? <div className='m-section'>
        <PluginTaskSettings rows={(center.pluginTasks ?? []).filter(isCheckinTask)} t={t} disabled={Boolean(busy)} expanded
          request={(method, path, body) => apiCall(path, { method, body })} refresh={onChange}
          title={t('签到与成长自动任务')}
          description={t('插件提供任务能力，主程序统一管理开关、执行时间和结果。与每日签到计划分别设置，不会因打开页面自动执行。')} />
      </div> : null}
    </>
  )
}

function KeepalivePanel({ center, busy, action }: { center: CheckinCenter; busy: string; action: CheckinAction }) {
  const [draft, setDraft] = useState<string | null>(null)
  const keyState = useModelApiKeys(() => apiCall('/api/model-access/keys'))
  const text = draft ?? center.keepalive.models.join('\n')
  const save = async () => {
    const models = [...new Set(text.split(/[\n,，、]+/).map(item => item.trim()).filter(Boolean))]
    const result = await action('keepalive', () => saveKeepalive(models), t('模型链已保存'))
    if (result) setDraft(null)
  }

  return (
    <div className='m-section'>
      <SectionHead title={t('保活模型链')} />
      <Card className='m-card m-card-pad'>
        <ModelApiKeyField {...keyState} value={center.keepalive.apiKeyId ?? null} disabled={Boolean(busy)}
          onRefresh={() => { void keyState.refresh().catch(() => {}) }} t={t} id='keepalive-api-key'
          onChange={apiKeyId => { void action('keepalive', () => saveKeepalive(center.keepalive.models, apiKeyId), t('保活 API Key 已保存')) }} />
        {center.keepalive.authError ? <p role='status' className='m-ck-error'>{center.keepalive.authError}</p> : null}
        <p className='m-ck-note'>{t('按顺序尝试免费模型，一行一个；清空并保存恢复默认链。')}</p>
        <Textarea className='text-base' rows={4} value={text} disabled={Boolean(busy)}
          aria-label={t('保活模型链')} onChange={event => setDraft(event.currentTarget.value)} />
        <div className='m-ck-actions'>
          <Button size='sm' variant='outline' disabled={Boolean(busy)} onClick={() => setDraft(center.keepalive.defaultModels.join('\n'))}>
            {t('填入缺省链')}
          </Button>
          <Button size='sm' disabled={Boolean(busy) || draft === null} onClick={() => void save()}>{busy === 'keepalive' ? t('保存中…') : t('保存')}</Button>
        </div>
      </Card>
    </div>
  )
}

function TravelPanel({ center, busy, action }: { center: CheckinCenter; busy: string; action: CheckinAction }) {
  const [draft, setDraft] = useState<string | null>(null)
  const travel = center.travel
  const text = draft ?? (travel.times || []).join(', ')

  const save = async () => {
    const times = text.split(/[,，\s]+/).map(item => item.trim()).filter(Boolean)
    if (!times.length || times.length > 6 || times.some(time => !/^([01]\d|2[0-3]):[0-5]\d$/.test(time))) {
      store.toast(t('请填写 1–6 个 HH:MM 时刻，以逗号分隔'), 'err')
      return
    }
    const result = await action('travel-save', () => saveTravel({ times }), t('已保存'))
    if (result) setDraft(null)
  }

  const run = async () => {
    const result = await action('travel-run', runTravel)
    if (!result) return
    const failed = Number(result.summary?.failed) || 0
    store.toast(result.outcome === 'skipped' ? result.message || t('本轮已跳过')
      : t('猫猫旅行：{n} 个动作，+{credit} 积分，{failed} 个失败', {
        n: result.summary?.actions?.length || 0, credit: Number(result.summary?.credit) || 0, failed,
      }), failed > 0 ? 'err' : 'ok')
  }

  return (
    <div className='m-section'>
      <SectionHead title={t('猫猫旅行定时排程')} />
      <Card className='m-card m-card-pad'>
        <div className='m-ck-toolbar'>
          <div className='m-ck-grow'>
            <div className='m-ck-title'>{t('自动巡检')}</div>
            <div className='m-ck-note'>{t('一趟推进一步，与签到排程互相独立')}</div>
          </div>
          <Switch checked={travel.enabled} disabled={Boolean(busy)} aria-label={t('自动巡检')}
            onCheckedChange={enabled => void action('travel-toggle', () => saveTravel({ enabled }), t('已保存'))} />
        </div>
        {travel.available === false ? <p className='m-ck-note'>{t('自动巡检已暂停：{reason}', { reason: travel.pausedReason || '' })}</p> : null}
        <Label className='m-ck-field'>{t('每天时刻（北京时间）')}
          <Input className='text-base' value={text} placeholder='09:00, 21:00' disabled={Boolean(busy)}
            onChange={event => setDraft(event.currentTarget.value)} />
        </Label>
        <div className='m-ck-actions'>
          <Button size='sm' disabled={Boolean(busy) || draft === null} onClick={() => void save()}>{t('保存')}</Button>
          <Button size='sm' variant='outline' disabled={Boolean(busy) || travel.running || travel.available === false} onClick={() => void run()}>
            {busy === 'travel-run' || travel.running ? t('巡检中…') : t('立即巡检全部')}
          </Button>
        </div>
        {travel.firedToday?.length ? <p className='m-ck-note'>{t('今天已执行：{times}', { times: travel.firedToday.join(' / ') })}</p> : null}
        {travel.lastResult?.at ? <p className='m-ck-note'>{t('上次执行：{time}', { time: formatStamp(travel.lastResult.at) })}</p> : null}
        {travel.lastResult?.error ? <p className='m-ck-error'>{travel.lastResult.error}</p> : null}
        {travel.lastResult?.actions?.map((item, index) => <p className='m-ck-note' key={`${item.accountId}:${index}`}>
          {item.name || item.accountId}: {item.message || item.outcome}
        </p>)}
      </Card>
    </div>
  )
}
