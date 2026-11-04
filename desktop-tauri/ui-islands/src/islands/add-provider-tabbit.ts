import type { ProviderConfig } from './add-account-configs'
import { t } from '../i18n'

/**
 * Tabbit 浏览器（国内站 web.tabbit.com）。
 *
 * ── 为什么归 Web 段而不是 Agent 段 ──────────────────────────
 * 它的「客户端」是一个 Chromium 浏览器，登录态是网页 Cookie 里的 JWT；
 * 上游是网页对话产品，没有原生工具调用。与智谱清言 / DeepSeek 网页同一形态。
 *
 * ── 添加账号为什么不提供「网页登录」──────────────────────────
 * 国际站的 Google OAuth 只把授权结果投递给它自己登记的 redirect_uri，网关
 * 接不到；国内站的手机号登录在灰度里。所以两个站点都走「粘贴已有登录态」。
 *
 * ── 为什么有两家 ────────────────────────────────────────────
 * 两站签名 key 不同、账号不通用、登录方式也不同（国内站有手机号登录），
 * 按仓库的「两套账号就拆家」规则拆成 `tabbit` / `tabbit-intl`。
 */
export const TABBIT: ProviderConfig = {
  provider: 'tabbit',
  label: t('Tabbit 国内版'),
  desktop: false,
  smsLogin: {
    noteHtml: t('用绑定的手机号登录：点「获取验证码」，收到短信后填入下方并点「登录并添加」。这是国内站官方唯一的登录方式。'),
  },
  manualTitle: t('粘贴网页登录凭证'),
  manualNote: t('在 web.tabbit.com 登录后，从开发者工具 Application → Cookies 复制 token 与 user_id，或直接粘贴该站点请求的 Copy as cURL 整段。登录态有效期约 7 天，上游没有续期接口，过期后重新导入即可。Cookie 与 JWT 至少填一项。'),
  fields: [
    {
      key: 'cookie',
      label: t('网页 Cookie'),
      rows: 3,
      optional: true,
      placeholder: t('token=…; user_id=…（也可粘贴 Copy as cURL 整段）'),
    },
    {
      key: 'token',
      label: t('登录 JWT'),
      rows: 3,
      optional: true,
      placeholder: t('只拿到 JWT 时填这里；Cookie 里有 token 可留空'),
    },
    {
      key: 'unlockPro',
      label: t('解锁 Premium'),
      optional: true,
      placeholder: t('默认开启；填 0 关闭（发送「已设为默认浏览器」标记）'),
    },
    {
      key: 'clientVersion',
      label: t('客户端版本'),
      optional: true,
      placeholder: t('留空使用 1.17.16(10117016)'),
    },
    {
      key: 'name',
      label: t('备注名'),
      optional: true,
      placeholder: t('可选，留空自动生成'),
    },
  ],
  fileImport: {
    accept: '.json,.txt,text/plain,application/json',
    note: t('选择包含 Tabbit Cookie / JWT 的 JSON 或文本文件；也可以直接粘贴到上方表单。'),
  },
}

/**
 * Tabbit 浏览器（国际站 web.tabbit.ai）。
 *
 * 国际站只能用「已有登录态」导入：Google 授权回调回不到网关，手机号登录
 * 在上游灰度里被 403 拒绝（参考实现实测）。所以这里没有网页登录入口。
 */
export const TABBIT_INTL: ProviderConfig = {
  provider: 'tabbit-intl',
  label: t('Tabbit 国际版'),
  desktop: false,
  manualTitle: t('粘贴网页登录凭证'),
  manualNote: t('在 web.tabbit.ai 登录后，从开发者工具 Application → Cookies 复制 token 与 user_id，或直接粘贴该站点请求的 Copy as cURL 整段。国际站没有可复刻的网页授权回调，手机号登录也处在上游灰度里，所以只能导入已有登录态。登录态有效期约 7 天，过期后重新导入。Cookie 与 JWT 至少填一项。'),
  fields: [
    {
      key: 'cookie',
      label: t('网页 Cookie'),
      rows: 3,
      optional: true,
      placeholder: t('token=…; user_id=…（也可粘贴 Copy as cURL 整段）'),
    },
    {
      key: 'token',
      label: t('登录 JWT'),
      rows: 3,
      optional: true,
      placeholder: t('只拿到 JWT 时填这里；Cookie 里有 token 可留空'),
    },
    {
      key: 'unlockPro',
      label: t('解锁 Premium'),
      optional: true,
      placeholder: t('默认开启；填 0 关闭（发送「已设为默认浏览器」标记）'),
    },
    {
      key: 'clientVersion',
      label: t('客户端版本'),
      optional: true,
      placeholder: t('留空使用 1.17.16(10117016)'),
    },
    {
      key: 'name',
      label: t('备注名'),
      optional: true,
      placeholder: t('可选，留空自动生成'),
    },
  ],
  fileImport: {
    accept: '.json,.txt,text/plain,application/json',
    note: t('选择包含 Tabbit Cookie / JWT 的 JSON 或文本文件；也可以直接粘贴到上方表单。'),
  },
}
