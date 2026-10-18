import { useState } from 'react'
import { PageIntro, IconButton, Badge, Empty, ErrorNote, SkeletonList } from '../components/ui'
import { ProviderLogo, GlyphLogo } from '../components/ProviderLogo'
import { Icon } from '../components/Icon'
import { store } from '../state/store'
import { fetchCheckinCenter, runCheckin, type CheckinCenter } from '../api/gateway'
import { formatRelative, formatStamp, isToday } from '../lib/format'
import { providerLabel } from '../lib/providers'
import { usePolling } from '../lib/usePolling'
import { t } from '../i18n'

/**
 * 签到中心。
 *
 * 一屏完成「看进度 + 一键签 + 看谁没签」。桌面端这三件事分在概览卡与
 * 展开表里；手机上把进度条与分组列表直接摞在一起。
 *
 * 分组口径与后端完全一致（`/api/checkin-center` 的 `daily.providers`）：
 * 能进这个列表的账号 = 提供商支持签到，与「立即全部签到」实际签到的集合
 * 不会分叉。`outOfScope` 单独列在底部，说明哪些账号不参与、为什么。
 */
export function CheckinPage() {
  const [data, setData] = useState<CheckinCenter | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState('')
  const [expanded, setExpanded] = useState<string | null>(null)

  usePolling(async () => {
    try {
      const next = await fetchCheckinCenter()
      setData(next)
      setError('')
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
    }
  }, 60_000)

  const daily = data?.daily
  const providers = daily?.providers || []
  const eligible = daily?.todayEligible || 0
  const done = daily?.todayDone || 0
  const percent = eligible ? Math.round((done / eligible) * 100) : 0

  const sign = async (id?: string, label?: string) => {
    setBusy(id || 'all')
    try {
      await runCheckin(id)
      store.toast(label ? t('已提交「{label}」的签到', { label }) : t('已开始批量签到'))
      const next = await fetchCheckinCenter()
      setData(next)
    } catch (failure) {
      store.toast(failure instanceof Error ? failure.message : String(failure), 'err')
    } finally {
      setBusy('')
    }
  }

  return (
    <div className='m-page'>
      <PageIntro subtitle={t('今天 · 北京时间自然日')}
        actions={
          <IconButton
            name='refresh'
            label={t('刷新')}
            onClick={() => {
              void fetchCheckinCenter().then(setData).catch(() => undefined)
            }} />
        }
      />

      {error ? <ErrorNote text={error} /> : null}
      {!data ? <SkeletonList rows={3} height={80} /> : null}

      {data ? (
        <>
          <div className='m-section' style={{ marginTop: 0 }}>
            <div className='m-card m-card-pad' style={{ display: 'flex', gap: 15, alignItems: 'center' }}>
              <div style={{ flex: 'none' }}>
                <div style={{ fontSize: 30, fontWeight: 650, letterSpacing: '-0.035em', fontVariantNumeric: 'tabular-nums' }}>
                  {done}
                  <span style={{ fontSize: 15, color: 'var(--m-text-3)', fontWeight: 500 }}> / {eligible}</span>
                </div>
                <div style={{ fontSize: 12, color: 'var(--m-text-3)', marginTop: 2 }}>{t('今日已签')}</div>
              </div>
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ height: 7, borderRadius: 4, background: 'var(--m-surface-3)', overflow: 'hidden' }}>
                  <i
                    style={{
                      display: 'block',
                      height: '100%',
                      width: `${percent}%`,
                      borderRadius: 4,
                      background: 'linear-gradient(90deg, var(--m-primary), oklch(0.62 0.19 250))',
                    }}
                  />
                </div>
                <div style={{ display: 'flex', justifyContent: 'space-between', marginTop: 6, fontSize: 11, color: 'var(--m-text-3)' }}>
                  <span>{percent}%</span>
                  <span>
                    {data.daily?.outOfScope?.reduce((sum, item) => sum + (item.count || 0), 0) || 0} {t('个账号不参与')}
                  </span>
                </div>
              </div>
            </div>
          </div>

          <div className='m-section'>
            <div className='m-card' style={{ padding: '12px 15px', display: 'flex', gap: 10 }}>
              <div style={{ flex: 1 }}>
                <div style={{ fontSize: 11.5, color: 'var(--m-text-3)' }}>{t('定时签到')}</div>
                <div style={{ fontSize: 14, fontWeight: 600, marginTop: 3 }}>
                  {data.auto?.enabled ? data.auto?.time || t('已开启') : t('未开启')}
                </div>
                <div style={{ fontSize: 11, color: 'var(--m-text-3)', marginTop: 2 }}>
                  {data.auto?.enabled ? t('每天自动执行') : t('在桌面端设置')}
                </div>
              </div>
              <div style={{ width: 1, background: 'var(--m-line)' }} />
              <div style={{ flex: 1 }}>
                <div style={{ fontSize: 11.5, color: 'var(--m-text-3)' }}>{t('签到记录')}</div>
                <div style={{ fontSize: 14, fontWeight: 600, marginTop: 3 }}>
                  {t('{n} 条', { n: data.history?.length || 0 })}
                </div>
                <div style={{ fontSize: 11, color: 'var(--m-text-3)', marginTop: 2 }}>
                  {data.history?.[0]?.at ? formatRelative(data.history[0].at) : t('还没有记录')}
                </div>
              </div>
            </div>
          </div>

          <div className='m-section'>
            <div className='m-section-head'>
              <h2>{t('每日签到')}</h2>
              <button
                type='button'
                className='m-link'
                disabled={busy === 'all'}
                onClick={() => void sign()}
              >
                {busy === 'all' ? t('签到中…') : t('立即全部签到')}
              </button>
            </div>
            <div className='m-card'>
              {providers.length ? (
                providers.map(group => {
                  const key = String(group.id || '')
                  const total = group.totalCount || 0
                  const groupDone = group.doneCount || 0
                  const tone = total === 0 ? 'mute' : groupDone >= total ? 'ok' : groupDone > 0 ? 'warn' : 'brand'
                  const toneText =
                    total === 0 ? t('无账号') : groupDone >= total ? t('已签') : groupDone > 0 ? t('部分') : t('待签')
                  const isOpen = expanded === key
                  return (
                    <div key={key} style={{ borderBottom: '1px solid var(--m-line)' }}>
                      <button
                        type='button'
                        onClick={() => setExpanded(isOpen ? null : key)}
                        style={{
                          display: 'flex',
                          alignItems: 'center',
                          gap: 11,
                          width: '100%',
                          padding: '12px 14px',
                          background: 'none',
                          border: 0,
                          textAlign: 'left',
                        }}
                      >
                        <ProviderLogo id={key} label={group.label} />
                        <div style={{ flex: 1, minWidth: 0 }}>
                          <div style={{ fontSize: 13.5, fontWeight: 550 }}>{providerLabel(key, group.label)}</div>
                          <div style={{ fontSize: 11, color: 'var(--m-text-3)', marginTop: 3 }}>
                            {t('{done}/{total} 已签', { done: groupDone, total })}
                          </div>
                        </div>
                        <Badge tone={tone}>{toneText}</Badge>
                        <Icon
                          name='chev'
                          style={{
                            width: 16,
                            height: 16,
                            color: 'var(--m-text-3)',
                            flex: 'none',
                            transform: isOpen ? 'rotate(90deg)' : 'none',
                            transition: 'transform .16s',
                          }}
                        />
                      </button>
                      {isOpen ? (
                        <div style={{ padding: '0 14px 12px' }}>
                          {(group.accounts || []).map(account => (
                            <div
                              key={account.id}
                              style={{
                                display: 'flex',
                                alignItems: 'center',
                                gap: 8,
                                padding: '9px 0',
                                borderTop: '1px solid var(--m-line)',
                                fontSize: 12.5,
                              }}
                            >
                              <span
                                style={{
                                  flex: 1,
                                  minWidth: 0,
                                  overflow: 'hidden',
                                  textOverflow: 'ellipsis',
                                  whiteSpace: 'nowrap',
                                  color: account.available === false ? 'var(--m-text-3)' : 'var(--m-text)',
                                }}
                              >
                                {account.name || account.id}
                              </span>
                              <span style={{ fontSize: 11, color: 'var(--m-text-3)', flex: 'none' }}>
                                {account.checkinAt ? formatStamp(account.checkinAt) : '—'}
                              </span>
                              <button
                                type='button'
                                className='m-btn m-sm m-ghost'
                                disabled={busy === account.id || account.available === false}
                                onClick={() => void sign(account.id, account.name || account.id)}
                                style={{ height: 28, fontSize: 11.5, padding: '0 10px' }}
                              >
                                {busy === account.id
                                  ? t('签到中…')
                                  : isToday(account.checkinAt)
                                    ? t('重签')
                                    : t('签到')}
                              </button>
                            </div>
                          ))}
                        </div>
                      ) : null}
                    </div>
                  )
                })
              ) : (
                <Empty text={t('没有可签到的账号')} />
              )}
            </div>
          </div>

          {(data.daily?.outOfScope || []).length ? (
            <div className='m-section'>
              <div className='m-section-head'>
                <h2>{t('不参与签到')}</h2>
              </div>
              <div className='m-card'>
                {(data.daily?.outOfScope || []).map(item => (
                  <div key={item.label} className='m-row'>
                    <GlyphLogo name='info' />
                    <div className='m-grow'>
                      <div className='m-t1'>{item.label}</div>
                      <div className='m-t2'>{item.reason}</div>
                    </div>
                    <span className='m-val'>{t('{n} 个', { n: item.count || 0 })}</span>
                  </div>
                ))}
              </div>
            </div>
          ) : null}

          {(data.extras?.onboarding || []).length || (data.extras?.plans || []).length ? (
            <div className='m-section'>
              <div className='m-section-head'>
                <h2>{t('新手福利与活动')}</h2>
              </div>
              <div className='m-card'>
                {(data.extras?.onboarding || []).map(item => (
                  <div key={item.id} className='m-row'>
                    <ProviderLogo id={item.provider} />
                    <div className='m-grow'>
                      <div className='m-t1'>{item.name || item.id}</div>
                      <div className='m-t2'>{t('新手任务')}</div>
                    </div>
                    <Badge tone='brand'>{t('去领取')}</Badge>
                  </div>
                ))}
                {(data.extras?.plans || []).map(item => (
                  <div key={item.id} className='m-row'>
                    <ProviderLogo id={item.provider} />
                    <div className='m-grow'>
                      <div className='m-t1'>{item.name || item.id}</div>
                      <div className='m-t2'>{t('套餐领取')}</div>
                    </div>
                    <Badge tone={item.claimAt ? 'mute' : 'brand'}>
                      {item.claimAt ? t('已领') : t('可领')}
                    </Badge>
                  </div>
                ))}
              </div>
              <div style={{ marginTop: 8, fontSize: 11.5, color: 'var(--m-text-3)', padding: '0 2px' }}>
                {t('福利与套餐的逐项领取在桌面端签到中心完成')}
              </div>
            </div>
          ) : null}
        </>
      ) : null}
    </div>
  )
}
