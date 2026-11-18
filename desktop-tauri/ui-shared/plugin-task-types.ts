export type PluginTaskCategory = 'checkin' | 'growth' | 'rewards' | 'maintenance' | 'other'

export type PluginTaskRun = {
  account: string
  name?: string | null
  running: boolean
  interrupted?: boolean
  lastRunAt: number
  nextRunAt: number
  error?: string | null
  result?: string | null
}

export type PluginTaskRow = {
  provider: string
  providerLabel?: string
  category: PluginTaskCategory
  task: {
    id: string
    label: string
    enabled: boolean
    times: string[]
    interval_ms?: number | null
    utc_offset_minutes: number
    legacy_config?: string | null
  }
  runs: PluginTaskRun[]
  unavailable?: string | null
  operation?: { active: boolean; at?: number; manual?: boolean; skipped?: boolean; interrupted?: boolean; error?: string | null }
}

export type PluginTaskRequest = <T>(method: 'GET' | 'POST' | 'PATCH', path: string, body?: unknown) => Promise<T>

export function isCheckinTask(row: PluginTaskRow): boolean {
  return row.category === 'checkin' || row.category === 'growth' || row.category === 'rewards'
}

export function pluginTaskRunning(row: PluginTaskRow): boolean {
  return row.operation?.active === true || row.runs.some(run => run.running)
}
