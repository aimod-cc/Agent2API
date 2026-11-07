/**
 * 图片兼容的前后端契约。
 *
 * 两种处理方式各是一份固定设置（不是可以增删的配置列表）：各自带一份「适用模型」名单，
 * 没被任何名单选中的模型直接原样放行；原本能看图的模型本来就不参与。
 */
export type VisionWay = 'assist' | 'reroute'
export type VisionModelRef = { provider: string; model: string }
export type VisionTarget = { provider: string; model: string; reasoning: string }
export type VisionSource = 'shared' | 'custom'
export type VisionFallback = 'stop' | 'designated'

export type VisionAssist = {
  /** 这些模型看不了图片时走辅助识图；空 = 这种方式不参与 */
  models: VisionModelRef[]
  source: VisionSource
  target: VisionTarget | null
  reuseResults: boolean
}

export type VisionReroute = {
  /** 这些模型的请求整体改交视觉模型；空 = 这种方式不参与 */
  models: VisionModelRef[]
  preferCurrentRoute: boolean
  fallback: VisionFallback
  /** 指定的接管模型与备选，按顺序尝试；第一项就是「接管回答的模型」 */
  targets: VisionTarget[]
}

export type VisionSettings = {
  enabled: boolean
  assist: VisionAssist
  reroute: VisionReroute
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
  maxModels: number
  /** 服务端把旧版「全局二选一 + 模型单独配置」展开成了名单，界面要提示一句 */
  legacyExpanded: boolean
}

export type VisionCompatApi = {
  load(): Promise<VisionView>
  /** 没有保存步骤：改一次就提交一次，返回服务端的最新状态 */
  save(payload: { config: VisionSettings; revision: string }): Promise<VisionView>
}

export function visionTargetKey(target: Pick<VisionTarget, 'provider' | 'model'>): string {
  return JSON.stringify([target.provider, target.model])
}

/** 名单里的模型（provider + model）与视觉模型目标用的是同一套键 */
export const visionModelKey = visionTargetKey

/** 两块名单的重叠项：两边都选了时按辅助识图处理，界面要把这件事直接说出来 */
export function visionOverlap(config: VisionSettings): VisionModelRef[] {
  const other = new Set(config.reroute.models.map(visionModelKey))
  return config.assist.models.filter(entry => other.has(visionModelKey(entry)))
}
