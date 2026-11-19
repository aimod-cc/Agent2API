/**
 * 生成一个足够唯一的短 ID。
 *
 * `crypto.randomUUID` 只在安全上下文（HTTPS、localhost）里存在；管理面板通过
 * 局域网 HTTP 打开时它是 undefined，直接调用会让按钮点了没反应。这里优先用
 * `getRandomValues`（非安全上下文同样可用），再退化到时间戳 + 随机数。
 */
export function newId(): string {
  const bytes = globalThis.crypto?.getRandomValues(new Uint8Array(12))
  if (bytes) return Array.from(bytes, byte => byte.toString(16).padStart(2, '0')).join('')
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`
}
