import { ensureChannelAttribution } from './channel/channel-store.js'

function normalizeRecordType(value) {
  const normalized = String(value || '').trim().toLowerCase();
  if (normalized === 'earned' || normalized === 'consumed' || normalized === 'all') {
    return normalized;
  }
  return 'all';
}

function normalizeDirection(value) {
  const normalized = String(value || '').trim().toLowerCase();
  if (normalized === 'debit' || normalized === 'credit') {
    return normalized;
  }
  return 'debit';
}

/**
 * 归一化积分记录聚合粒度。
 *
 * - `chat` / `msg`：对话型消耗，按会话 / 按消息聚合（老口径）
 * - `purpose`：功能型消耗，按 request_purpose + source_endpoint 聚合。
 *   知识库识图/摘要/检索、输入助手补全/纠错、记忆、技能、搜索、生图这类请求
 *   没有 chatId/msgId，只能靠这一轴展示，否则就是「消耗了但看不见」
 * - `turn`：轮次消耗，按 turn_id 聚合，展示单次用户操作的总消耗
 */
function normalizeGranularity(value) {
  const normalized = String(value || '').trim().toLowerCase();
  if (normalized === 'chat' || normalized === 'msg' || normalized === 'purpose' || normalized === 'turn') {
    return normalized;
  }
  return 'chat';
}

function buildQuery(params) {
  const searchParams = new URLSearchParams();
  Object.entries(params).forEach(([key, value]) => {
    if (value === undefined || value === null || value === '') {
      return;
    }
    searchParams.set(key, String(value));
  });
  return searchParams.toString();
}

/**
 * 校验宠物打工接口使用的服务端业务日。
 *
 * @param {unknown} value 待校验的日期值。
 * @returns {boolean} 是否为有效的 `YYYY-MM-DD` 日历日期。
 */
function isBusinessDate(value) {
  const text = String(value || '').trim();
  if (!/^\d{4}-\d{2}-\d{2}$/.test(text)) {
    return false;
  }

  const parsedTimestamp = Date.parse(`${text}T00:00:00Z`);
  if (!Number.isFinite(parsedTimestamp)) {
    return false;
  }

  return new Date(parsedTimestamp).toISOString().slice(0, 10) === text;
}

function resolveApiErrorMessage(requestPath, payload, fallback) {
  return payload?.desc || payload?.message || fallback || `${requestPath} 调用失败`;
}

// 错误类型定义
class PointsApiError extends Error {
  constructor(message, code, isAuthError = false, isNetworkError = false) {
    super(message);
    this.name = 'PointsApiError';
    this.code = code;
    this.isAuthError = isAuthError;
    this.isNetworkError = isNetworkError;
  }
}

export class PointsService {
  constructor({ getConfig, getAuthSession = () => null, getDeviceId = () => '' }) {
    this.getConfig = getConfig;
    this.getAuthSession = getAuthSession;
    this.getDeviceId = getDeviceId;
  }

  resolveConfig() {
    const config = this.getConfig();
    if (!config?.baseUrl) {
      throw new Error('积分服务未配置，请设置 Points HTTP API 地址');
    }
    return config;
  }

  resolveAsrConfig(token = '') {
    const config = this.resolveConfig();
    const resolvedToken = String(token || this.getAuthSession?.() || config?.token || '').trim();

    if (!resolvedToken) {
      throw new PointsApiError('缺少接口 token，请先完成配置', 'NO_TOKEN', true);
    }

    return {
      baseUrl: config.baseUrl,
      token: resolvedToken,
    };
  }

