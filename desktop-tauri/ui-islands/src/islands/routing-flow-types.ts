export type FlowQuery = {
  model?: string
  minutes: number
  scope: 'window' | 'running'
  search?: string
  sessionSearch?: string
  path?: string
  session?: string
  client?: string
  provider?: string
  account?: string
  activity?: 'all' | 'running' | 'idle'
  detail?: 'sessions' | 'requests' | 'models'
  offset?: number
  limit?: number
}
export type FlowPath = {
  id: string
  client: string
  clientLabel?: string
  model: string
  provider: string
  accountId: string
  accountName: string
  requests: number
  running: number
  successful: number
  failed: number
  sessions: number
  stickyHits: number
  switched: number
  lastSeen: number
}
export type FlowSnapshot = {
  available: boolean
  paths: FlowPath[]
  pathCount: number
  truncated: boolean
  requests: number
  running: number
  sessions: number
  unknownRequests: number
  now: number
  since: number
}
export type FlowSession = {
  id: string
  model: string
  requestCount: number
  running: number
  lastSeen: number
  latest: import('./routing-types').ReplayRow
}
export type FlowDetails = {
  available: boolean
  entries: Array<FlowSession | import('./routing-types').ReplayRow>
  matched: number
  unknownRequests?: number
  now: number
}
/** `detail=models`：当前时间窗内真的发过请求的模型（筛选下拉用）。 */
export type FlowModels = { available: boolean; models: string[]; now: number }
export type FlowSelection =
  | { kind: 'client' | 'model' | 'provider' | 'account'; id: string }
  | { kind: 'path'; id: string }
  | { kind: 'edge'; model: string; client?: string; accountId?: string; provider?: string }
  | { kind: 'request'; row: import('./routing-types').ReplayRow }
  | { kind: 'session'; session: FlowSession }
  | null
export type ConfigTarget = { kind: 'model' | 'provider' | 'account'; id: string }
