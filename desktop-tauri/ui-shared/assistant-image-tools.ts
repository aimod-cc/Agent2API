import type { AssistantTextFile } from './assistant-files'
import type { Translate } from './assistant-read-types'

export type ImageAsset = { id: string; url: string; mime: string; width: number; height: number }
export type ImageToolContext = {
  session: string
  images: string[]
  /** 最近一条带附件的用户消息里的 JSON / .info 文件（账号导入工具用） */
  files?: AssistantTextFile[]
  confirmed: boolean
  apiKey?: string
  apiKeyName?: string
  signal?: AbortSignal
}
export type ImageToolResult = { text: string; summary: string; images: ImageAsset[] }
export type ImageToolApi = (path: string, body: unknown, signal?: AbortSignal, apiKey?: string) => Promise<unknown>

export type AssistantImageTool = {
  id: string
  label: string
  kind: 'run'
  desc: string
  params: Record<string, unknown>
  plan: (args: Record<string, unknown>) => { lead: string; table: { head: string[]; rows: string[][] }; note: string }
  run: (args: Record<string, unknown>, context?: ImageToolContext) => Promise<ImageToolResult>
}

const properties = {
  prompt: { type: 'string', description: '完整的生成或编辑要求，包括需要保留的内容。', maxLength: 32000 },
  model: { type: 'string', description: 'list_models 返回的 kind=image 模型 ID 或启用的映射名；不要使用对话模型。' },
  n: { type: 'integer', minimum: 1, maximum: 10, description: '生成数量，默认 1；Antigravity 仅支持 1。' },
  size: { type: 'string', description: 'auto 或 WIDTHxHEIGHT，如 1024x1024；精确支持范围由提供商决定。' },
  quality: { type: 'string', description: '仅传提供商支持的质量；默认 auto。Antigravity 不支持显式质量档。' },
  output_format: { type: 'string', enum: ['png', 'jpeg', 'webp'], description: '图片输出编码；默认保留上游编码。' },
}

function text(value: unknown): string { return typeof value === 'string' ? value.trim() : '' }

