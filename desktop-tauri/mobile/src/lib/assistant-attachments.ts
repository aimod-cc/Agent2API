import { t } from '../i18n'
import {
  MAX_TEXT_FILES,
  MAX_TEXT_FILE_BYTES,
  type AssistantTextFile,
} from '../../../ui-shared/assistant-files'

export const MAX_ATTACHMENTS = 6

function readAsDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(String(reader.result || ''))
    reader.onerror = () => reject(new Error(t('读取图片失败')))
    reader.readAsDataURL(file)
  })
}

async function prepareImage(file: File): Promise<string> {
  if (!['image/png', 'image/jpeg', 'image/webp'].includes(file.type)) throw new Error(t('参考图仅支持 PNG、JPEG、WebP'))
  if (file.size > 10 * 1024 * 1024) throw new Error(t('单张参考图不能超过 10 MiB'))
  const raw = await readAsDataUrl(file)
  const image = new Image()
  await new Promise<void>((resolve, reject) => {
    image.onload = () => resolve()
    image.onerror = () => reject(new Error(t('读取图片失败')))
    image.src = raw
  })
  if (Math.max(image.naturalWidth, image.naturalHeight) <= 1600 && raw.length <= 1_200_000) return raw
  const scale = Math.min(1, 1600 / Math.max(image.naturalWidth, image.naturalHeight))
  const canvas = document.createElement('canvas')
  canvas.width = Math.max(1, Math.round(image.naturalWidth * scale))
  canvas.height = Math.max(1, Math.round(image.naturalHeight * scale))
  const context = canvas.getContext('2d')
  if (!context) throw new Error(t('读取图片失败'))
  context.drawImage(image, 0, 0, canvas.width, canvas.height)
  const encoded = canvas.toDataURL('image/webp', 0.86)
  return encoded.length < raw.length ? encoded : raw
}

/** Windows 上 .json 的 MIME 经常为空，所以扩展名也认 */
function isTextCredentialFile(file: File): boolean {
  return /\.(json|info)$/i.test(file.name) || file.type === 'application/json'
}

/**
 * 一次读取选中的图片与 JSON / .info 文件。
 *
 * 与桌面端同一口径：图片按张数、文件按个数分别限额；文本文件读原文，二进制
 * 或空文件直接给出可读原因（不做静默丢弃）。
 */
export async function readMobileAttachments(
  files: FileList | File[],
  counts: { images: number; files: number },
): Promise<{ images: string[]; files: AssistantTextFile[] }> {
  const list = Array.from(files)
  const imageFiles = list.filter(file => file.type.startsWith('image/'))
  const textFiles = list.filter(file => !file.type.startsWith('image/') && isTextCredentialFile(file))
  if (imageFiles.length + textFiles.length !== list.length) {
    throw new Error(t('只支持图片和 JSON 凭证文件（.json / .info）'))
  }
  if (imageFiles.length + counts.images > MAX_ATTACHMENTS) {
    throw new Error(t('一次最多 {n} 张图片。', { n: MAX_ATTACHMENTS }))
  }
  if (textFiles.length + counts.files > MAX_TEXT_FILES) {
    throw new Error(t('一次最多 {n} 个文件。', { n: MAX_TEXT_FILES }))
  }
  const images: string[] = []
  for (const file of imageFiles) images.push(await prepareImage(file))
  const result: AssistantTextFile[] = []
  for (const file of textFiles) {
    if (file.size > MAX_TEXT_FILE_BYTES) {
      throw new Error(t('「{name}」超过 {n} MB，请换一个小一点的 JSON 文件。', {
        name: file.name,
        n: MAX_TEXT_FILE_BYTES / 1024 / 1024,
      }))
    }
    const text = (await file.text()).replace(/^\uFEFF/, '')
    if (!text.trim()) throw new Error(t('「{name}」是空文件。', { name: file.name }))
    if (text.includes('\u0000')) throw new Error(t('「{name}」看起来是二进制文件，助手只接收 JSON 文本。', { name: file.name }))
    result.push({ name: file.name, size: file.size, text })
  }
  return { images, files: result }
}
