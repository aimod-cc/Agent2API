export type TaskFieldType =
  | 'text'
  | 'password'
  | 'textarea'
  | 'number'
  | 'switch'
  | 'select'
  | 'segmented'
  | 'multiselect'

export type TaskFieldOption = {
  value: string
  label: string
}

export type TaskFieldCondition = {
  key: string
  equals: string
}

export type TaskFieldDef = {
  key: string
  label: string
  type: TaskFieldType
  placeholder?: string
  description?: string
  required?: boolean
  secret?: boolean
  group?: 'basic' | 'advanced'
  defaultValue?: string | number | boolean | string[] | null
  min?: number | null
  max?: number | null
  unit?: string
  options?: TaskFieldOption[]
  when?: TaskFieldCondition | null
}

export type TaskActionDef = {
  id: string
  label: string
  variant: 'default' | 'outline' | 'destructive'
  placement: 'footer' | 'inline'
  attachToField?: string | null
  confirm?: string | null
  cooldownSeconds?: number | null
}

export type TaskStatusFieldDef = {
  key: string
  label: string
}

export type TaskPlugin = {
  id: string
  name: string
  description: string
  version: string
  source: 'official' | 'local' | 'third-party'
  signed: boolean
  enabled: boolean
  icon: string
  defaultCron: string
  defaultTimezone: string
  defaultTimeout: number
  domains: string[]
  declaresNotify: boolean
  notifyAllowed: boolean
  fields: TaskFieldDef[]
  actions: TaskActionDef[]
  statusFields: TaskStatusFieldDef[]
  scriptBody: string
  createdAt: number
  updatedAt: number
}

export type TaskPluginStoreItem = {
  id: string
  name: string
  description: string
  version: string | null
  icon: string | null
  downloads?: number | null
  homepage: string | null
  page: string | null
  notes: string | null
  publishedAt: string | null
  prerelease: boolean
  requireSigned: boolean
  installed: boolean
  updatable: boolean
  asset: { target: string; url: string; size: number; sha256?: string | null } | null
}

export type TaskPluginUpdate = {
  id: string
  current: string
  latest: string
  notes?: string | null
  publishedAt?: string | null
  prerelease?: boolean
}

export type TaskPluginsPayload = {
  plugins: TaskPlugin[]
  store: TaskPluginStoreItem[]
  updates: TaskPluginUpdate[]
}

export type TaskProxyOption = {
  id: string
  name: string
  protocol: string
  host: string
  port: number
}

export type TaskJob = {
  id: string
  pluginId: string
  name: string
  cronExpression: string
  timezone: string
  timeoutSeconds: number
  proxyId: string
  enabled: boolean
  values: Record<string, unknown>
  savedSecrets: Record<string, boolean>
  statusSummary: Record<string, string>
  nextRunAt: number | null
  lastRunAt: number | null
  lastStatus: 'running' | 'success' | 'failed' | null
  lastDurationMs: number | null
  lastError: string | null
  createdAt: number
  updatedAt: number
}

export type TaskJobsPayload = {
  jobs: TaskJob[]
  proxies: TaskProxyOption[]
}

export type TaskJobRun = {
  id: number
  jobId: string
  triggerType: 'schedule' | 'manual' | 'action'
  triggerLabel: string
  status: 'running' | 'success' | 'failed'
  startedAt: number
  finishedAt: number | null
  durationMs: number | null
  output: string
  error: string | null
}

export type SandboxOutcome = {
  success: boolean
  durationMs: number
  output: string
  error?: string | null
  message?: string | null
  level?: 'success' | 'info' | 'warning' | 'error' | null
  qrText?: string | null
  qrImageDataUrl?: string | null
  imageDataUrl?: string | null
  statusPatch?: Record<string, string> | null
  patchForm?: Record<string, unknown> | null
}

export type TaskApiRequest = <T>(method: string, path: string, body?: unknown) => Promise<T>
export type TaskTranslate = (key: string, params?: Record<string, string | number>) => string

export const TASK_TIMEZONES = [
  'Asia/Shanghai',
  'Asia/Tokyo',
  'Asia/Singapore',
  'UTC',
  'America/Los_Angeles',
  'America/New_York',
  'Europe/London',
]

export const TASK_CRON_PRESETS: Array<{ label: string; cron: string }> = [
  { label: '每天 09:00', cron: '0 9 * * *' },
  { label: '每天 00:05', cron: '5 0 * * *' },
  { label: '每小时', cron: '0 * * * *' },
  { label: '每 15 分钟', cron: '*/15 * * * *' },
]