  async request(requestPath, { method = 'GET', token = '', query = {}, body = null } = {}) {
    const config = this.resolveConfig();
    const resolvedToken = String(token || this.getAuthSession?.() || config?.token || '').trim();

    if (!resolvedToken) {
      console.error('[PointsService] 缺少 token')
      throw new PointsApiError('缺少积分接口 token，请先完成用户登录', 'NO_TOKEN', true);
    }

    const queryString = buildQuery(query);
    const url = `${config.baseUrl}${requestPath}${queryString ? `?${queryString}` : ''}`;

    const headers = {
      token: resolvedToken,
    };

    if (body !== null) {
      headers['Content-Type'] = 'application/json';
    }

    console.log('[PointsService] 发起请求:', {
      method,
      url,
      authenticated: Boolean(headers.token),
    })

    let response;
    try {
      const controller = new AbortController();
      const timeoutId = setTimeout(() => controller.abort(), 30000);

      response = await fetch(url, {
        method,
        headers,
        body: body !== null ? JSON.stringify(body) : undefined,
        signal: controller.signal,
      });

      clearTimeout(timeoutId);

    } catch (error) {
      // 保留原始错误信息
      if (error.name === 'AbortError') {
        console.error('[PointsService] 请求超时:', { path: requestPath, timeout: 30000 })
        throw new PointsApiError(
          `${requestPath} 请求超时，请检查网络连接`,
          'TIMEOUT',
          false,
          true
        );
      }
      console.error('[PointsService] 网络错误:', {
        path: requestPath,
        errorName: error.name,
        errorMessage: error.message,
      })
      throw new PointsApiError(
        `${requestPath} 调用失败: ${error.message || '无法连接到服务器'}`,
        'NETWORK_ERROR',
        false,
        true
      );
    }

    const text = await response.text();
    let data = null;

    if (text) {
      try {
        data = JSON.parse(text);
      } catch {
        console.error('[PointsService] JSON 解析失败:', {
          path: requestPath,
          textLength: text.length,
          textPreview: text.slice(0, 100),
        })
        throw new PointsApiError(
          `${requestPath} 返回数据不是合法 JSON`,
          'INVALID_JSON'
        );
      }
    }

    if (!response.ok) {
      console.error('[PointsService] HTTP 错误:', {
        path: requestPath,
        status: response.status,
        statusText: response.statusText,
        data,
      })
      throw new PointsApiError(
        resolveApiErrorMessage(requestPath, data, `${requestPath} 调用失败: HTTP ${response.status}`),
        `HTTP_${response.status}`
      );
    }

    // 检查业务错误码
    if (data?.code && data.code !== '000000') {
      console.warn('[PointsService] 业务错误:', {
        path: requestPath,
        code: data.code,
        desc: data.desc,
      })

      // 特殊处理 token 过期错误
      if (data.code === '100002') {
        console.error('[PointsService] Token 过期:', {
          path: requestPath,
          code: data.code,
          desc: data.desc,
        })
        throw new PointsApiError(
          data.desc || '未登录或登录已失效',
          '100002',
          true // 标记为认证错误
        );
      }

      throw new PointsApiError(
        resolveApiErrorMessage(requestPath, data, data.code),
        data.code
      );
    }

    return data;
  }

  async requestPublic(requestUrl, { method = 'GET', query = {} } = {}) {
    const queryString = buildQuery(query);
    const normalizedUrl = String(requestUrl || '').trim();
    if (!normalizedUrl) {
      throw new PointsApiError('缺少公共活动位接口地址', 'NO_PROMO_URL');
    }

    const url = `${normalizedUrl}${queryString ? `?${queryString}` : ''}`;

    let response;
    try {
      const controller = new AbortController();
      const timeoutId = setTimeout(() => controller.abort(), 30000);

      response = await fetch(url, {
        method,
        signal: controller.signal,
      });

      clearTimeout(timeoutId);
    } catch (error) {
      if (error.name === 'AbortError') {
        throw new PointsApiError(
          '公共活动位请求超时，请检查网络连接',
          'PROMO_TIMEOUT',
          false,
          true
        );
      }

      throw new PointsApiError(
        `公共活动位调用失败: ${error.message || '无法连接到服务器'}`,
        'PROMO_NETWORK_ERROR',
        false,
        true
      );
    }

    const text = await response.text();
    let data = null;

    if (text) {
      try {
        data = JSON.parse(text);
      } catch {
        if (!response.ok) {
          throw new PointsApiError(text || `公共活动位调用失败: HTTP ${response.status}`, `HTTP_${response.status}`);
        }
        throw new PointsApiError('公共活动位返回数据不是合法 JSON', 'INVALID_JSON');
      }
    }

    if (!response.ok) {
      throw new PointsApiError(
        resolveApiErrorMessage(url, data, `公共活动位调用失败: HTTP ${response.status}`),
        `HTTP_${response.status}`
      );
    }

    if (data?.code && data.code !== '000000') {
      throw new PointsApiError(
        resolveApiErrorMessage(url, data, data.code),
        data.code
      );
    }

    return data;
  }

  async get(requestPath, { token = '', query = {} } = {}) {
    return this.request(requestPath, { method: 'GET', token, query });
  }

  async post(requestPath, { token = '', body = {} } = {}) {
    return this.request(requestPath, { method: 'POST', token, body });
  }

