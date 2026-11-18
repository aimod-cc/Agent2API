import * as React from 'react'
import { PluginTaskSettings } from '../../../ui-shared/plugin-task-settings'
import { isCheckinTask } from '../../../ui-shared/plugin-task-types'
import type { CheckinCenter } from './checkin-types'
import { api } from './plugins-shared'
import { loadCenter, store } from './checkin-store'
import { t } from '../i18n'

export function CheckinPluginTasks({ center, provider = '', categories, title, description, expanded = false }: {
  center: CheckinCenter
  provider?: string
  categories?: string[]
  title?: string
  description?: string
  expanded?: boolean
}) {
  const rows = (center.pluginTasks ?? []).filter(row => isCheckinTask(row)
    && (!provider || row.provider === provider) && (!categories || categories.includes(row.category)))
  return <PluginTaskSettings rows={rows} request={api} refresh={loadCenter} t={t} disabled={store.saving}
    expanded={expanded} title={title || t('签到与成长自动任务')}
    description={description || t('插件提供任务能力，主程序统一管理开关、执行时间和结果。与每日签到计划分别设置，不会因打开页面自动执行。')} />
}
