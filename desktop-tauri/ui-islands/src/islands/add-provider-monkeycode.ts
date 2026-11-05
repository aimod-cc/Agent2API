import type { ProviderConfig } from './add-account-configs'
import { t } from '../i18n'

function monkeycodeForm(provider: string, label: string, siteNote: string): ProviderConfig {
  return {
    provider,
    label,
    desktop: false,
    webLogin: {
      desktopOnly: true,
      noteHtml: t('在所选站点的 MonkeyCode 官方页面完成登录和验证码，成功后自动读取登录态并加入账号列表，无需手动复制 Cookie。'),
      button: t('打开网页登录'),
      hint: t('内嵌窗口打开；完成后自动加入列表，关窗即取消等待'),
      busyText: t('等待 MonkeyCode 登录完成…'),
      webHint: t('MonkeyCode 自动网页登录仅限桌面端；浏览器面板请登录对应官网后粘贴 session。'),
    },
    manualTitle: t('粘贴 session cookie'),
    manualNoteHtml: t('粘贴网页登录态 cookie <code>monkeycode_ai_session</code> 的值（在浏览器里复制整段 Cookie 直接粘贴也行，网关会自己剥出值）。<br>{siteNote}<br>MonkeyCode <b>没有续期接口</b>：session 有效期约 30 天（硬限制），过期后重新登录网页、再粘贴一次即可。', { siteNote }),
    fields: [
      { key: 'session', label: 'session', rows: 3, placeholder: t('monkeycode_ai_session 的值（也可直接粘贴整段 Cookie）') },
      { key: 'imageId', label: t('任务镜像 id'), optional: true, placeholder: t('可选：发起对话必需，留空则由网关从该账号已有任务里自动发现，发现不到时之后手动补') },
      { key: 'name', label: t('备注名'), optional: true, placeholder: t('可选，留空则用上游返回的账号名或账号 id') },
    ],
  }
}

export const MONKEYCODE = monkeycodeForm(
  'monkeycode',
  t('MonkeyCode'),
  t('这里填国内站（monkeycode-ai.com）的账号：请粘贴国内站的 session。国际站（monkeycode-ai.net）是另一家 provider、另一套账号，两边的凭证不通用。'),
)

export const MONKEYCODE_INTL = monkeycodeForm(
  'monkeycode-intl',
  t('MonkeyCode 国际版'),
  t('这里填国际站（monkeycode-ai.net）的账号：请粘贴国际站的 session。国内站（monkeycode-ai.com）是另一家 provider、另一套账号，两边的凭证不通用。'),
)