  async recognizeAudio(payload = {}) {
    const {
      audio,
      source: _source = 'unknown',
      fileName: _fileName = '',
      localPath: _localPath = '',
      language,
      accent,
      format,
      encoding,
    } = payload;

    if (!(audio instanceof Blob)) {
      throw new Error('audio 必须是 Blob 或 File');
    }

    const config = this.resolveAsrConfig(payload?.token);

    const formData = new FormData();
    formData.append('audio', audio);

    if (language) formData.append('language', language);
    if (accent) formData.append('accent', accent);
    if (format) formData.append('format', format);
    if (encoding) formData.append('encoding', encoding);

    let response;
    try {
      const controller = new AbortController();
      const timeoutId = setTimeout(() => controller.abort(), 30000);

      response = await fetch(`${config.baseUrl}/api/v1/asr/recognize`, {
        method: 'POST',
        headers: {
          token: config.token,
        },
        body: formData,
        signal: controller.signal,
      });

      clearTimeout(timeoutId);
    } catch (error) {
      if (error.name === 'AbortError') {
        throw new PointsApiError('ASR 请求超时，请检查网络连接', 'ASR_TIMEOUT', false, true);
      }
      throw new PointsApiError(
        `ASR 调用失败: ${error.message || '无法连接到服务器'}`,
        'ASR_NETWORK_ERROR',
        false,
        true
      );
    }

    const text = await response.text();
    let data = null;

    if (text) {
      try {
        data = JSON.parse(text);
      } catch {
        if (!response.ok) {
          throw new PointsApiError(text || `ASR 调用失败: HTTP ${response.status}`, `HTTP_${response.status}`);
        }
        throw new PointsApiError('ASR 返回数据不是合法 JSON', 'INVALID_JSON');
      }
    }

    if (!response.ok) {
      throw new PointsApiError(
        resolveApiErrorMessage('/api/v1/asr/recognize', data, `ASR 调用失败: HTTP ${response.status}`),
        `HTTP_${response.status}`
      );
    }

    if (data?.code && data.code !== '000000') {
      throw new PointsApiError(
        resolveApiErrorMessage('/api/v1/asr/recognize', data, data.code),
        data.code
      );
    }

    const normalizedText =
      typeof data?.text === 'string'
        ? data.text
        : typeof data?.data?.text === 'string'
          ? data.data.text
          : '';

    return {
      ...data,
      text: normalizedText,
    };
  }

  /**
   * 获取当前账号、当前服务端业务日的宠物打工权威快照。
   *
   * @param {string} token 当前登录会话令牌。
   * @returns {Promise<object>} 服务端统一响应 envelope。
   * @throws {PointsApiError} token 缺失、网络失败或服务端业务失败时抛出。
   */
  async getPetWorkSnapshot(token = '') {
    return this.get('/api/v1/pet-work', { token })
  }

  /**
   * 申请当前业务日已经达到的最高宠物打工奖励档位。
   *
   * 请求体只允许发送服务端定义的三个字段。调用方即使传入积分金额、用户 ID、
   * 工作分数或任务信息，也不会被透传到网络边界。
   *
   * @param {object} payload 奖励上下文。
   * @param {string} payload.businessDate 最近快照返回的服务端业务日。
   * @param {string} payload.configVersion 最近快照返回的配置版本。
   * @param {string} payload.reachedTierId 本地计算出的最高达成档位。
   * @param {string} [payload.token] 当前登录会话令牌。
   * @returns {Promise<object>} 服务端统一响应 envelope。
   * @throws {PointsApiError} 必填字段无效、网络失败或服务端拒绝时抛出。
   */
  async grantPetWorkReward(payload = {}) {
    const businessDate = String(payload?.businessDate || '').trim()
    const reachedTierId = String(payload?.reachedTierId || '').trim()
    const configVersion = String(payload?.configVersion || '').trim()
    if (!isBusinessDate(businessDate) || !reachedTierId || !configVersion) {
      throw new PointsApiError('宠物打工奖励请求缺少有效业务日、档位或配置版本', '100001')
    }

    return this.post('/api/v1/pet-work/rewards', {
      token: payload?.token,
      body: {
        businessDate,
        configVersion,
        reachedTierId,
      },
    })
  }

  async queryRecords(payload = {}) {
    const pageNo = Math.max(1, Number(payload?.pageNo) || 1);
    const pageSize = Math.min(100, Math.max(1, Number(payload?.pageSize) || 20));
    const recordType = normalizeRecordType(payload?.recordType);

    const data = await this.get('/api/v1/points/records', {
      token: payload?.token,
      query: {
        pageNo,
        pageSize,
        recordType,
      },
    });

    return data;
  }

