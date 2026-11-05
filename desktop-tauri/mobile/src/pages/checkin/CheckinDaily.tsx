import { useState } from 'react'
import { Button, Card, SegmentedControl } from '@ui'
import { Badge, Empty, SectionHead } from '../../components/ui'
import { ProviderLogo } from '../../components/ProviderLogo'
import { store } from '../../state/store'
import { CHECKIN_SECTION_OPTIONS, modesOf, runActivity, runCheckin, type CheckinCenter, type CheckinSection } from '../../api/checkin'
import { activitySummary, checkinModeLabel, checkinSummary } from '../../lib/checkin'
import { formatStamp } from '../../lib/format'
import type { CheckinAction } from '../CheckinPage'
import { CheckinBenefits } from './CheckinBenefits'
import { CheckinUmid } from './CheckinUmid'
import { CheckinPlansSheet } from './CheckinPlansSheet'
import { t } from '../../i18n'
import {
  accountCheckinStatus, checkinActionLabel, checkinPlanProgress, checkinStatusLabel, checkinStatusTone,
} from '../../../../ui-shared/checkin-result'

export function CheckinDaily({ center, busy, action, onRecords, onChange }: {
  center: CheckinCenter
  busy: string
  action: CheckinAction
  onRecords: () => void
  onChange: () => void
}) {
  const [section, setSection] = useState<CheckinSection>('daily')
  const [planId, setPlanId] = useState('')
  const plan = center.extras?.plans?.find(entry => entry.id === planId)
  const sections = CHECKIN_SECTION_OPTIONS.map(option => ({ ...option, label: t(option.label) }))
  const daily = center.daily
  const groups = daily?.providers || []
  const hasCapability = Object.keys(center.checkinFeatures || {}).length > 0 || groups.length > 0
  const hasAccounts = groups.some(group => group.accounts?.length > 0)

  const sign = async (id?: string, mode = 'checkin') => {
    if (mode === 'checkin') {
      const result = await action(id || 'all', () => runCheckin(id))
      if (result) {
        const summary = checkinSummary(result, Boolean(id))
        store.toast(summary.text, summary.tone)
      }
      return
    }
    const result = await action(id || 'all', () => runActivity(id || '', mode))
    if (result) {
      const summary = activitySummary(result)
      store.toast(summary.text, summary.tone)
    }
  }

  return (
    <>
      <div className='m-section' style={{ marginTop: 0 }}>
        <Card className='m-card m-card-pad'>
          <div className='m-ck-toolbar'>
            <div>
              <div className='m-ck-title'>{t('今日签到 {done} / {total}', { done: daily?.todayDone || 0, total: daily?.todayEligible || 0 })}</div>
              <div className='m-ck-note'>{center.auto?.running ? t('自动签到执行中…')
                : center.auto?.enabled ? t('自动签到已开启') : t('自动签到已关闭')}</div>
            </div>
            <Button size='sm' disabled={Boolean(busy) || center.auto?.running || !hasAccounts} onClick={() => void sign()}>
              {busy === 'all' ? t('签到中…') : t('全部签到')}
            </Button>
          </div>
          <Button size='sm' variant='ghost' onClick={onRecords}>{t('查看签到记录')}</Button>
        </Card>
      </div>

      <div className='m-section'>
        <SegmentedControl options={sections} value={section} onValueChange={setSection}
          aria-label={t('签到功能分类')} className='m-checkin-tabs flex-wrap' />
      </div>

      {section === 'daily' && (!hasCapability || !hasAccounts) ? (
        <div className='m-section'>
          <Card className='m-card m-card-pad'>
            <p className='m-ck-note'>{!hasCapability
              ? t('还没有可签到的提供商。请安装或启用所连接网关的提供商插件。')
              : t('还没有可签到的账号。账号与插件均保存在所连接的网关上。')}</p>
            <Button size='sm' variant='outline' onClick={() => store.push(!hasCapability ? 'plugins' : 'accounts')}>
              {!hasCapability ? t('查看插件状态') : t('查看账号')}
            </Button>
          </Card>
        </div>
      ) : null}

      {section === 'daily' ? groups.filter(group => group.accounts?.length > 0).map(group => (
        <div className='m-section' key={group.id}>
          <SectionHead title={t(group.label)} />
          <Card className='m-card'>
            {group.description ? <p className='m-ck-note m-card-pad'>{t(group.description)}</p> : null}
            {group.uiHooks?.includes('umid_component') ? <CheckinUmid /> : null}
            {group.accounts.map(account => (
              <div className='m-card-pad m-ck-account' key={account.id}>
                <div className='m-ck-toolbar'>
                  <ProviderLogo id={group.id} label={group.label} />
                  <div className='m-ck-grow'>
                    <div className='m-ck-title'>{account.name || account.id}</div>
                    <div className='m-ck-note'>{account.checkinAt ? formatStamp(account.checkinAt) : t('还没签到过')}
                      {account.streakDays > 0 ? t(' · 连登 {n} 天', { n: account.streakDays }) : ''}</div>
                  </div>
                  <Badge tone={!account.available ? 'bad' : checkinStatusTone(accountCheckinStatus(account))}>
                    {!account.available ? t('凭证不可用') : t(checkinStatusLabel(accountCheckinStatus(account)))}
                  </Badge>
                  {checkinPlanProgress(account.dailyCheckin) && account.dailyCheckin ? <span className='m-ck-note'>
                    {t('已领 {done} / {total} 份套餐', { done: account.dailyCheckin.completedCount, total: account.dailyCheckin.totalCount })}
                  </span> : null}
                </div>
                {account.dailyCheckin?.message && !account.checkedInToday
                  ? <p className='m-ck-note'>{t(account.dailyCheckin.message)}</p> : null}
                <div className='m-ck-actions'>
                  {modesOf(group).map(mode => (
                    <Button key={mode} variant='outline' size='sm'
                      disabled={Boolean(busy) || center.auto?.running || !account.available || !['checkin', 'keepalive', 'claim', 'full'].includes(mode)}
                      onClick={() => {
                        if (group.manualPlans && mode === 'checkin') setPlanId(account.id)
                        else void sign(account.id, mode)
                      }}>
                      {group.manualPlans && mode === 'checkin' ? t('套餐明细')
                        : busy === account.id ? t(account.dailyCheckin?.queryOnly ? '查询中…' : '执行中…')
                          : mode === 'checkin' ? t(checkinActionLabel(account)) : checkinModeLabel(mode)}
                    </Button>
                  ))}
                </div>
              </div>
            ))}
          </Card>
        </div>
      )) : null}

      {section === 'daily' && daily?.outOfScope?.length ? (
        <div className='m-section'>
          <SectionHead title={t('不参与每日签到')} />
          <Card className='m-card'>
            {daily.outOfScope.map(row => <div className='m-card-pad m-ck-account' key={`${row.label}:${row.reason}`}>
              <div className='m-ck-title'>{t(row.label)} ×{row.count}</div>
              <div className='m-ck-note'>{t(row.reason)}</div>
            </div>)}
          </Card>
        </div>
      ) : null}

      <CheckinBenefits center={center} section={section} busy={busy} action={action} onChange={onChange} />
      {plan ? <CheckinPlansSheet key={plan.id} entry={plan} onClose={() => setPlanId('')} onChange={onChange} /> : null}
      {section === 'daily' && !groups.length && !center.extras?.onboarding?.length && !center.extras?.plans?.length && !center.extras?.growth?.length
        ? <Empty text={t('安装插件并添加账号后，支持的签到与福利会自动显示')} /> : null}
    </>
  )
}
