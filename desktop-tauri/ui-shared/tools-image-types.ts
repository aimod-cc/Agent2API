export type ImageTarget = { provider: string; model: string; account_id: string }
export type ImageGenerationConfig = {
  generation: ImageTarget
  editing: ImageTarget | null
  max_images: number
  max_concurrency: number
  retention_hours: number
  storage_mb: number
}
export type ImageModelOption = {
  id: string; provider: string; provider_label: string; generation: boolean; editing: boolean
}
export type ImageAccountOption = { id: string; name: string; provider: string; enabled: boolean }
export type ImageConfigView = {
  config: ImageGenerationConfig | null
  defaults: ImageGenerationConfig
  models: ImageModelOption[]
  accounts: ImageAccountOption[]
  generation_error: string | null
  editing_error: string | null
  timeout_seconds: number
}
export type ToolImageFile = {
  id: string; mime: string; width: number; height: number; bytes: number; url: string; ref: string
}
export type ToolImageJob = {
  job_id: string; request_id: string; status: 'running' | 'succeeded' | 'failed' | 'uncertain' | 'expired'
  operation: 'generate' | 'edit'; model: string; provider: string; created_at: number; expires_at: number
  error?: string | null; images?: ToolImageFile[]; points_consumed?: number; usage?: unknown
  actual_account_name?: string; actual_model?: string; credit?: number | null
}
export type ToolImageJobs = {
  jobs: ToolImageJob[]
  storage: { used_bytes: number; reserved_bytes: number }
}
export type ImageToolApi = {
  load: () => Promise<ImageConfigView>
  save: (config: ImageGenerationConfig) => Promise<ImageConfigView>
  saveTimeout?: (seconds: number) => Promise<unknown>
  jobs: () => Promise<ToolImageJobs>
  remove: (id: string) => Promise<unknown>
  download: (file: ToolImageFile) => Promise<void>
}
export const emptyImageTarget = (): ImageTarget => ({ provider: '', model: '', account_id: '' })