  async queryRecordsV2(payload = {}) {
    const pageNo = Math.max(1, Number(payload?.pageNo) || 1);
    const pageSize = Math.min(100, Math.max(1, Number(payload?.pageSize) || 20));
    const direction = normalizeDirection(payload?.direction);
    const granularity = normalizeGranularity(payload?.granularity);

    const data = await this.get('/api/v2/points/records', {
      token: payload?.token,
      query: {
        pageNo,
        pageSize,
        direction,
        granularity,
      },
    });

    return data;
  }

  // 团队 V1 接口：返回 raw ledger，使用 recordType=all|earned|consumed；queryPointsSummary 拉余额用
  async queryTeamRecords(payload = {}) {
    const data = await this.get('/api/v1/team-points/balance', {
      token: payload?.token,
    });
    const payloadData = data?.data || {};
    return {
      ...data,
      data: {
        ...payloadData,
        balance: payloadData.currentBalance,
      },
    };
  }

  // 团队 V2 接口：响应形状与个人 V2 一致（聚合 chatId / 累加 points / first/lastCreatedAt）
  async queryTeamRecordsV2(payload = {}) {
    const pageNo = Math.max(1, Number(payload?.pageNo) || 1);
    const pageSize = Math.min(100, Math.max(1, Number(payload?.pageSize) || 20));
    const direction = normalizeDirection(payload?.direction);
    const granularity = normalizeGranularity(payload?.granularity);

    return this.get('/api/v2/team-points/records', {
      token: payload?.token,
      query: {
        pageNo,
        pageSize,
        direction,
        granularity,
      },
    });
  }

  async queryInvitationCodes(payload = {}) {
    return this.get('/api/v1/invitation-codes', {
      token: payload?.token,
    });
  }

  async redeemRedemptionCode(payload = {}) {
    const { token, code } = payload;
    const normalizedCode = String(code || '').trim();

    if (!normalizedCode) {
      throw new Error('缺少兑换码参数');
    }

    return this.post('/api/v1/points/redemption-codes/redeem', {
      token,
      body: { code: normalizedCode },
    });
  }

  async completeFirstLogin(payload = {}) {
    const inviteCode = String(payload?.inviteCode || '').trim();
    const deviceId = inviteCode ? String(payload?.deviceId || this.getDeviceId?.() || '').trim() : '';
    const body = {};
    if (inviteCode) body.inviteCode = inviteCode;
    if (deviceId) body.deviceId = deviceId;

    // 渠道归因：拿到稳定渠道号 channel 则随首次登录初始化上报；后端只在首次初始化记录，重复调用不覆盖。
    // 特殊渠道商（如 CakeGrowth/cg）额外带动态 visitedId，供后端注册成功后异步回传渠道商；
    // visitedId 不参与平台渠道统计，仅在 channel 存在时一起上报。
    const { channel, visitedId } = await ensureChannelAttribution();
    if (channel) body.channel = channel;
    if (channel && visitedId) body.visitedId = visitedId;

    return this.post('/api/v1/points/first-login', {
      token: payload?.token,
      body,
    });
  }

  async queryPromotions() {
    const config = this.resolveConfig();
    const promoBannersUrl = `${String(config?.baseUrl || '').replace(/\/+$/, '')}/api/v1/public/promo-banners`;

    return this.requestPublic(promoBannersUrl, {
      method: 'GET',
      query: {},
    });
  }

  async checkActivation(payload = {}) {
    return this.get('/api/v1/points/activation', {
      token: payload?.token,
    });
  }

  async bindInviteCode(payload = {}) {
    const { token, inviteCode } = payload;
    const normalizedInviteCode = String(inviteCode || '').trim();
    const deviceId = String(payload?.deviceId || this.getDeviceId?.() || '').trim();

    if (!normalizedInviteCode) {
      throw new Error('缺少邀请码参数');
    }

    const body = {
      inviteCode: normalizedInviteCode,
    };
    if (deviceId) {
      body.deviceId = deviceId;
    }

    return this.post('/api/v1/points/activation', {
      token,
      body,
    });
  }

  /**
   * 查询用户身份信息（是否为内部用户）
   */
  async getUserIdentity(token) {
    return this.get('/api/v1/user/identity', { token })
  }

  async getTeamAssets(token) {
    return this.get('/api/v1/user/team-assets', { token })
  }

