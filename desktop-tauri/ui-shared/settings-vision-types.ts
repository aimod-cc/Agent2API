export type VisionMode = 'assist' | 'reroute'
export type VisionTarget = { provider: string; model: string; reasoning: string }
export type VisionOverride = { provider: string; model: string; mode: VisionMode | 'off' }
export type VisionSettings = {
  enabled: boolean
  mode: VisionMode
  assist: { source: 'shared' | 'custom'; target: VisionTarget | null; sharedTarget: VisionTarget | null; reuseResults: boolean }
  reroute: { preferCurrentRoute: boolean; fallback: 'stop' | 'designated'; targets: VisionTarget[] }
  overrides: VisionOverride[]
  unknownCapability: 'try_original' | 'compat'
  timeoutSeconds: number
}
export type VisionModel = {
  provider: string
  providerLabel: string
  model: string
  name: string
  accountCount: number
  capabilities: { supportsImages: boolean | null }
}
export type VisionView = {
  config: VisionSettings
  revision: string
  models: VisionModel[]
  sharedTarget: VisionTarget | null
  maxTargets: number
  maxOverrides: number
}
export type VisionCompatApi = {
  load(): Promise<VisionView>
  save(payload: { config: VisionSettings; revision: string; consent: boolean }): Promise<VisionView>
}
export function visionTargetKey(target: Pick<VisionTarget, 'provider' | 'model'>): string {
  return JSON.stringify([target.provider, target.model])
}
