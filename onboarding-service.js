// 新手任务服务（mock-first 双层）。
//
// backend = 'mock'（默认）：读写 userData/onboarding-tasks.json，本地持久化 +
//   本地 earned 累计（不接真实积分服务），renderer 完全感觉是真接口。
// backend = 'real'：转发到 GET/POST /api/v1/onboarding/tasks*，按 v2 契约统一
//   apiresp.Envelope（{code,desc,trace_id,data}）拆包：code==='000000' 取 data，
//   否则抛错（100002→鉴权失效）。renderer / IPC 协议零修改。
//
// 单一切换点：构造参数 backend（由 main.js 读 process.env.ONBOARDING_BACKEND 注入）。
// 契约见 docs/API_CONTRACT_ONBOARDING_TASKS_v2_BACKEND_PROPOSAL.md（v2，现行）。

import fs from 'node:fs'
import path from 'node:path'

// 内置 key→points 映射，与 src/lib/onboarding/task-registry.js 等价。
// 二者一致性由 electron/__tests__/onboarding-service.test.js 交叉断言防漂移
//（避免主进程 import 渲染进程模块）。
export const TASK_POINTS = {
  first_message: 500,
  pick_skill: 1000,
  generate_ppt: 1500,
  set_schedule: 1000,
  install_skill: 1500,
  configure_remote: 1000,
  create_soul: 1500,
  share_soul: 2000,
}

const STORE_FILENAME = 'onboarding-tasks.json'
const STORE_VERSION = 1

export class OnboardingService {
  constructor({
    userDataDir,
    registry,
    backend = 'mock',
    getConfig,
    getAuthSession,
  } = {}) {
    this.userDataDir = userDataDir
    this.backend = backend === 'real' ? 'real' : 'mock'
    // registry 可注入（测试用）；默认用内置 TASK_POINTS。
    this.points = registry?.points || TASK_POINTS
    this.keys = Object.keys(this.points)
    this.total = Object.values(this.points).reduce((sum, p) => sum + p, 0)
    // real 模式依赖（与 PointsService/SkillStoreService 同源注入）：
    // baseUrl 取自 pointsConfig，token 取自缓存的登录会话。mock 模式不需要。
    this.getConfig = typeof getConfig === 'function' ? getConfig : null
    this.getAuthSession =
      typeof getAuthSession === 'function' ? getAuthSession : null
  }

  get filePath() {
    return path.join(this.userDataDir, STORE_FILENAME)
  }

  // 读持久化的 tasks 映射；文件缺失 → 空；损坏 → 备份后空（不静默清除用户进度）。
  _readTasks() {
    let raw
    try {
      raw = fs.readFileSync(this.filePath, 'utf-8')
    } catch {
      return {}
    }
    try {
      const parsed = JSON.parse(raw)
      const tasks = parsed?.tasks
      return tasks && typeof tasks === 'object' ? tasks : {}
    } catch {
      const backup = `${this.filePath}.bak.${Date.now()}`
      try {
        fs.renameSync(this.filePath, backup)
      } catch {
        /* 备份失败也不阻塞，继续以空状态运行 */
      }
      return {}
    }
  }

  // 原子写：写 *.tmp → rename 替换，避免半写损坏。
  // ⚠️ 不变量：markComplete 的 _readTasks→_writeTasks 之间禁止插入 await——
  // 二者均为同步 fs 调用，单线程下保证单次 markComplete 原子；一旦中间出现
  // await，多个并发 markComplete 会交错读到旧态互相覆盖丢 key。
  _writeTasks(tasks) {
    const payload = {
      version: STORE_VERSION,
      tasks,
      earned: this._computeEarned(tasks),
      updatedAt: new Date().toISOString(),
    }
    const tmpPath = `${this.filePath}.${process.pid}.tmp`
    fs.writeFileSync(tmpPath, JSON.stringify(payload, null, 2))
    fs.renameSync(tmpPath, this.filePath)
  }

