import * as React from 'react'
import { Card, CardContent, CardFooter, CardHeader, CardTitle } from '@ui'
import { ErrorNumber, ErrorToggle } from './routing-errors-controls'
import type { ErrorSettings, ErrorTranslate } from './routing-errors-types'

export function ErrorProtection({ settings, t, onChange }: { settings: ErrorSettings; t: ErrorTranslate; onChange(settings: ErrorSettings): void }) {
  const patch = (next: Partial<ErrorSettings>) => onChange({ ...settings, ...next })
  return <>
    <Card><CardHeader><CardTitle>{t('失败退避')}</CardTitle></CardHeader><CardContent className='re-protection'>
      <ErrorToggle label={t('失败后暂时休息')} description={t('只统计连接和服务异常，不惩罚请求参数、内容拒绝或能力缺失。')} checked={settings.backoff} onChange={backoff => patch({ backoff })} />
      <div className='re-form-grid'><ErrorNumber label={t('首次休息时间')} unit={t('秒')} value={settings.backoffSeconds} min={1} max={3600} disabled={!settings.backoff} onChange={backoffSeconds => patch({ backoffSeconds, backoffCapSeconds: Math.max(settings.backoffCapSeconds, backoffSeconds) })} />
        <ErrorNumber label={t('连续失败最长休息')} unit={t('秒')} value={settings.backoffCapSeconds} min={settings.backoffSeconds} max={3600} disabled={!settings.backoff} onChange={backoffCapSeconds => patch({ backoffCapSeconds })} /></div>
    </CardContent><CardFooter>{t('连续失败逐次延长；成功后清零。')}</CardFooter></Card>
    <Card><CardHeader><CardTitle>{t('限流后降低优先级')}</CardTitle></CardHeader><CardContent className='re-protection'>
      <ErrorToggle label={t('限流后暂时排到后面')} description={t('只影响选路顺序，不延长上游明确的冷却时间。')} checked={settings.sink} onChange={sink => patch({ sink })} />
      <ErrorNumber label={t('最长保留时间')} unit={t('秒')} value={settings.sinkSeconds} min={30} max={3600} disabled={!settings.sink} onChange={sinkSeconds => patch({ sinkSeconds })} />
    </CardContent><CardFooter>{t('到点自动恢复原顺序，成功一次可提前恢复。')}</CardFooter></Card>
    <Card><CardHeader><CardTitle>{t('连续失败保护')}</CardTitle></CardHeader><CardContent className='re-protection'>
      <ErrorToggle label={t('启用连续失败保护')} description={t('只统计连接和服务异常；生效时间取较晚的一项，不叠加等待。')} checked={settings.breaker} onChange={breaker => patch({ breaker })} />
      <div className='re-form-grid re-form-three'><ErrorNumber label={t('连续失败阈值')} unit={t('次')} value={settings.breakerFailures} min={1} max={20} disabled={!settings.breaker} onChange={breakerFailures => patch({ breakerFailures })} />
        <ErrorNumber label={t('保护时长')} unit={t('秒')} value={settings.breakerSeconds} min={10} max={600} disabled={!settings.breaker} onChange={breakerSeconds => patch({ breakerSeconds })} />
        <ErrorNumber label={t('恢复所需成功次数')} unit={t('次')} value={settings.breakerSuccesses} min={1} max={10} disabled={!settings.breaker} onChange={breakerSuccesses => patch({ breakerSuccesses })} /></div>
    </CardContent></Card>
  </>
}

export function ErrorBoundaries({ t }: { t: ErrorTranslate }) {
  return <Card><CardHeader><CardTitle>{t('始终保留的保护')}</CardTitle></CardHeader><CardContent className='re-boundaries'>
    <div><strong>{t('已经开始输出')}</strong><p>{t('不从头重发，不拼接另一个账号的回答。')}</p></div>
    <div><strong>{t('任务或计费结果不确定')}</strong><p>{t('没有可靠的安全重放依据时，不重复提交。')}</p></div>
    <div><strong>{t('验证、风控与明确拒绝')}</strong><p>{t('不通过换号绕过；保留原因，返回客户端。')}</p></div>
    <div><strong>{t('既有权限与路由范围')}</strong><p>{t('不扩大网关密钥授权，不擅自改指定账号，不新增付费操作。')}</p></div>
  </CardContent></Card>
}
