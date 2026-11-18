/** 内置订阅动态登录文案；仅翻译固定文本，设备码与账号名作为原样参数保留。 */
import { t } from '../i18n'

export function subscriptionLoginText(value: string): string | undefined {
  const failure = /^(换取 ChatGPT 登录凭证失败|更新 ChatGPT 登录凭证失败|读取 OpenAI 身份验证公钥失败)（HTTP (\d{3})）：([\s\S]+)$/.exec(value)
  if (failure) {
    const params = { status: failure[2], reason: failure[3].split('\n').map(line => subscriptionLoginText(line) || t(line)).join('\n') }
    return failure[1] === '读取 OpenAI 身份验证公钥失败'
      ? t('读取 OpenAI 身份验证公钥失败（HTTP {status}）：{reason}', params)
      : failure[1] === '更新 ChatGPT 登录凭证失败'
        ? t('更新 ChatGPT 登录凭证失败（HTTP {status}）：{reason}', params)
        : t('换取 ChatGPT 登录凭证失败（HTTP {status}）：{reason}', params)
  }
  const deviceCode = /^请在 ChatGPT 官方页面输入验证码：([^\r\n]+)$/.exec(value)
  if (deviceCode) {
    const translated = t('请在 ChatGPT 官方页面输入验证码：{code}', { code: deviceCode[1] })
    return translated === '请在 ChatGPT 官方页面输入验证码：{code}' ? value : translated
  }
  const account = /^重新授权：([^\r\n]*)$/.exec(value)
  if (account) {
    const translated = t('重新授权：{name}', { name: account[1] })
    return translated === '重新授权：{name}' ? value : translated
  }

  switch (value.trim()) {
    case 'OpenAI 返回了安全验证页面，请检查当前网络或联系 OpenAI 支持':
      return t('OpenAI 返回了安全验证页面，请检查当前网络或联系 OpenAI 支持')
    case 'OpenAI 未接受本次请求，请查看原始响应确认原因':
      return t('OpenAI 未接受本次请求，请查看原始响应确认原因')
    case 'OpenAI 不支持当前请求来源的国家或地区，请核对软件的出网设置；浏览器授权与软件请求可能使用不同线路':
      return t('OpenAI 不支持当前请求来源的国家或地区，请核对软件的出网设置；浏览器授权与软件请求可能使用不同线路')
    case 'OpenAI 未接受本应用的套餐授权，请核对账号或工作区的使用权限':
      return t('OpenAI 未接受本应用的套餐授权，请核对账号或工作区的使用权限')
    case 'OpenAI 未接受本应用的注册信息，请重新发起登录':
      return t('OpenAI 未接受本应用的注册信息，请重新发起登录')
    case '当前软件使用直连，不会跟随系统浏览器的代理；如需使用系统代理，可在“备注与代理”中点击“填入系统代理”。':
      return t('当前软件使用直连，不会跟随系统浏览器的代理；如需使用系统代理，可在“备注与代理”中点击“填入系统代理”。')
    case '只关联网关所在机器上已登录的官方 Claude Code；请先自行运行 claude auth login。网关不接收订阅令牌，不替代官方登录。此路线使用 -p，额度计费可能不同于交互终端。':
      return t('只关联网关所在机器上已登录的官方 Claude Code；请先自行运行 claude auth login。网关不接收订阅令牌，不替代官方登录。此路线使用 -p，额度计费可能不同于交互终端。')
    case '关联官方 Claude Code 登录态': return t('关联官方 Claude Code 登录态')
    case '官方登录态位置': return t('官方登录态位置')
    case '当前系统用户的官方登录态': return t('当前系统用户的官方登录态')
    case '我指定的官方配置目录': return t('我指定的官方配置目录')
    case '已登录的 CLAUDE_CONFIG_DIR 绝对路径': return t('已登录的 CLAUDE_CONFIG_DIR 绝对路径')
    case '官方原生 CLI 路径（可选，默认从安装位置查找）':
      return t('官方原生 CLI 路径（可选，默认从安装位置查找）')
    case '授权范围': return t('授权范围')
    case '允许网关使用我选择的官方登录态；客户端工具仅回传执行':
      return t('允许网关使用我选择的官方登录态；客户端工具仅回传执行')
    case '官方 Claude 登录态已核对，正在保存本机引用；没有复制令牌':
      return t('官方 Claude 登录态已核对，正在保存本机引用；没有复制令牌')
    case 'Claude 关联流程已过期或取消，请重新关联':
      return t('Claude 关联流程已过期或取消，请重新关联')

    case '使用 ChatGPT 订阅的 Codex 授权，凭据仅保存到本网关，不修改本机 Codex 登录文件。':
      return t('使用 ChatGPT 订阅的 Codex 授权，凭据仅保存到本网关，不修改本机 Codex 登录文件。')
    case 'Codex 设备授权': return t('Codex 设备授权')
    case '打开官方授权页面并输入本次验证码；需要在 ChatGPT 安全设置或工作区权限中允许设备授权。':
      return t('打开官方授权页面并输入本次验证码；需要在 ChatGPT 安全设置或工作区权限中允许设备授权。')
    case '导入 Codex 登录凭据': return t('导入 Codex 登录凭据')
    case '粘贴你主动提供的 Codex auth.json；请勿与其他正在刷新的客户端长期共用同一份凭据。这里不是 OpenAI API Key。':
      return t('粘贴你主动提供的 Codex auth.json；请勿与其他正在刷新的客户端长期共用同一份凭据。这里不是 OpenAI API Key。')
    case 'Codex 凭据 JSON': return t('Codex 凭据 JSON')
    case '粘贴 auth.json 的完整内容': return t('粘贴 auth.json 的完整内容')
    case '验证码 15 分钟有效。仅继续你自己在本网关发起的授权；完成后返回这里。':
      return t('验证码 15 分钟有效。仅继续你自己在本网关发起的授权；完成后返回这里。')
    case 'Codex 设备验证码已过期，请重新发起授权':
      return t('Codex 设备验证码已过期，请重新发起授权')
    case 'Codex 登录已取消': return t('Codex 登录已取消')
    case 'Codex 登录已取消或过期，请重新授权': return t('Codex 登录已取消或过期，请重新授权')
    case 'Codex 登录流程不存在、已取消或已过期': return t('Codex 登录流程不存在、已取消或已过期')

    case '添加新的 ChatGPT 账号或工作区': return t('添加新的 ChatGPT 账号或工作区')
    case '使用 ChatGPT 登录': return t('使用 ChatGPT 登录')
    case '在系统浏览器授权。如果回调页面无法打开，请粘贴地址栏里的完整回调地址。':
      return t('在系统浏览器授权。如果回调页面无法打开，请粘贴地址栏里的完整回调地址。')
    case '登录账号': return t('登录账号')
    case '请在系统浏览器完成 ChatGPT 授权。远程访问时，复制无法打开的回调页面完整地址并粘贴回来。':
      return t('请在系统浏览器完成 ChatGPT 授权。远程访问时，复制无法打开的回调页面完整地址并粘贴回来。')
    case '回调地址与本次 ChatGPT 登录不匹配，请重新复制完整地址':
      return t('回调地址与本次 ChatGPT 登录不匹配，请重新复制完整地址')
    case '已收到浏览器授权，正在向 OpenAI 换取登录凭证…':
      return t('已收到浏览器授权，正在向 OpenAI 换取登录凭证…')
    case '正在验证 ChatGPT 授权结果…': return t('正在验证 ChatGPT 授权结果…')
    case '等待浏览器完成 ChatGPT 授权': return t('等待浏览器完成 ChatGPT 授权')
    case 'ChatGPT 登录已取消或超时，请重新发起': return t('ChatGPT 登录已取消或超时，请重新发起')
    case 'ChatGPT 登录任务不存在或已过期，请重新发起':
      return t('ChatGPT 登录任务不存在或已过期，请重新发起')
    default: return undefined
  }
}
