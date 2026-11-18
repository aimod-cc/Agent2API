import { t } from '../i18n'
import { updateErrorMessage } from '../../../ui-shared/updates'
import type { PluginMutation } from './plugins-types'

export type PluginBridge = {
  __TAURI_INTERNALS__?: {
    invoke?: (command: string, args?: unknown) => Promise<unknown>
  }
  wbApp?: {
    toast?: (message: string, kind?: 'err' | 'ok') => void
    refresh?: () => Promise<boolean>
    refreshPluginPages?: (payload?: PluginMutation) => Promise<void> | void
    readonly currentPage?: string
  }
}

export function bridge(): PluginBridge {
  return window as unknown as PluginBridge
}

export function errorMessage(error: unknown): string {
  return updateErrorMessage(error, t)
}

export function toast(message: string, kind?: 'err' | 'ok'): void {
  bridge().wbApp?.toast?.(errorMessage(message), kind)
}

export async function api<T>(method: string, path: string, body?: unknown, options?: { retry: 'never' }): Promise<T> {
  const invoke = bridge().__TAURI_INTERNALS__?.invoke
  if (!invoke) throw new Error(t('桌面运行时不可用（Tauri 未初始化）'))
  const result = await invoke('api_request', {
    request: { method, path, body: body === undefined ? null : body, ...(options ? { retry: options.retry } : {}) },
  })
  if (result && typeof result === 'object' && 'success' in result) {
    const envelope = result as { success?: boolean; data?: unknown; error?: string }
    if (envelope.success === false) throw new Error(envelope.error || t('请求失败'))
    return envelope.data as T
  }
  return result as T
}

export function formatTime(value: number): string {
  return value ? new Date(value).toLocaleString() : '—'
}

export function formatSize(bytes: number): string {
  if (!bytes) return ''
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
  if (bytes >= 1024) return `${Math.round(bytes / 1024)} KB`
  return `${bytes} B`
}

export function platformLabel(target: string): string {
  const labels: Record<string, string> = {
    'x86_64-pc-windows-msvc': t('Windows (x64)'),
    'aarch64-pc-windows-msvc': t('Windows (ARM64)'),
    'x86_64-apple-darwin': t('macOS (Intel)'),
    'aarch64-apple-darwin': t('macOS (Apple Silicon)'),
    'x86_64-unknown-linux-gnu': t('Linux (amd64)'),
    'aarch64-unknown-linux-gnu': t('Linux (arm64)'),
  }
  return labels[target] || target
}

export function readBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onerror = () => reject(new Error(t('读取文件失败')))
    reader.onload = () => {
      const text = typeof reader.result === 'string' ? reader.result : ''
      const index = text.indexOf('base64,')
      resolve(index >= 0 ? text.slice(index + 7) : text)
    }
    reader.readAsDataURL(file)
  })
}

export function pluginName(id: string, name?: string | null): string {
  if (id === 'agent2api-providers') return t('提供商增强')
  if (id === 'agent2api-web-providers') return t('网页提供商')
  return name || id
}

export function pluginDescription(id: string, description?: string | null): string {
  if (id === 'agent2api-providers') {
    return t('为网关扩展多家 AI 提供商，支持账号登录与导入、模型调用、凭证续期、模型目录、余额查询和签到。具体能力以各提供商的支持情况为准。')
  }
  if (id === 'agent2api-web-providers') {
    return t('为 Agent2API 扩展网页提供商，支持智谱清言的模型调用、访客与登录账号、凭证续期、模型目录、积分查询、签到和任务领取。')
  }
  return description || t('作者尚未提供插件介绍。')
}
