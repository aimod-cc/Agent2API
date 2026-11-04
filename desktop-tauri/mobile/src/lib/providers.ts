/**
 * 提供商图标与展示名。
 *
 * ── 图标为什么用 <img> 而不是内联 SVG ────────────────────────
 * 提供商图标是一批彩色 PNG（`ui/assets/providers/*.png`），由桌面端同一套
 * 资源提供。移动端页面与它们同源（都在 `ui/` 下），直接引相对路径即可，
 * 不必再打包一份。
 *
 * ── 为什么没有「第二份提供商清单」─────────────────────────────
 * 项目里明确禁止前端维护写死的 provider 列表（见 account_store 模块头）：
 * 加一家内置提供商时，后端注册表是唯一事实来源。这里的映射只解决一件
 * 后端不管的事 —— **图标文件叫什么、中文展示名怎么念**。认不出的 id
 * 一律回落到首字徽章，界面不会空一块。
 */

import { t } from '../i18n'

/** 提供商 id → 图标文件（相对 `ui/` 根，页面在 `ui/m.html` 所以前缀 `m/` 不用加） */
const ICONS: Record<string, string> = {
  workbuddy: 'assets/providers/workbuddy.png',
  'workbuddy-intl': 'assets/providers/workbuddy.png',
  raccoon: 'assets/providers/raccoon.png',
  catpaw: 'assets/providers/catpaw.png',
  autoclaw: 'assets/providers/autoclaw.png',
  'autoclaw-intl': 'assets/providers/autoclaw.png',
  qoder: 'assets/providers/qoder.png',
  'qoder-intl': 'assets/providers/qoder.png',
  'cline-free': 'assets/providers/cline.png',
  cline: 'assets/providers/cline.png',
  'cline-pass': 'assets/providers/cline.png',
  accio: 'assets/providers/accio.png',
  'accio-cn': 'assets/providers/accio.png',
  'accio-intl': 'assets/providers/accio.png',
  zcode: 'assets/providers/zcode.png',
  'zcode-intl': 'assets/providers/zcode.png',
  codearts: 'assets/providers/codearts.png',
  trae: 'assets/providers/trae.png',
  loomy: 'assets/providers/loomy.png',
  kuku: 'assets/providers/kuku.png',
  monkeycode: 'assets/providers/monkeycode.png',
  'monkeycode-intl': 'assets/providers/monkeycode.png',
  commandcode: 'assets/providers/commandcode.png',
  antigravity: 'assets/providers/antigravity.png',
  officeace: 'assets/providers/officeace.png',
  orcaterm: 'assets/providers/orcaterm.png',
  chatglm: 'assets/providers/chatglm.png',
  'deepseek-web': 'assets/providers/deepseek.png',
  tabbit: 'assets/providers/tabbit.png',
  'tabbit-intl': 'assets/providers/tabbit.png',
  modelscope: 'assets/providers/modelscope.png',
  'modelscope-intl': 'assets/providers/modelscope.png',
  atomcode: 'assets/providers/atomcode.png',
  lobsterai: 'assets/providers/lobsterai.png',
  doubaowork: 'assets/providers/doubaowork.png',
  cursor: 'assets/providers/cursor.svg',
  kiro: 'assets/providers/kiro.svg',
  astudio: 'assets/providers/astudio.png',
  devin: 'assets/providers/devin.svg',
  factory: 'assets/providers/factory.svg',
  zed: 'assets/providers/zed.svg',
  grok: 'assets/providers/grok.svg',
  'minimax-code': 'assets/providers/minimax-code.svg',
  'minimax-code-global': 'assets/providers/minimax-code.svg',
  qwenwork: 'assets/providers/qwenwork.svg',
  'qwenwork-intl': 'assets/providers/qwenwork.svg',
  'qwenwork-enterprise': 'assets/providers/qwenwork.svg',
  'kimi-work': 'assets/providers/kimi-work.svg',
  opendesign: 'assets/providers/opendesign.svg',
  'nous-portal': 'assets/providers/nous-portal.svg',
  'claude-subscription': 'assets/providers/anthropic.png',
  'codex-subscription': 'assets/providers/openai.png',
  'chatgpt-subscription': 'assets/providers/openai.png',
  'copilot-subscription': 'assets/providers/copilot.svg',
  'gemini-subscription': 'assets/providers/gemini-cli.svg',
}

/** 后端没给 label 时的兜底展示名（认不出的 id 原样显示） */
const LABELS: Record<string, string> = {
  astudio: 'AStudio',
  workbuddy: 'WorkBuddy 国内版',
  'workbuddy-intl': 'WorkBuddy 国际版',
  raccoon: '小浣熊',
  catpaw: 'CatPaw',
  autoclaw: 'AutoClaw 国内版',
  'autoclaw-intl': 'AutoClaw 国际版',
  qoder: 'Qoder 中国版',
  'qoder-intl': 'Qoder 国际版',
  'cline-free': 'Cline 免费池',
  cline: 'Cline Free',
  'cline-pass': 'Cline Pass',
  accio: 'Accio 国际版',
  'accio-cn': 'Accio 国内版',
  'accio-intl': 'Accio 国内版',
  zcode: 'ZCode 国内版',
  'zcode-intl': 'ZCode 国际版',
  codearts: 'CodeArts',
  trae: 'Trae',
  loomy: 'Loomy',
  kuku: 'KukuAI',
  monkeycode: 'MonkeyCode 国内版',
  'monkeycode-intl': 'MonkeyCode 国际版',
  commandcode: 'Command Code',
  antigravity: 'Antigravity',
  officeace: 'OfficeAce 果办',
  orcaterm: 'OrcaTerm',
  chatglm: '智谱清言',
  tabbit: 'Tabbit 国内版',
  'tabbit-intl': 'Tabbit 国际版',
  modelscope: 'ModelScope 国内版',
  'modelscope-intl': 'ModelScope 国际版',
  atomcode: 'AtomCode',
  lobsterai: 'LobsterAI',
  doubaowork: '豆包工作',
  cursor: 'Cursor',
  kiro: 'Kiro',
  devin: 'Devin',
  factory: 'Factory',
  zed: 'Zed',
  grok: 'Grok (SuperGrok)',
  'minimax-code': 'MiniMax Code 国内版',
  'minimax-code-global': 'MiniMax Code 国际版',
  qwenwork: '千问办公 国内版',
  'qwenwork-intl': '千问办公 国际版',
  'qwenwork-enterprise': 'QwenWork 千问办公企业版',
  'kimi-work': 'Kimi Work',
  opendesign: 'Open Design',
  'nous-portal': 'Nous Portal',
  'claude-subscription': 'Claude 订阅',
  'codex-subscription': 'Codex 订阅',
  'chatgpt-subscription': 'ChatGPT API 订阅',
  'copilot-subscription': 'GitHub Copilot',
  'gemini-subscription': 'Gemini CLI',
}

/** 图标地址；认不出的 id 返回空串（调用方画首字徽章） */
export function providerIcon(id: string | undefined): string {
  return (id && ICONS[id]) || ''
}

/** 展示名：优先用后端给的 label，其次本地映射，最后原样回显 id */
export function providerLabel(id: string | undefined, fallback?: string): string {
  if (fallback) return t(fallback)
  if (!id) return t('未知')
  return t(LABELS[id] || id)
}
