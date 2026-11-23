import { t } from './i18n'
import { formatUnitCompact, numberLocaleTag } from './units'

const PROVIDER_LABELS: Record<string, string> = {
  codebuddy: 'CodeBuddy',
  workbuddy: 'WorkBuddy 国内版',
  'workbuddy-intl': 'WorkBuddy 国际版',
  raccoon: '小浣熊',
  catpaw: 'CatPaw',
  autoclaw: 'AutoClaw 国内版',
  'autoclaw-intl': 'AutoClaw 国际版',
  qoder: 'Qoder 中国版',
  'qoder-intl': 'Qoder 国际版',
  zcode: 'ZCode 国内版',
  'zcode-intl': 'ZCode 国际版',
  codearts: 'CodeArts',
  officeace: 'OfficeAce 果办',
  orcaterm: 'OrcaTerm',
  chatglm: '智谱清言',
  zhipu: '智谱清言',
  glm: '智谱 GLM',
  deepseek: 'DeepSeek',
  kimi: 'Kimi',
  'kimi-work': 'Kimi Work',
  moonshot: 'Kimi',
  doubao: '豆包',
  doubaowork: '豆包工作',
  qwen: '通义千问',
  tongyi: '通义千问',
  qwenwork: '千问办公 国内版',
  'qwenwork-intl': '千问办公 国际版',
  'qwenwork-enterprise': '千问办公企业版',
  claude: 'Claude',
  'claude-subscription': 'Claude 订阅',
  anthropic: 'Anthropic',
  openai: 'OpenAI',
  'codex-subscription': 'Codex 订阅',
  'chatgpt-subscription': 'ChatGPT API 订阅',
  gemini: 'Gemini',
  'gemini-subscription': 'Gemini CLI',
  antigravity: 'Antigravity',
  cursor: 'Cursor',
  windsurf: 'Windsurf',
  trae: 'Trae',
  github: 'GitHub Copilot',
  copilot: 'GitHub Copilot',
  'copilot-subscription': 'GitHub Copilot',
  augment: 'Augment',
  iflow: 'iFlow',
  yuanbao: '腾讯元宝',
  hunyuan: '腾讯混元',
  minimax: 'MiniMax',
  'minimax-code': 'MiniMax Code 国内版',
  'minimax-code-global': 'MiniMax Code 国际版',
  stepfun: '阶跃星辰',
  baidu: '文心一言',
  siliconflow: 'SiliconFlow',
  astudio: 'AStudio',
  quwork: 'QuWork',
  loomy: 'Loomy',
  'loomy-plugin': 'Loomy',
  kuku: 'KukuAI',
  codar: '科搭 Codar',
  croli: 'Croli',
  dumate: 'DuMate',
  'dumate-plugin': 'DuMate',
  'intern-inkstone': '书生·墨石',
  'wps-lingxi': 'WPS 灵犀',
  deepaicode: 'DeepAICode',
  'agnes-code': 'Agnes Code',
  modelscope: 'ModelScope 国内版',
  'modelscope-intl': 'ModelScope 国际版',
  atomcode: 'AtomCode',
  lobsterai: 'LobsterAI',
  tabbit: 'Tabbit 国内版',
  'tabbit-intl': 'Tabbit 国际版',
  monkeycode: 'MonkeyCode 国内版',
  'monkeycode-intl': 'MonkeyCode 国际版',
  'cline-free': 'Cline 免费池',
  cline: 'Cline Free',
  'cline-pass': 'Cline Pass',
  accio: 'Accio 国际版',
  'accio-cn': 'Accio 国内版',
  'accio-intl': 'Accio 国内版',
  commandcode: 'Command Code',
  kiro: 'Kiro',
  devin: 'Devin',
  factory: 'Factory',
  zed: 'Zed',
  grok: 'Grok (SuperGrok)',
  opendesign: 'Open Design',
  'nous-portal': 'Nous Portal',
}

