import type { ResourceCapabilities } from '../../../ui-shared/agent-resources'

export type AgentModel = {
  id: string
  name: string
  contextWindow?: number | null
  maxOutputTokens?: number | null
  supportsTools?: boolean | null
  supportsImages?: boolean | null
  supportsVideo?: boolean | null
  supportsReasoning?: boolean | null
  reasoningLevels: string[]
  effort: string
}

export type AgentProfile = {
  id: string
  name: string
  kind: string
  protocol: string
  baseUrl: string
  model: string
  provider: string
  effort: string
  enabled: boolean
  default: boolean
  note: string
  models: AgentModel[]
  claudeModels: { opus: string; sonnet: string; haiku: string; subagent: string }
  authMode: string
  apiKey: string
  envKey: string
  gatewayKeyId: string
  hasApiKey?: boolean
  schemaVersion?: number
  sourceId?: string
  inFile?: boolean
  validationError?: string | null
  editable?: boolean
  deletable?: boolean
  updatedAt?: number
  liveId?: string | null
  savedIds?: string[]
  status?: 'active' | 'enabled' | 'disabled' | 'available' | 'inactive'
}

export type AgentClient = {
  resources?: ResourceCapabilities
  id: string
  name: string
  vendor: string
  hue: number
  letter: string
  icon: string
  iconFull: boolean
  configPath: string
  resolvedPath: string
  pathOverride: string
  configTargets?: Array<{ path: string; label: string; profile: string; version: string; layout: 'legacy' | 'profile' }>
  configLayout?: 'legacy' | 'profile' | ''
  format: string
  configMode: 'single' | 'providers' | 'models' | 'readonly'
  protocol: string
  protocols: string[]
  installed: boolean
  detected: boolean
  detectionState: 'missing' | 'uninitialized' | 'read_error' | 'unsupported' | 'ready'
  evidence: string[]
  version: string
  writable: boolean
  supportsModels: boolean
  modelFields?: Array<keyof AgentModel>
  supportsEffort: boolean
  supportsDefault: boolean
  supportsToggle: boolean
  supportsGateway: boolean
  supportsEnvKey: boolean
  supportNote: string
  docsUrl: string
  needsRestart: boolean
  configurations: AgentProfile[]
  profiles: AgentProfile[]
  quickProfiles?: AgentProfile[]
  live: AgentProfile[]
  readError: string | null
  storeError: string | null
  viaGateway: boolean
  activeId: string | null
  revision: string
  nativeRevision?: string | null
  applied: { canUndo?: boolean; canDisconnect?: boolean; modifiedExternally?: boolean; pendingRecovery?: boolean; appliedAt?: number; backupId?: string; error?: string }
}

export type AgentPreview = {
  token: string
  hasChanges: boolean
  profileId: string
  warnings: string[]
  files: Array<{
    path: string
    before: string
    text: string
    changed: boolean
    created: boolean
    deleted: boolean
    diff: Array<{ kind: 'same' | 'added' | 'removed'; text: string; oldLine?: number; newLine?: number }>
  }>
}

export type AgentGatewayKey = { id: string; name: string; allowedModels: string[]; allowedProviders?: string[] }
export type AgentPayload = {
  clients: AgentClient[]
  gateway: { baseUrl: string; requiresKey: boolean; keys: AgentGatewayKey[]; defaultModel?: string }
  detectedAt: number
  host: { name: string }
  applied?: { changed?: boolean; verified?: boolean; needsRestart?: boolean }
  imported?: { count: number }
}
export type AgentOperation = 'apply' | 'default' | 'restore' | 'disconnect' | 'native-edit' | 'native-delete' | 'quick-connect'
  | 'config-use' | 'config-disable' | 'config-edit' | 'config-delete'
