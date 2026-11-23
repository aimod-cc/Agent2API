import type { ErrorPolicyRecord, ErrorTranslate } from './routing-errors-types'
import { actionLabels, kindLabels, scopeLabels } from './routing-errors-types'

export function errorPolicyDescription(record: ErrorPolicyRecord | null | undefined, t: ErrorTranslate): string {
  if (!record?.kind || !record.action) return ''
  const reasons: Record<string, string> = {
    '该账号需要重新登录': t('该账号需要重新登录'),
    '当前账号已关闭失败换号': t('当前账号已关闭失败换号'),
    '本次请求明确指定账号，未更换指定对象': t('本次请求明确指定账号，未更换指定对象'),
    '当前路由没有其他可用对象': t('当前路由没有其他可用对象'),
    '换账号次数已用尽，未继续发送': t('换账号次数已用尽，未继续发送'),
    '重发次数或累计等待已用尽，不缩短上游恢复时间': t('重发次数或累计等待已用尽，不缩短上游恢复时间'),
    '响应已开始，未重放；只记录原账号的恢复时间': t('响应已开始，未重放；只记录原账号的恢复时间'),
  }
  const name = record.ruleName && record.ruleId !== record.kind ? record.ruleName : kindLabels(t)[record.kind]
  const parts = [name, record.reason ? reasons[record.reason] || t(record.reason) : actionLabels(t)[record.action]]
  if (record.resetAt && record.timeSource !== 'none') {
    parts.push(record.timeSource === 'sink'
      ? t('优先级已调至最低')
      : record.timeSource === 'upstream' ? t('冷却来自上游时间') : t('冷却来自本机估算'))
    if (record.scope) parts.push(scopeLabels(t)[record.scope])
    parts.push(t('{time} 恢复', { time: new Date(record.resetAt).toLocaleString() }))
  }
  return parts.filter(Boolean).join(' · ')
}