export function serializeTaskPluginFile(plugin: TaskPlugin): string {
  const header = {
    id: plugin.id,
    name: plugin.name,
    version: plugin.version,
    description: plugin.description,
    icon: plugin.icon,
    defaultCron: plugin.defaultCron,
    defaultTimezone: plugin.defaultTimezone,
    defaultTimeout: plugin.defaultTimeout,
    permissions: {
      domains: plugin.domains,
      notify: plugin.declaresNotify,
    },
    fields: plugin.fields,
    actions: plugin.actions,
    statusFields: plugin.statusFields,
  }
  return `/* @agent2api-task-plugin\n${JSON.stringify(header, null, 2)}\n*/\n\n${plugin.scriptBody.trimStart()}`
}

export function parseTaskPluginFileLocal(
  raw: string,
  fallback: TaskPlugin
): { plugin: TaskPlugin; error?: string } {
  const match = raw.match(/\/\*\s*@agent2api-task-plugin\s*([\s\S]*?)\*\//)
  if (!match) {
    return {
      plugin: { ...fallback, scriptBody: raw },
      error: '未找到 /* @agent2api-task-plugin ... */ 头部声明块',
    }
  }
  try {
    const meta = JSON.parse(match[1].trim()) as Record<string, unknown>
    const body = raw.slice((match.index ?? 0) + match[0].length).trimStart()
    const perms = (meta.permissions && typeof meta.permissions === 'object'
      ? meta.permissions
      : {}) as Record<string, unknown>
    return {
      plugin: {
        ...fallback,
        id: typeof meta.id === 'string' && meta.id.trim() ? meta.id.trim().toLowerCase() : fallback.id,
        name: typeof meta.name === 'string' && meta.name.trim() ? meta.name.trim() : fallback.name,
        version: typeof meta.version === 'string' && meta.version.trim() ? meta.version.trim() : fallback.version,
        description: typeof meta.description === 'string' ? meta.description : fallback.description,
        icon: typeof meta.icon === 'string' ? meta.icon : fallback.icon,
        defaultCron: typeof meta.defaultCron === 'string' ? meta.defaultCron : fallback.defaultCron,
        defaultTimezone: typeof meta.defaultTimezone === 'string' ? meta.defaultTimezone : fallback.defaultTimezone,
        defaultTimeout: typeof meta.defaultTimeout === 'number' ? meta.defaultTimeout : fallback.defaultTimeout,
        domains: Array.isArray(perms.domains)
          ? perms.domains.filter((d): d is string => typeof d === 'string')
          : fallback.domains,
        declaresNotify: typeof perms.notify === 'boolean' ? perms.notify : fallback.declaresNotify,
        fields: Array.isArray(meta.fields) ? (meta.fields as TaskFieldDef[]) : fallback.fields,
        actions: Array.isArray(meta.actions) ? (meta.actions as TaskActionDef[]) : fallback.actions,
        statusFields: Array.isArray(meta.statusFields)
          ? (meta.statusFields as TaskStatusFieldDef[])
          : fallback.statusFields,
        scriptBody: body,
      },
    }
  } catch (cause) {
    return {
      plugin: fallback,
      error: cause instanceof Error ? cause.message : '头部 JSON 解析失败',
    }
  }
}

export function defaultValuesForPlugin(plugin: TaskPlugin): Record<string, unknown> {
  const values: Record<string, unknown> = {}
  for (const field of plugin.fields) {
    if (field.defaultValue !== undefined && field.defaultValue !== null) {
      values[field.key] = field.defaultValue
    } else if (field.type === 'switch') {
      values[field.key] = false
    } else if (field.type === 'multiselect') {
      values[field.key] = []
    } else if (field.type === 'segmented' && field.options?.length) {
      values[field.key] = field.options[0].value
    }
  }
  return values
}

export function templateLoginCheckin(): TaskPlugin {
  return {
    id: `login-checkin-${Date.now().toString().slice(-4)}`,
    name: '账密登录与定时签到',
    description: '先通过表单输入账号密码点击登录保存凭证，随后按计划每日自动执行签到并在失败时发通知。',
    version: '0.1.0',
    source: 'local',
    signed: false,
    enabled: true,
    icon: '',
    defaultCron: '0 9 * * *',
    defaultTimezone: 'Asia/Shanghai',
    defaultTimeout: 30,
    domains: ['httpbin.org'],
    declaresNotify: true,
    notifyAllowed: true,
    fields: [
      { key: 'account', label: '账号 / 邮箱', type: 'text', placeholder: '输入登录账号', required: true, group: 'basic' },
      { key: 'password', label: '登录密码', type: 'password', secret: true, placeholder: '加密保存，不随插件导出', required: true, group: 'basic' },
      { key: 'notifyOnSuccess', label: '签到成功后发送通知', type: 'switch', defaultValue: false, group: 'advanced' },
    ],
    actions: [
      { id: 'login', label: '登录并保存凭证', variant: 'default', placement: 'footer' },
      { id: 'check', label: '检测当前状态', variant: 'outline', placement: 'footer' },
      { id: 'logout', label: '清除登录态', variant: 'destructive', placement: 'footer', confirm: '确认清除当前任务保存的加密凭证？' },
    ],
    statusFields: [
      { key: 'authState', label: '登录状态' },
      { key: 'lastCheckin', label: '最近签到' },
    ],
    scriptBody: `// 按钮点击入口：处理登录、检测状态、退出登录
export async function onAction(action, ctx) {
  if (action === 'login') {
    if (!ctx.form.account || !ctx.form.password) {
      throw new Error('请先填写账号和密码');
    }
    const res = await ctx.http.postJson('https://httpbin.org/post', {
      account: ctx.form.account,
      digest: ctx.crypto.sha256(ctx.form.password),
    });
    const token = ctx.crypto.sha256(ctx.form.account + ':' + Date.now()).slice(0, 24);
    await ctx.storage.set('token', token);
    ctx.log('登录接口响应成功，来源 IP：' + (res.origin || '未知'));
    return {
      message: '登录成功，凭证已加密保存',
      level: 'success',
      status: { authState: '已登录', lastCheckin: '尚未签到' },
    };
  }
  if (action === 'check') {
    const token = await ctx.storage.get('token');
    return {
      message: token ? '当前存在有效凭证' : '尚未登录，请先点击登录',
      level: token ? 'success' : 'warning',
      status: { authState: token ? '已登录' : '未登录' },
    };
  }
  if (action === 'logout') {
    await ctx.storage.delete('token');
    return {
      message: '已清除登录凭证',
      level: 'info',
      status: { authState: '未登录' },
    };
  }
}

// 定时任务主入口：读取加密凭证执行每日签到
export async function onSchedule(ctx) {
  const token = await ctx.storage.get('token');
  if (!token) {
    await ctx.notify.send('签到提醒', '凭证缺失，请前往重新登录');
    throw new Error('尚未登录，请先配置账号密码并点击登录');
  }
  const res = await ctx.http.get('https://httpbin.org/get', {
    headers: { Authorization: 'Bearer ' + token },
  });
  ctx.log('定时签到完成，HTTP ' + res.status);
  if (ctx.form.notifyOnSuccess) {
    await ctx.notify.send('定时签到成功', '账号 ' + (ctx.form.account || '') + ' 今日已完成签到');
  }
  return {
    message: '今日签到完成',
    status: { authState: '已登录', lastCheckin: new Date().toLocaleTimeString() },
  };
}`,
    createdAt: 0,
    updatedAt: 0,
  }
}

export function templateWebWatch(): TaskPlugin {
  return {
    id: `web-watch-${Date.now().toString().slice(-4)}`,
    name: '网页变动监控通知',
    description: '定时拉取目标页面内容，匹配关键词或内容变化后自动推送系统通知。',
    version: '0.1.0',
    source: 'local',
    signed: false,
    enabled: true,
    icon: '',
    defaultCron: '*/30 * * * *',
    defaultTimezone: 'Asia/Shanghai',
    defaultTimeout: 30,
    domains: ['httpbin.org'],
    declaresNotify: true,
    notifyAllowed: true,
    fields: [
      { key: 'url', label: '目标地址', type: 'text', placeholder: 'https://httpbin.org/json', defaultValue: 'https://httpbin.org/json', required: true, group: 'basic' },
      { key: 'keyword', label: '关注关键词', type: 'text', placeholder: '输入要检测的文字', defaultValue: 'slideshow', required: true, group: 'basic' },
      { key: 'onlyOnChange', label: '仅在状态变化时推送', type: 'switch', defaultValue: true, group: 'basic' },
    ],
    actions: [
      { id: 'test_fetch', label: '立即抓取测试', variant: 'default', placement: 'footer' },
      { id: 'test_notify', label: '发送测试通知', variant: 'outline', placement: 'footer' },
    ],
    statusFields: [
      { key: 'matchState', label: '检测结果' },
    ],
    scriptBody: `export async function onAction(action, ctx) {
  if (action === 'test_notify') {
    const res = await ctx.notify.send('监控通道测试', '监控地址：' + ctx.form.url);
    return { message: res.sent ? '测试通知已发送' : '未发送通知：' + (res.reason || '请检查系统通知配置') };
  }
  return onSchedule(ctx);
}

export async function onSchedule(ctx) {
  const text = await ctx.http.getText(ctx.form.url);
  const hit = text.includes(ctx.form.keyword);
  const prev = await ctx.storage.get('lastHit');
  ctx.log('抓取长度 ' + text.length + ' 字符，命中关键词：' + (hit ? '是' : '否'));
  if (hit && (!ctx.form.onlyOnChange || prev !== 'yes')) {
    await ctx.notify.send('网页变动提醒', '页面命中关键词：' + ctx.form.keyword);
  }
  await ctx.storage.set('lastHit', hit ? 'yes' : 'no');
  return {
    message: hit ? '已命中关键词' : '暂未命中关键词',
    status: { matchState: hit ? '已命中' : '未命中' },
  };
}`,
    createdAt: 0,
    updatedAt: 0,
  }
}
