import { t } from '../i18n'

export function pluginLoginText(value: string): string {
  switch (value.trim()) {
    case 'ACP 的本机进程仍使用原来的代理。请先在插件页停用再启用 ACP 插件，再使用新的代理。':
      return t('ACP 的本机进程仍使用原来的代理。请先在插件页停用再启用 ACP 插件，再使用新的代理。')
    case 'ACP 代理配置无效，请重新选择代理。': return t('ACP 代理配置无效，请重新选择代理。')
    case '此提供商已有登录任务，请先完成或取消。': return t('此提供商已有登录任务，请先完成或取消。')
    case 'ACP 尚未返回可用的会话模型目录，请检查官方 CLI 登录状态后重试。':
      return t('ACP 尚未返回可用的会话模型目录，请检查官方 CLI 登录状态后重试。')
    case 'ACP 会话失败时不自动重发，也不切换到其他接入方式。':
      return t('ACP 会话失败时不自动重发，也不切换到其他接入方式。')
    case 'Google 授权（ACP）': return t('Google 授权（ACP）')
    case 'ACP 使用网关电脑上的官方 agy CLI，不是当前手机或浏览器所在设备。请先安装 agy 1.5.0 或更新版本：https://antigravity.google/docs/cli。程序不在默认位置时，可在网关启动环境设置 AGY_PATH。':
      return t('ACP 使用网关电脑上的官方 agy CLI，不是当前手机或浏览器所在设备。请先安装 agy 1.5.0 或更新版本：https://antigravity.google/docs/cli。程序不在默认位置时，可在网关启动环境设置 AGY_PATH。')
    case '点击登录后，在 Google 页面授权，再原样粘贴授权码或完整页面地址；已登录的 agy 会直接连接。Google 凭证由官方客户端保管，Agent2API 只保存本机连接标记。同一网关仅使用 agy 当前登录的一个账号，不支持复制为多个独立账号。':
      return t('点击登录后，在 Google 页面授权，再原样粘贴授权码或完整页面地址；已登录的 agy 会直接连接。Google 凭证由官方客户端保管，Agent2API 只保存本机连接标记。同一网关仅使用 agy 当前登录的一个账号，不支持复制为多个独立账号。')
    case 'Browser sign-in (ink credits + creates an API key)':
      return t('浏览器登录（墨点额度 + 自动创建 API Key）')
    case 'Browser sign-in + reuse an existing API key':
      return t('浏览器登录（复用已有 API Key）')
    case 'API key only (models, no ink-credit card)':
      return t('仅 API Key（模型调用，不含墨点额度）')
    case 'API key + console token (models + ink-credit card)':
      return t('API Key + 控制台令牌（模型调用 + 墨点额度）')
    case 'Leave empty to sign in in a browser (paste an existing key to reuse it)':
      return t('已有 API Key（留空则在浏览器登录后自动创建）')
    case 'TokenPlan API key (console → 科研模型 → API Key)':
      return t('TokenPlan API Key（控制台 → 科研模型 → API Key）')
    case 'TokenPlan API key':
      return 'TokenPlan API Key'
    case 'Console uaa-token (paste just the token value, or the full "uaa-token=…" cookie entry)':
      return t('控制台 uaa-token（粘贴令牌值或完整的 uaa-token=… Cookie 项）')
    case 'A browser window opens on the InkStone console. Sign in there (password, SMS or QR code); the plugin reads the completed sign-in by itself and closes the window. You do not need to close it. After the sign-in the plugin creates an ink-* API key on your account — the plaintext is shown once, and the plugin keeps it for you.':
      return t('请在插件打开的端砚控制台窗口中登录（支持密码、短信或扫码）。插件会自动读取登录结果并关闭窗口，无需手动关闭；随后按所选方式创建或复用 API Key。新建密钥的明文只显示一次，由插件保存。')
    case 'A browser window opens on the InkStone console. Sign in there (password, SMS or QR code), then quit that browser yourself — Cmd+Q on macOS (closing its last window is not enough), or close its last window on Linux. The plugin then reads the session from the profile and finishes. After the sign-in the plugin creates an ink-* API key on your account — the plaintext is shown once, and the plugin keeps it for you.':
      return t('请在插件打开的端砚控制台窗口中登录（支持密码、短信或扫码），然后退出该浏览器：macOS 请按 Cmd+Q（只关闭窗口不够），Linux 请关闭最后一个窗口。插件读取登录结果后会继续处理，并按所选方式创建或复用 API Key。新建密钥的明文只显示一次，由插件保存。')
    case '浏览器登录（登录一次，凭证全自动）':
      return t('插件专用 Chrome 登录（自动读取凭据）')
    case '已弹出专用 Chrome 窗口：请在里面登录移动云（yun.139.com，网页支持多种登录方式），登录成功后本流程自动继续并收走全部凭证':
      return t('请在插件打开的专用 Chrome 窗口中登录移动云（yun.139.com）。登录成功后会自动读取并验证凭据，随后继续添加账号。')
    case '等待插件完成登录':
      return t('等待插件完成登录')
    case '等待浏览器完成登录':
      return t('等待浏览器完成登录')
    case '已收到浏览器授权，正在获取账号凭据…':
      return t('已收到浏览器授权，正在获取账号凭据…')
    case '浏览器显示登录完成后，网关还需联网获取账号凭据。若浏览器或 Magpie 使用系统代理，请先在下方点击「填入系统代理」；选择直连不会跟随系统代理。':
      return t('浏览器显示登录完成后，网关还需联网获取账号凭据。若浏览器或 Magpie 使用系统代理，请先在下方点击「填入系统代理」；选择直连不会跟随系统代理。')
    case '浏览器授权已收到，但网关直连 DeepAI 失败。请在登录代理中填入系统代理或选择可用代理后重新登录。':
      return t('浏览器授权已收到，但网关直连 DeepAI 失败。请在登录代理中填入系统代理或选择可用代理后重新登录。')
    case '浏览器授权已收到，但通过所选代理获取账号凭据失败。请检查该代理后重新登录。':
      return t('浏览器授权已收到，但通过所选代理获取账号凭据失败。请检查该代理后重新登录。')
    default:
      return t(value)
  }
}

export function pluginBrowserInstructions(provider: string): string {
  if (provider === 'intern-inkstone') {
    return t('端砚由插件在网关电脑上启动独立的 Edge、Chrome 或 Chromium 窗口，不使用系统默认浏览器或应用内嵌窗口。')
  }
  if (provider === 'mclaw') {
    return t('MClaw 由插件在网关电脑上启动专用 Chrome 窗口，独立登录环境也用于自动续期，不使用系统默认浏览器或应用内嵌窗口。')
  }
  return t('插件会在网关电脑上启动独立浏览器并读取登录结果，不使用系统默认浏览器。')
}

export function mergeLoginInstructions(...values: Array<string | null | undefined>): string {
  const seen = new Set<string>()
  return values.flatMap(value => value?.split(/\r?\n/) || []).map(line => line.trim()).filter(line => {
    const key = line.replace(/\s+/g, ' ')
    if (!key || seen.has(key)) return false
    seen.add(key)
    return true
  }).join('\n')
}