  // earned 始终由 registry + 已完成 tasks 现算（不信任文件里的 earned 字段），
  // 自动兼容 registry 改 points / 文件残留多余 key 的情况。
  _computeEarned(tasks) {
    return this.keys.reduce(
      (sum, key) => (tasks[key] === true ? sum + this.points[key] : sum),
      0,
    )
  }

  // 以 registry 为准归一：缺的 key 补 false，文件里多余的 key 忽略。
  _normalize(tasks) {
    const normalized = {}
    for (const key of this.keys) {
      normalized[key] = tasks[key] === true
    }
    return normalized
  }

  // real 模式 HTTP + envelope 拆包（复用 points-service 模式）：
  // 命中 code==='000000' 返回 envelope.data；否则抛错（100002 鉴权失效保留 desc）。
  async _request(requestPath, { method = 'GET', body = null } = {}) {
    const baseUrl = String(this.getConfig?.()?.baseUrl || '').trim()
    if (!baseUrl) {
      throw new Error('新手任务服务未配置 baseUrl')
    }
    const token = String(this.getAuthSession?.() || '').trim()
    if (!token) {
      throw new Error('缺少接口 token，请先完成用户登录')
    }

    const headers = { token }
    if (body !== null) {
      headers['Content-Type'] = 'application/json'
    }

    const controller = new AbortController()
    const timeoutId = setTimeout(() => controller.abort(), 30000)
    let response
    try {
      response = await fetch(`${baseUrl}${requestPath}`, {
        method,
        headers,
        body: body !== null ? JSON.stringify(body) : undefined,
        signal: controller.signal,
      })
    } catch (error) {
      throw new Error(
        error?.name === 'AbortError'
          ? `${requestPath} 请求超时`
          : `${requestPath} 调用失败: ${error?.message || '网络错误'}`,
      )
    } finally {
      clearTimeout(timeoutId)
    }

    const text = await response.text()
    let payload = null
    if (text) {
      try {
        payload = JSON.parse(text)
      } catch {
        throw new Error(`${requestPath} 返回数据不是合法 JSON`)
      }
    }
    if (!response.ok) {
      throw new Error(
        payload?.desc || `${requestPath} 调用失败: HTTP ${response.status}`,
      )
    }
    // v2：业务成败由 envelope.code 表达（非 HTTP 状态码），非 000000 一律抛错。
    if (payload?.code && payload.code !== '000000') {
      throw new Error(payload.desc || `业务错误 ${payload.code}`)
    }
    return payload?.data
  }

  async getTasks() {
    if (this.backend === 'real') {
      const d = await this._request('/api/v1/onboarding/tasks', {
        method: 'GET',
      })
      // §6.1：earned 不信任后端回传，按注册中心 + 归一后 tasks 现算（与 mock 对称）。
      const tasks = this._normalize(d?.tasks || {})
      return {
        tasks,
        earned: this._computeEarned(tasks),
        total: Number.isFinite(d?.total) ? d.total : this.total,
      }
    }
    const tasks = this._normalize(this._readTasks())
    return {
      tasks,
      earned: this._computeEarned(tasks),
      total: this.total,
    }
  }

  async markComplete(payload) {
    const key = payload?.key
    // v2：未知 key 不再返回 {ok:false}，统一抛错经 IPC success:false 上报
    //（与 real envelope code 100001 对称）；real 模式下亦在发请求前本地拦截。
    if (typeof key !== 'string' || !(key in this.points)) {
      throw new Error(`未知的 task key: ${String(key)}`)
    }

    if (this.backend === 'real') {
      const d = await this._request('/api/v1/onboarding/tasks/complete', {
        method: 'POST',
        body: { key },
      })
      return {
        alreadyCompleted: d?.alreadyCompleted === true,
        balance: d?.balance,
      }
    }

    const stored = this._readTasks()
    if (stored[key] === true) {
      return {
        alreadyCompleted: true,
        balance: this._computeEarned(this._normalize(stored)),
      }
    }

    const next = { ...this._normalize(stored), [key]: true }
    this._writeTasks(next)
    return {
      alreadyCompleted: false,
      balance: this._computeEarned(next),
    }
  }
}
