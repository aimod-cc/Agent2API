import type { ProviderConfig } from './add-account-configs'
import { t } from '../i18n'

export const DEEPSEEK_WEB: ProviderConfig = {
  provider: 'deepseek-web',
  label: t('DeepSeek 网页'),
  desktop: false,
  manualTitle: t('粘贴网页访问令牌'),
  manualNote: t('在 chat.deepseek.com 登录本人账号后，从浏览器开发者工具的 Application → Local Storage 复制 userToken 的 value。Cookie 可选，需补充时粘贴该站点请求里的完整 Cookie；登录态失效或要求人机验证时，请回官方网页手动登录后更新。这里不接受收费 API Key。'),
  fields: [
    {
      key: 'accessToken', label: t('网页访问令牌'), rows: 3,
      placeholder: t('chat.deepseek.com 的 userToken.value，不是收费 API Key'),
    },
    {
      key: 'cookie', label: t('网页 Cookie'), rows: 3, optional: true,
      placeholder: t('官方网页正常登录后的 Cookie；风控时请手动更新'),
    },
    { key: 'name', label: t('备注名'), optional: true, placeholder: t('可选，留空自动生成') },
  ],
  fileImport: {
    accept: '.json,application/json',
    note: t('选择包含该提供商凭证的 JSON 文件，或本软件导出的单账号 JSON 文件。导入后凭证保存在网关，可独立使用。'),
  },
}