  async setCurrentTeam(payload = {}) {
    const { token, teamId } = payload;
    const body = teamId ? { teamId: String(teamId) } : {};
    return this.post('/api/v1/user/current-team', { token, body });
  }

  async leaveTeam(payload = {}) {
    const { token, teamId } = payload;
    const normalizedTeamId = String(teamId || '').trim();
    if (!normalizedTeamId) {
      throw new Error('缺少 teamId');
    }
    return this.post('/api/v1/user/leave-team', {
      token,
      body: { teamId: normalizedTeamId },
    });
  }

  /**
   * 按 ChatId 查询对话真实积分消耗（个人积分）
   *
   * 根据文档 frontend-points-consumption-display.md，by-chat 接口从 MongoDB 主写库
   * 即时聚合当前对话的所有真实扣分（主回复、图片、搜索、工具编排、记忆提取、前处理/后处理等）。
   *
   * @param {Object} payload
   * @param {string} payload.token - 认证 token
   * @param {string} payload.chatId - 对话 ID
   * @returns {Promise<Object>} 返回 { chatId, found, points_consumed, ledger_count, firstCreatedAt?, lastCreatedAt? }
   */
  async queryPointsByChatId(payload = {}) {
    const { token, chatId } = payload;
    const normalizedChatId = String(chatId || '').trim();
    if (!normalizedChatId) {
      throw new Error('缺少 chatId');
    }
    return this.get('/api/v1/points/by-chat', {
      token,
      query: { chatId: normalizedChatId },
    });
  }

  /**
   * 按 ChatId 查询对话真实积分消耗（团队积分）
   *
   * 根据文档 frontend-points-consumption-display.md，by-chat 接口从 MongoDB 主写库
   * 即时聚合当前对话的所有真实扣分（主回复、图片、搜索、工具编排、记忆提取、前处理/后处理等）。
   *
   * @param {Object} payload
   * @param {string} payload.token - 认证 token
   * @param {string} payload.chatId - 对话 ID
   * @returns {Promise<Object>} 返回 { chatId, found, points_consumed, ledger_count, firstCreatedAt?, lastCreatedAt? }
   */
  async queryTeamPointsByChatId(payload = {}) {
    const { token, chatId } = payload;
    const normalizedChatId = String(chatId || '').trim();
    if (!normalizedChatId) {
      throw new Error('缺少 chatId');
    }
    return this.get('/api/v1/team-points/by-chat', {
      token,
      query: { chatId: normalizedChatId },
    });
  }

  /**
   * 按 TurnId 查询「本轮」真实积分消耗（个人积分）
   *
   * 与 by-chat 的区别：by-chat 是整个会话累计，by-turn 是**单次用户操作**的总消耗
   * （主请求 + 工具编排 + 上下文压缩 + 子代理 + 记忆提取/技能沉淀等后处理）。
   * 服务端同样读 MongoDB 主写库而非 Doris，避免 outbox 异步同步期间读到半值。
   *
   * 客户端一轮会查两次：渲染完成即查一次（先把主链路的数显示出来），
   * 收到 `points:turn-settled` 后再查一次补上记忆/技能那笔。一轮内 points_consumed
   * 只增不减，所以第二次是原地更新，不会闪烁或回跳。`ledger_count` 用来区分
   * 「有新流水进来」还是「同一笔被修正」。
   *
   * 老流水（turn_id 为空）查不到，这是预期：不造「未分轮」桶，也不回填历史数据。
   *
   * @param {Object} payload
   * @param {string} payload.token - 认证 token
   * @param {string} payload.turnId - 轮次 ID（= 该轮起始 msgId）
   * @returns {Promise<Object>} { turnId, found, points_consumed, ledger_count, firstCreatedAt?, lastCreatedAt? }
   */
  async queryPointsByTurnId(payload = {}) {
    const { token, turnId } = payload;
    const normalizedTurnId = String(turnId || '').trim();
    if (!normalizedTurnId) {
      throw new Error('缺少 turnId');
    }
    return this.get('/api/v1/points/by-turn', {
      token,
      query: { turnId: normalizedTurnId },
    });
  }

  /** 按 TurnId 查询本轮真实积分消耗（团队积分）。口径与个人侧完全一致。 */
  async queryTeamPointsByTurnId(payload = {}) {
    const { token, turnId } = payload;
    const normalizedTurnId = String(turnId || '').trim();
    if (!normalizedTurnId) {
      throw new Error('缺少 turnId');
    }
    return this.get('/api/v1/team-points/by-turn', {
      token,
      query: { turnId: normalizedTurnId },
    });
  }
}
