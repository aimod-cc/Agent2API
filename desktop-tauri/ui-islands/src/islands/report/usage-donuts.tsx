import * as React from 'react'
import {
  Button, Card, CardContent, CardHeader, CardTitle, Empty, Label,
  Popover, PopoverContent, PopoverTrigger, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, Switch,
} from '@ui'
import { DEFAULT_DONUT_SETTINGS, DONUT_THRESHOLDS, donutView, formatInt, type DonutSettings, type DonutView, type StatsGroup } from '../report-charts'
import { UsageDonut } from './report-recharts'
import { t } from '../../i18n'

const SETTINGS_KEY = 'workbuddy-desktop-report-donut-settings'
const TIP_GROUPING = t('合并只作用于饼图。低于阈值的扇区归入“其他”，右侧列表完整保留每个项目，总量和占比不变。')
const TIP_MODELS = t('按 Token 用量从多到少排列，扇区角度表示占比。占比不低于 3% 的扇区显示百分比，所有项目的完整读数保留在右侧列表与悬停提示中。')
const TIP_PROVIDERS = t('所选范围内每个提供商各消耗了多少 Token，按用量从多到少排，扇区角度即占比。同一个模型名可能有多家都能提供，网关按设置页「转发路由」的优先级依次尝试，所以占比也反映了实际落到谁身上。中心是这一维的合计（等于概览里的总 Token），右侧每行给出该提供商的 Token 用量与百分比。「未知」是指那批请求没记下承载它的提供商 —— 主要是记录提供商这个功能上线之前的旧明细，其次是请求在选定上游之前就失败了。')

function readSettings(): DonutSettings {
  try {
    const saved: unknown = JSON.parse(localStorage.getItem(SETTINGS_KEY) || 'null')
    if (!saved || typeof saved !== 'object') return DEFAULT_DONUT_SETTINGS
    const record = saved as Record<string, unknown>
    return {
      enabled: typeof record.enabled === 'boolean' ? record.enabled : DEFAULT_DONUT_SETTINGS.enabled,
      threshold: DONUT_THRESHOLDS.some(value => value === record.threshold) ? record.threshold as number : DEFAULT_DONUT_SETTINGS.threshold,
    }
  } catch { return DEFAULT_DONUT_SETTINGS }
}

function DonutPanel({ panelId, listId, title, tip, view, ariaLabel, itemLabel, countText, emptyText }: {
  panelId: string; listId: string; title: string; tip: string; ariaLabel: string; itemLabel: string
  view: DonutView | null; countText: string; emptyText: string
}) {
  return <Card render={<section />} className='report-card report-donut-card' id={panelId}>
    <CardHeader className='report-card-header'>
      <CardTitle>{title}</CardTitle><span className='tip-q' data-tip={`${tip}\n\n${TIP_GROUPING}`} />
      <span className='report-card-meta ml-auto'>{countText}</span>
    </CardHeader>
    <CardContent className='report-donut-content'><div id={listId}>
      {view ? <UsageDonut view={view} ariaLabel={ariaLabel} itemLabel={itemLabel} /> : <Empty>{emptyText}</Empty>}
    </div></CardContent>
  </Card>
}

export function ReportUsageDonuts({ models, providers }: { models: StatsGroup[] | null; providers: StatsGroup[] | null }) {
  const [settings, setSettings] = React.useState<DonutSettings>(readSettings)
  const switchId = React.useId()
  const thresholdId = React.useId()
  const changeSettings = (patch: Partial<DonutSettings>) => {
    setSettings(previous => {
      const next = { ...previous, ...patch }
      try { localStorage.setItem(SETTINGS_KEY, JSON.stringify(next)) } catch { /* 本次会话照常生效。 */ }
      return next
    })
  }
  const modelView = donutView(models || undefined, t('未知模型'), settings)
  const providerView = donutView(providers || undefined, t('未知'), settings)
  if (!models && !providers) return null
  return <section className='report-donut-section' aria-label={t('用量分布')}>
    <div className='report-section-heading'>
      <div><h2>{t('用量分布')}</h2><span>{t('按 Token 统计')}</span></div>
      <Popover><PopoverTrigger render={<Button variant='ghost' size='sm' className='report-donut-settings-button' />}>
        <svg width='14' height='14' viewBox='0 0 16 16' fill='none' aria-hidden='true'><path d='M2 4h12M2 12h12M6 2v4M10 10v4' stroke='currentColor' strokeWidth='1.5' strokeLinecap='round' /></svg>
        {settings.enabled ? t('饼图小于 {threshold}% 归入其他', { threshold: settings.threshold }) : t('饼图不合并小项')}
      </PopoverTrigger><PopoverContent align='end' className='report-donut-settings'>
        <div className='report-setting-title'>{t('饼图显示')}</div>
        <p>{t('仅调整两个饼图，右侧列表始终完整显示')}</p>
        <div className='report-setting-row'><Label htmlFor={switchId}>{t('合并小项')}</Label>
          <Switch id={switchId} checked={settings.enabled} onCheckedChange={enabled => changeSettings({ enabled })} /></div>
        <div className='report-setting-row'><Label htmlFor={thresholdId}>{t('占比低于')}</Label>
          <Select value={String(settings.threshold)} disabled={!settings.enabled} onValueChange={value => {
            const threshold = Number(value)
            if (DONUT_THRESHOLDS.some(option => option === threshold)) changeSettings({ threshold })
          }}>
            <SelectTrigger id={thresholdId} className='w-[96px]' aria-label={t('小项合并阈值')}><SelectValue>{`${settings.threshold}%`}</SelectValue></SelectTrigger>
            <SelectContent>{DONUT_THRESHOLDS.map(value => <SelectItem key={value} value={String(value)}>{`${value}%`}</SelectItem>)}</SelectContent>
          </Select>
        </div>
      </PopoverContent></Popover>
    </div>
    <div className='report-donut-grid'>
      {models ? <DonutPanel panelId='report-models-panel' listId='report-models-donut'
        title={t('模型用量')} tip={TIP_MODELS} ariaLabel={t('模型用量占比')} itemLabel={t('模型')} view={modelView}
        countText={t('{n} 个模型', { n: formatInt(modelView?.count || 0) })} emptyText={t('所选范围内还没有模型用量')} /> : null}
      {providers ? <DonutPanel panelId='report-providers-pie-panel' listId='report-providers-donut'
        title={t('提供商用量')} tip={TIP_PROVIDERS} ariaLabel={t('提供商用量占比')} itemLabel={t('提供商')} view={providerView}
        countText={t('{n} 家提供商', { n: formatInt(providerView?.count || 0) })} emptyText={t('所选范围内还没有提供商用量')} /> : null}
    </div>
    <p className='report-distribution-note'>{TIP_GROUPING}</p>
  </section>
}
