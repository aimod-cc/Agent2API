export type ForceUpdatePolicy = { required: boolean; minimumVersion: string; targetVersion: string; message: string; source?: string }

export type UpdateCapability = {
  deployment: 'desktop' | 'docker' | 'systemd' | 'windows-service' | 'foreground'
  mode: 'desktop' | 'docker' | 'systemd' | 'windows-service' | 'manual'
  packageKind: 'desktop' | 'server'
  installSupported: boolean
  reason?: 'disabled' | 'helper_missing' | 'helper_unavailable' | 'custom_installation' | 'unsupported_arch' | 'windows_service' | 'windows_setup_required' | 'windows_update_permissions' | 'unmanaged' | null
}

export type ServiceUpdateTask = {
  id: string
  version: string
  phase: 'downloading' | 'verifying' | 'installing' | 'restarting' | 'completed' | 'failed'
  startedAt: number
  updatedAt: number
  error?: string | null
}

export type SystemdUpdateTask = ServiceUpdateTask

export function updateUnavailableReason(capability: UpdateCapability, t: Translator): string {
  switch (capability.reason) {
    case 'windows_setup_required': return t('当前 Windows 服务尚未接入一键更新。请解压新版服务端包，以管理员身份运行一次 install-service.ps1，原有账号、目录、端口和配置会保留。')
    case 'windows_update_permissions': return t('Windows 服务升级权限或目录保护不符合要求。请以管理员身份重跑新版 install-service.ps1；自定义服务账号或外部托管环境请按原方式升级。')
    case 'disabled': return t('管理员已关闭面板一键更新，请使用手动升级方式。')
    case 'helper_missing': return t('当前安装尚未接入一键更新。请下载新版服务端包，执行一次 sudo bash install.sh --upgrade；之后即可在面板更新。')
    case 'helper_unavailable': return t('一键更新组件不可用或权限配置不正确。请使用新版安装包执行 sudo bash install.sh --upgrade 修复。')
    case 'custom_installation': return t('检测到自定义程序或界面路径，一键更新不会覆盖这些位置。请按原部署方式升级。')
    case 'unsupported_arch': return t('当前系统或架构暂不支持面板自动安装，请查看发布页。')
    case 'windows_service': return t('Windows 服务部署需要手动升级。请解压新版服务端包，以管理员身份运行 install-service.ps1；账号与配置会保留。')
    default: return t('当前为直接运行的服务端，请停止程序后替换服务端包，再重新启动。')
  }
}

export type UpdateChannels = {
  stable: boolean
  beta: boolean
  saved?: boolean
}

export type UpdateCheckPlan = {
  enabled: boolean
  interval: number
  saved?: boolean
}

export type UpdateSettingsMode = 'follow' | 'custom'

type Translator = (key: string, params?: Record<string, string | number>) => string

/**
 * 「有新版本，但没有可下载的包」时后端给的诊断（check 响应的 `assetProbe`）。
 *
 * ── 为什么需要它 ────────────────────────────────────────────
 * 光凭 `asset` 为空说不清原因，而三种原因的处置办法完全不同：上游压根没传包
 * （等构建 / 自己去发布页下）、只传了别的平台（同上）、被更新源的访问方式挡掉了
 * （去「更新设置」改访问方式）。这三个字段就是用来分开它们的。
 *
 * 字段全可选：升级前落库的那份检查结果里没有它，界面必须能退化到「不说原因」。
 */
export type AssetProbe = {
  /** 这一版 release 上传的文件数（按更新源白名单过滤**前**的清单） */
  assetsTotal?: number
  /** 这些文件出现过的后缀（小写、带点、去重） */
  suffixes?: string[]
  /** 有文件因更新源的访问方式（加速前缀 / 自定义 API）没通过而被丢掉 */
  filteredBySource?: boolean
  /** 本机要找的后缀（桌面安装包；容器内更新是空表） */
  expectedSuffixes?: string[]
  /** 本机要找的完整文件名（容器内更新包；桌面是 null） */
  expectedName?: string | null
}

/**
 * 「为什么没有可下载的包」——toast / 弹窗 / 面板状态行共用这一处口径。
 *
 * 返回的是一句能独立看懂的**原因**，由调用方拼进自己的句子里（桌面与容器的
 * 前半句不同，见 update-panel 与 update-modal）。取不到诊断时退化成「本机要
 * 什么」这一层 —— 至少让排查有方向，而不是只丢一句「没有包」。
 */
export function assetMissingReason(
  probe: AssetProbe | null | undefined,
  options: { selfUpdate?: boolean; fallbackSuffix?: string; serverPackage?: boolean },
  t: Translator,
): string {
  // ① 被更新源的访问方式过滤掉：文件其实传了，是我们这边把它们挡掉了。
  // 先说这条 —— 它的处置办法（去「更新设置」改访问方式）与另外两种完全不同
  if (probe?.filteredBySource) {
    return t('发布页上的文件被更新源的访问方式过滤掉了，请检查「更新设置」里的 GitHub 访问方式')
  }
  // ② 一个文件都没传：构建失败，或者还在打包上传
  if (probe?.assetsTotal === 0) {
    return t('这一版还没有上传任何文件（构建可能失败，或仍在打包上传）')
  }
  // ③ 传了文件，但没有本机要的那一份 —— 把「有什么」与「要什么」都摆出来。
  // 后端给的 expected 优先（Linux 桌面壳是 [deb, AppImage] 两个候选），
  // 老后端没给时退到调用方按 installerKind 推的那个后缀
  const seen = (probe?.suffixes || []).filter(Boolean).slice(0, 6)
  const serverPackage = options.serverPackage || options.selfUpdate
  const wanted = serverPackage
    ? String(probe?.expectedName || '')
    : ((probe?.expectedSuffixes ?? [options.fallbackSuffix || '']).filter(Boolean).join(' / '))
  if (seen.length && wanted) {
    return t('未找到适用的更新包。需要：{wanted}；发布页包含的文件类型：{seen}。', { seen: seen.join(' / '), wanted })
  }
  // ④ 兜底：连「有什么」都不知道，至少说清「本机要什么」
  if (serverPackage) {
    return wanted
      ? t('发布页上没有 {wanted} 这个文件', { wanted })
      : t('发布页上没有对应本机架构的服务端更新包')
  }
  return wanted
    ? t('这一版没有发布本机需要的安装包（{wanted}）', { wanted })
    : t('这一版没有为当前平台提供安装包')
}

export function githubErrorMessage(error: unknown, t: Translator, settings: 'update' | 'plugin'): string {
  const message = error instanceof Error ? error.message : String(error ?? '')
  if (message === 'GitHub 接口限流中，请在「更新设置」的「GitHub 令牌」中填写凭证，以提高请求限额。'
    || message === 'GitHub 接口限流中，请在「插件设置」的「GitHub 令牌」中填写凭证，以提高请求限额。') {
    return settings === 'plugin'
      ? t('GitHub 接口限流中，请在「插件设置」的「GitHub 令牌」中填写凭证，以提高请求限额。')
      : t('GitHub 接口限流中，请在「更新设置」的「GitHub 令牌」中填写凭证，以提高请求限额。')
  }
  const cooldown = /^GitHub 接口限流中，请在 (\d+) 秒后重试$/.exec(message)
  if (cooldown) return t('GitHub 接口限流中，请在 {seconds} 秒后重试', { seconds: cooldown[1] })
  if (message === 'GitHub 接口限流中，请稍后重试') return t('GitHub 接口限流中，请稍后重试')
  return message
}
