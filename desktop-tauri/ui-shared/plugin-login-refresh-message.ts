type Translate = (key: string, params?: Record<string, string | number>) => string

export function credentialRefreshMessage(message: string, t: Translate): string {
  if (message === 'CodeArts 账号尚未授权刷新 Token，请通过「CodeArts 网页登录」完成授权。') {
    return t('CodeArts 账号尚未授权刷新 Token，请通过「CodeArts 网页登录」完成授权。')
  }
  if (message === '刷新失败，账号与续期记录不一致，尚未向官方发送请求。') {
    return t('刷新失败，账号与续期记录不一致，尚未向官方发送请求。')
  }
  const missing = /^刷新失败，缺少 ([a-z_][a-z0-9_.]*)，请通过「CodeArts 网页登录」完成授权。$/.exec(message)
  if (missing) {
    return t('刷新失败，缺少 {field}，请通过「CodeArts 网页登录」完成授权。', { field: missing[1] })
  }
  const invalid = /^刷新失败，([a-z_][a-z0-9_.]*(?: \/ [a-z_][a-z0-9_.]*)?) 无效，请通过「CodeArts 网页登录」完成授权。$/.exec(message)
  if (invalid) {
    return t('刷新失败，{field} 无效，请通过「CodeArts 网页登录」完成授权。', { field: invalid[1] })
  }
  return t(message)
}
