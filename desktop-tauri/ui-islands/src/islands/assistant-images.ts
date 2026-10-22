/**
 * 助手页的图片处理：文件 → data URL，并按需降采样。
 *
 * ── 为什么是 data URL 而不是 blob URL ───────────────────────
 * 会话要落库（`/api/assistant/sessions` 存的是消息 JSON），blob URL 只在本页
 * 生命周期内有效，重开历史就全裂了；base64 虽然让这份 JSON 变大，但它是唯一
 * 「关掉再打开还在」的形态，而上游的 `image_url` 也正好收 data URI（协议层的
 * 三家适配器都按这个解析）。
 *
 * ── 为什么要降采样 ──────────────────────────────────────────
 * 截图动辄 2560 宽，原图 base64 进消息会把会话 JSON 撑到几 MB（后端单条会话
 * 上限 8 MB），表现为「对话还在屏幕上，但没进历史」。上游真正需要的细节远不到
 * 那个分辨率，所以按长边缩到 [`MAX_IMAGE_EDGE`]、再按需转 WebP。
 */

import { t } from '../i18n'

/** 图片长边上限（像素）。超过就等比缩小 */
export const MAX_IMAGE_EDGE = 1600
/** data URL 的字符数上限（≈ 0.9 MB 二进制）：超过就重编码一次 */
export const MAX_IMAGE_CHARS = 1_200_000

export function readAsDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(String(reader.result || ''))
    reader.onerror = () => reject(reader.error || new Error(t('读取图片失败')))
    reader.readAsDataURL(file)
  })
}

function loadImage(src: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const image = new Image()
    image.onload = () => resolve(image)
    image.onerror = () => reject(new Error(t('读取图片失败')))
    image.src = src
  })
}

/**
 * 读图并**按需降采样**。
 *
 * GIF 不重编码（canvas 只会留下第一帧，动画就丢了）；其余格式统一按长边
 * 缩到 [`MAX_IMAGE_EDGE`]，体积仍超 [`MAX_IMAGE_CHARS`] 时转 WebP ——
 * 两者取字符数更小的那一份，不盲目用重编码结果。
 */
export async function prepareImage(file: File): Promise<string> {
  const raw = await readAsDataUrl(file)
  if (file.type === 'image/gif') return raw
  const image = await loadImage(raw)
  const longest = Math.max(image.naturalWidth, image.naturalHeight)
  const scale = longest > 0 ? Math.min(1, MAX_IMAGE_EDGE / longest) : 1
  if (scale === 1 && raw.length <= MAX_IMAGE_CHARS) return raw
  const canvas = document.createElement('canvas')
  canvas.width = Math.max(1, Math.round(image.naturalWidth * scale))
  canvas.height = Math.max(1, Math.round(image.naturalHeight * scale))
  const context = canvas.getContext('2d')
  if (!context) return raw
  context.drawImage(image, 0, 0, canvas.width, canvas.height)
  const encoded = canvas.toDataURL('image/webp', 0.86)
  return encoded.length < raw.length ? encoded : raw
}
