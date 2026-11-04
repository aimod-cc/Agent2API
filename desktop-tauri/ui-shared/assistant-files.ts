/**
 * AI 助手的文本附件（JSON / .info）：消息形状、引用解析与传给模型的文本块。
 *
 * ── 为什么单独一个模块 ──────────────────────────────────────
 * 桌面端、手机端、会话历史与两个账号导入工具都要用同一套「附件编号怎么算、
 * 预览怎么拼、文件名怎么匹配」。分散在各端会出现「桌面按最新一条消息编号、
 * 手机按整个会话编号」这种分叉 —— 模型拿到的 `attachment:1` 指向就会变。
 *
 * ── 附件只跟「最近一条带附件的用户消息」走 ──────────────────
 * 用户在一条消息里可以带多个文件，之后的几轮工具调用都用同一组编号。
 * 再发一条不带文件的消息时，旧附件仍可被读取（会话里还留着原文），但编号
 * 仍以最近一次上传的那组为准 —— 与图片参考的 `attachment:N` 口径一致。
 *
 * ── 发给模型的不是全文 ──────────────────────────────────────
 * 文件原文存在消息里（重开历史还在），但每一轮请求只带「元信息 + 顶层字段 +
 * 前 1200 个字符」。完整内容由 `read_attachment` 工具按需读取，否则一份几十
 * KB 的 JSON 会在每一次工具来回里重复占上下文。
 */

export type AssistantTextFile = {
  name: string
  /** 原始字节数（展示用；文本长度另算） */
  size: number
  text: string
}

/** 一次最多带几个文件（与图片分开计数） */
export const MAX_TEXT_FILES = 3
/** 单个文件的字节上限：凭证 JSON 通常只有几 KB，512 KB 已经非常宽松 */
export const MAX_TEXT_FILE_BYTES = 512 * 1024
/** 每一轮请求里每个文件最多带多少字符的预览 */
export const MAX_FILE_PREVIEW_CHARS = 1200

export function isAssistantTextFile(value: unknown): value is AssistantTextFile {
  if (!value || typeof value !== 'object') return false
  const file = value as Record<string, unknown>
  return typeof file.name === 'string'
    && typeof file.text === 'string'
    && (typeof file.size === 'number' || file.size === undefined)
}

/** 宽松读取会话里存的文件数组（手改过的历史不让整条会话打不开） */
export function assistantFiles(value: unknown): AssistantTextFile[] {
  if (!Array.isArray(value)) return []
  return value.filter(isAssistantTextFile).map(file => ({
    name: file.name,
    size: Number.isFinite(file.size) ? Number(file.size) : file.text.length,
    text: file.text,
  }))
}

/** 最近一条带附件的用户消息里的文件（编号从 1 开始就是它里面的顺序） */
export function assistantFileContext(messages: Array<{ role?: unknown; files?: unknown }>): AssistantTextFile[] {
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index]
    if (message?.role !== 'user') continue
    const files = assistantFiles(message.files)
    if (files.length) return files
  }
  return []
}

/**
 * 把模型给的引用解析成附件：`attachment:1`、`1`、`1.json` 或文件名。
 * 编号优先；按名字匹配时只有唯一命中才算，避免同名文件之间猜。
 */
export function assistantFileOf(
  files: AssistantTextFile[],
  ref: unknown,
): { file: AssistantTextFile; index: number } | null {
  const text = typeof ref === 'string' ? ref.trim() : typeof ref === 'number' ? String(ref) : ''
  if (!text) return null
  const numbered = /^attachment:(\d+)$/i.exec(text) || /^(\d+)$/.exec(text)
  if (numbered) {
    const index = Number(numbered[1]) - 1
    if (index < 0 || index >= files.length) return null
    return { file: files[index], index }
  }
  const exact = files.findIndex(file => file.name === text)
  if (exact >= 0) return { file: files[exact], index: exact }
  const lower = text.toLowerCase()
  const matches = files.flatMap((file, index) => file.name.toLowerCase() === lower ? [index] : [])
  if (matches.length === 1) return { file: files[matches[0]], index: matches[0] }
  return null
}

export function formatFileSize(bytes: number): string {
  const value = Math.max(0, Math.round(Number(bytes) || 0))
  if (value < 1024) return `${value} B`
  if (value < 1024 * 1024) {
    const kb = value / 1024
    return `${kb >= 10 ? Math.round(kb) : kb.toFixed(1).replace(/\.0$/, '')} KB`
  }
  const mb = value / 1024 / 1024
  return `${mb >= 10 ? Math.round(mb) : mb.toFixed(1).replace(/\.0$/, '')} MB`
}

/** 顶层字段名（给模型一个「这是什么形状」的摘要）：不是对象 / 数组时返回空 */
export function jsonTopLevelKeys(text: string): string[] {
  try {
    const parsed = JSON.parse(text)
    if (Array.isArray(parsed)) return [`数组（${parsed.length} 项）`]
    if (parsed && typeof parsed === 'object') {
      return Object.keys(parsed as Record<string, unknown>).slice(0, 20)
    }
    return []
  } catch {
    return []
  }
}

/** 文件在模型侧的一段说明（不包含密码学意义上的脱敏：用户明确要把文件交给模型） */
function fileBlock(file: AssistantTextFile, index: number): string {
  const ordinal = index + 1
  const keys = jsonTopLevelKeys(file.text)
  const shape = keys.length ? `顶层字段：${keys.join(', ')}` : '不是合法 JSON（可先用 read_attachment 读取，再修正结构）'
  const truncated = file.text.length > MAX_FILE_PREVIEW_CHARS
  const preview = truncated
    ? `${file.text.slice(0, MAX_FILE_PREVIEW_CHARS)}\n…（预览已截断，剩余 ${file.text.length - MAX_FILE_PREVIEW_CHARS} 字符）`
    : file.text
  const hint = truncated
    ? `完整内容请用 read_attachment 工具读取附件 ${ordinal}；导入时可直接用 attachment:${ordinal} 引用。`
    : `完整内容已在上方；导入时可引用 attachment:${ordinal}，也可以用 read_attachment 重新读取。`
  return [
    `【附件 ${ordinal}：${file.name}（${formatFileSize(file.size)}，${file.text.length} 字符）】`,
    shape,
    '',
    '--- 内容开始 ---',
    preview,
    '--- 内容结束 ---',
    hint,
  ].join('\n')
}

/**
 * 一条用户消息 + 附件 → 发给上游的文本正文。
 *
 * 纯文本仍保持字符串形态（上游对字符串兼容性最好），附件说明只是追加在正文
 * 后面；带图时由调用方把这段文本塞进第一个 text part。
 */
export function userContentWithFiles(content: string, files: AssistantTextFile[]): string {
  const body = content
  if (!files.length) return body
  const blocks = files.map((file, index) => fileBlock(file, index)).join('\n\n')
  return `${body.trim() ? `${body}\n\n` : ''}【用户上传的文件】\n\n${blocks}`
}