function reference(value: unknown, context: ImageToolContext, t: Translate): string {
  const input = text(value)
  const match = /^attachment:(\d+)$/.exec(input)
  if (match) {
    const index = Number(match[1]) - 1
    if (index < 0 || index >= context.images.length) throw new Error(t('参考图片编号不存在'))
    return context.images[index]
  }
  if (/^assistant-image:[a-f0-9]{64}$/.test(input)) return input
  if (/^https?:\/\//i.test(input)) return input
  throw new Error(t('参考图片请使用附件编号、已生成图片 ID 或 HTTP(S) 链接'))
}

export function imageAssets(raw: unknown): ImageAsset[] {
  if (!Array.isArray(raw)) return []
  return raw.flatMap(value => {
    const item = value && typeof value === 'object' ? value as Record<string, unknown> : {}
    const id = text(item.id)
    const mime = text(item.mime)
    if (!/^[a-f0-9]{64}$/.test(id) || !['image/png', 'image/jpeg', 'image/webp'].includes(mime)) return []
    return [{ id, url: `/api/assistant/images/files/${id}`, mime, width: Number(item.width) || 0, height: Number(item.height) || 0 }]
  })
}

export function imageContext(messages: Array<{ role: string; images?: unknown }>): string[] {
  for (let index = messages.length - 1; index >= 0; index--) {
    const message = messages[index]
    if (message.role === 'user' && Array.isArray(message.images) && message.images.length) {
      return message.images.filter((image): image is string => typeof image === 'string')
    }
    const assets = imageAssets(message.images)
    if (assets.length) return assets.map(image => `assistant-image:${image.id}`)
  }
  return []
}

export function createAssistantImageTools(api: ImageToolApi, t: Translate): AssistantImageTool[] {
  return ([false, true] as const).map(editing => ({
    id: editing ? 'edit_images' : 'generate_images',
    label: editing ? t('编辑图片') : t('生成图片'),
    kind: 'run',
    desc: editing
      ? '用自然语言编辑图片或按参考图生成新图，走标准 Images 编辑链路。先用 list_models 选择 supports_image_editing=true 的模型（Loomy、Antigravity、ModelScope 或 WorkBuddy）。images 可写 attachment:1 等当前参考图编号，或之前图片工具返回的 assistant-image:ID；不填时使用最近上传或生成的图片。若无图，请让用户上传。只支持 PNG/JPEG/WebP 参考图，不支持 mask、input_fidelity、流式。会真实消耗额度，不自动重发不确定结果。工具返回的图片已在界面显示，只报告结果，不要编造本地文件路径。'
      : '根据文本生成图片，走标准 Images 生成链路。先用 list_models 选择 kind=image、supports_image_generation=true 的模型（小浣熊、Loomy、Antigravity、ModelScope 或 WorkBuddy）。会真实消耗额度，默认只生成 1 张；仅用户要求时才增加数量。不自动重发不确定结果。工具返回的图片已在界面显示，只报告结果，不要编造本地文件路径。',
    params: {
      type: 'object',
      properties: {
        ...properties,
        ...(editing ? { images: { type: 'array', minItems: 1, maxItems: 16, items: { type: 'string' }, description: '参考图编号 attachment:1 等、assistant-image:ID 或 HTTP(S) URL；不填时用最近一组图片。' } } : {}),
      },
      required: ['prompt', 'model'],
      additionalProperties: false,
    },
    plan: args => ({
      lead: editing ? t('按提示词编辑参考图片。') : t('按提示词生成图片。'),
      table: { head: [t('参数'), t('内容')], rows: [[t('模型'), text(args.model)], [t('提示词'), text(args.prompt)], [t('数量'), String(args.n ?? 1)]] },
      note: t('这会真实发起上游请求并消耗额度'),
    }),
    run: async (args, context) => {
      if (!context?.session) throw new Error(t('图片工具缺少当前会话'))
      if (!context.apiKey) throw new Error(t('请先选择一把 API Key'))
      if (context.signal?.aborted) throw new DOMException('Aborted', 'AbortError')
      const unknown = Object.keys(args).find(key => !Object.hasOwn(properties, key) && !(editing && key === 'images'))
      if (unknown) throw new Error(t('图片工具包含不支持的参数：{name}', { name: unknown }))
      const request: Record<string, unknown> = {}
      for (const key of Object.keys(properties)) if (args[key] !== undefined) request[key] = args[key]
      if (!text(request.prompt) || !text(request.model)) throw new Error(t('图片工具需要提示词和图片模型'))
      if (editing) {
        if (args.images !== undefined && (!Array.isArray(args.images) || !args.images.length)) {
          throw new Error(t('参考图片列表不能为空'))
        }
        const inputs = args.images === undefined ? context.images : (args.images as unknown[]).map(value => reference(value, context, t))
        if (!inputs.length) throw new Error(t('请先上传参考图片，或先生成一张图片'))
        request.images = inputs.map(image => ({ image_url: image }))
      }
      const raw = await api(`/api/assistant/images/${editing ? 'edits' : 'generations'}`, {
        session: context.session, confirmed: context.confirmed, request,
      }, context.signal, context.apiKey)
      const response = raw && typeof raw === 'object' ? raw as Record<string, unknown> : {}
      const images = imageAssets(response.data)
      if (!images.length) throw new Error(t('图片工具没有返回可显示的图片'))
      const result = { ...response, data: images.map(image => ({ ...image, ref: `assistant-image:${image.id}` })), displayed: true }
      return { text: JSON.stringify(result), summary: t('已生成 {n} 张图片', { n: images.length }), images }
    },
  }))
}

export function assistantImageMeta(t: Translate) {
  return [
    { id: 'generate_images', label: t('生成图片'), desc: t('按文本生成图片，结果直接显示在对话中。'), tag: 'cost' as const },
    { id: 'edit_images', label: t('编辑图片'), desc: t('修改上传或已生成的图片，支持多张参考图。'), tag: 'cost' as const },
  ]
}
