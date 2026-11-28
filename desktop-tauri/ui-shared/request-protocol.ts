export type RequestProtocolDisplay = {
  client: string
  upstream: string
  converted: boolean
  /** 列表主行展示文本（发生转换时为上游协议，未转换时为唯一协议，空为 ''） */
  mainLabel: string
  /** 列表次行展示文本（仅发生转换时为下游请求协议，否则为 ''） */
  subLabel: string
  mainFullLabel: string
  subFullLabel: string
  /** 详情页与移动端单行展示：未转换 `/chat/completions`，发生转换 `/messages → /chat/completions`，未知 `''` */
  inlineText: string
}

type Translate = (key: string) => string

const identity: Translate = text => text

export function normalizeRequestProtocol(raw: unknown): string {
  const trimmed = String(raw ?? '').trim().toLowerCase()
  switch (trimmed) {
    case '':
      return ''
    case 'chat':
    case 'chat_completions':
    case 'openai_chat':
    case 'openai':
    case '/chat/completions':
    case '/v1/chat/completions':
      return 'chat_completions'
    case 'responses':
    case 'openai_responses':
    case '/responses':
    case '/v1/responses':
      return 'responses'
    case 'anthropic':
    case 'messages':
    case 'anthropic_messages':
    case '/messages':
    case '/v1/messages':
      return 'anthropic'
    case 'gemini':
    case 'antigravity_gemini':
    case ':generatecontent':
    case ':streamgeneratecontent':
      return 'gemini'
    case 'images_generations':
    case 'image_generations':
    case '/images/generations':
    case '/v1/images/generations':
      return 'images_generations'
    case 'images_edits':
    case 'image_edits':
    case '/images/edits':
    case '/v1/images/edits':
      return 'images_edits'
    case 'images':
    case 'openai_images':
    case '/images':
      return 'images'
    case 'claude_cli':
      return 'claude_cli'
    case 'ai_sdk_ndjson':
      return 'ai_sdk_ndjson'
    case 'chatglm_snapshot':
      return 'chatglm_snapshot'
    case 'proprietary':
      return 'proprietary'
    default:
      return trimmed
  }
}

export function requestProtocolShortLabel(raw: unknown, translate: Translate = identity): string {
  const key = normalizeRequestProtocol(raw)
  switch (key) {
    case '':
      return ''
    case 'chat_completions':
      return '/chat/completions'
    case 'responses':
      return '/responses'
    case 'anthropic':
      return '/messages'
    case 'gemini':
      return ':generateContent'
    case 'images_generations':
      return '/images/generations'
    case 'images_edits':
      return '/images/edits'
    case 'images':
      return '/images'
    case 'claude_cli':
      return 'Claude CLI'
    case 'ai_sdk_ndjson':
      return 'AI SDK'
    case 'chatglm_snapshot':
      return 'ChatGLM'
    case 'proprietary':
      return translate('私有协议')
    default:
      return String(raw ?? '').trim()
  }
}

export function requestProtocolFullLabel(raw: unknown, translate: Translate = identity): string {
  const key = normalizeRequestProtocol(raw)
  switch (key) {
    case '':
      return ''
    case 'chat_completions':
      return '/chat/completions (OpenAI Chat Completions)'
    case 'responses':
      return '/responses (OpenAI Responses)'
    case 'anthropic':
      return '/messages (Anthropic Messages)'
    case 'gemini':
      return ':generateContent (Gemini API)'
    case 'images_generations':
      return '/images/generations (OpenAI Images)'
    case 'images_edits':
      return '/images/edits (OpenAI Images)'
    case 'images':
      return '/images (OpenAI Images)'
    case 'claude_cli':
      return 'Claude Code CLI'
    case 'ai_sdk_ndjson':
      return 'AI SDK NDJSON'
    case 'chatglm_snapshot':
      return 'ChatGLM'
    case 'proprietary':
      return translate('私有协议')
    default:
      return String(raw ?? '').trim()
  }
}

function isSpecificImages(key: string): boolean {
  return key === 'images_generations' || key === 'images_edits'
}

export function describeRequestProtocol(
  clientRaw: unknown,
  upstreamRaw: unknown,
  translate: Translate = identity,
): RequestProtocolDisplay {
  const client = normalizeRequestProtocol(clientRaw)
  const upstream = normalizeRequestProtocol(upstreamRaw)
  const sameImageFamily =
    (client === 'images' && isSpecificImages(upstream))
    || (upstream === 'images' && isSpecificImages(client))
  const converted = Boolean(client && upstream && client !== upstream && !sameImageFamily)
  const shown = sameImageFamily
    ? (isSpecificImages(upstream) ? upstream : client)
    : (upstream || client)
  const mainKey = converted ? upstream : shown
  const subKey = converted ? client : ''
  const mainLabel = requestProtocolShortLabel(mainKey, translate)
  const subLabel = requestProtocolShortLabel(subKey, translate)
  const mainFullLabel = requestProtocolFullLabel(mainKey, translate)
  const subFullLabel = requestProtocolFullLabel(subKey, translate)
  const inlineText = converted
    ? `${subLabel} → ${mainLabel}`
    : mainLabel
  return {
    client,
    upstream,
    converted,
    mainLabel,
    subLabel,
    mainFullLabel,
    subFullLabel,
    inlineText,
  }
}