const dynamicProviderLabels: Record<string, string> = {}

export function syncProviderLabels(
  list?: Array<{
    id?: string | null
    provider?: string | null
    name?: string | null
    label?: string | null
    providerLabel?: string | null
  }> | null,
): boolean {
  if (!Array.isArray(list)) return false
  let changed = false
  for (const item of list) {
    const id = (item?.id || item?.provider || '').trim().toLowerCase()
    const label = (item?.label || item?.providerLabel || item?.name || '').trim()
    if (id && label && dynamicProviderLabels[id] !== label) {
      dynamicProviderLabels[id] = label
      changed = true
    }
  }
  return changed
}

export function providerDisplayName(provider?: string | null, fallbackLabel?: string | null): string {
  if (fallbackLabel && fallbackLabel.trim()) return t(fallbackLabel.trim())
  const key = (provider || '').trim().toLowerCase()
  if (!key) return t('未知提供商')
  const mapped = dynamicProviderLabels[key] || PROVIDER_LABELS[key]
  return mapped ? t(mapped) : provider || key
}

export function formatCompact(num?: number | null): string {
  return formatUnitCompact(num)
}

export function formatNumber(num?: number | null): string {
  if (num === undefined || num === null || !Number.isFinite(num)) return '0'
  return Math.round(num).toLocaleString(numberLocaleTag())
}

export function formatPercent(ratioOrPercent?: number | null, isRatio = false): string {
  if (ratioOrPercent === undefined || ratioOrPercent === null || !Number.isFinite(ratioOrPercent)) {
    return '0%'
  }
  const value = isRatio ? ratioOrPercent * 100 : ratioOrPercent
  return `${Math.max(0, Math.min(100, value)).toFixed(1).replace(/\.0$/, '')}%`
}

export function formatDuration(ms?: number | null): string {
  if (ms === undefined || ms === null || !Number.isFinite(ms) || ms < 0) return '—'
  if (ms < 1000) return `${Math.round(ms)} ms`
  return `${(ms / 1000).toFixed(2)} s`
}

export function formatBytes(bytes?: number | null): string {
  if (!bytes || !Number.isFinite(bytes) || bytes <= 0) return '0 B'
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(2)} MB`
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`
}

export function formatRelativeTime(timestampMs?: number | null): string {
  if (!timestampMs || !Number.isFinite(timestampMs)) return '—'
  const ts = timestampMs < 1e11 ? timestampMs * 1000 : timestampMs
  const diffSec = Math.round((ts - Date.now()) / 1000)
  const absSec = Math.abs(diffSec)
  if (absSec < 45) return diffSec >= 0 ? t('即将') : t('刚刚')
  const mins = Math.round(absSec / 60)
  if (mins < 60) return diffSec >= 0 ? `${mins} 分钟后` : `${mins} 分钟前`
  const hours = Math.round(mins / 60)
  if (hours < 24) return diffSec >= 0 ? `${hours} 小时后` : `${hours} 小时前`
  const days = Math.round(hours / 24)
  if (days < 60) return diffSec >= 0 ? `${days} 天后` : `${days} 天前`
  return formatDateTime(ts)
}

export function formatDateTime(timestampMs?: number | null): string {
  if (!timestampMs || !Number.isFinite(timestampMs)) return '—'
  const ts = timestampMs < 1e11 ? timestampMs * 1000 : timestampMs
  const date = new Date(ts)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`
}

export async function copyText(text: string): Promise<boolean> {
  if (!text) return false
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text)
      return true
    }
  } catch {
    /* fallback */
  }
  try {
    const textarea = document.createElement('textarea')
    textarea.value = text
    textarea.style.position = 'fixed'
    textarea.style.opacity = '0'
    document.body.appendChild(textarea)
    textarea.select()
    const ok = document.execCommand('copy')
    document.body.removeChild(textarea)
    return ok
  } catch {
    return false
  }
}
