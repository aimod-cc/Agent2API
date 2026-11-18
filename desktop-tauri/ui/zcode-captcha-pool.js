/* 套餐池守卫只读概览；实际模型验证始终绑定用户点选的账号和配置。 */
(() => {
  const POLL_MS = 2500;
  const IDLE_POLL_MS = 15000;
  let timer = 0;
  let stopped = false;
  let running = false;
  let generation = 0;
  let active = null;
  let stats = null;
  let status = { state: 'idle', error: '', accountId: '', provider: '' };
  const accountStats = new Map();
  const accountStatuses = new Map();
  const accountReads = new Map();
  let readSequence = 0;

  const idle = accountId => ({ state: 'idle', error: '', accountId, provider: '' });
  const providerValid = value => value === 'zcode' || value === 'zcode-intl';
  const text = value => typeof value === 'string' && value.trim() && !/[\u0000-\u001f\u007f]/.test(value);
  const cancelled = () => new DOMException('请求已取消', 'AbortError');
  function remember(map, key, value) {
    if (!map.has(key) && map.size >= 128) map.delete(map.keys().next().value);
    map.set(key, value);
  }

  function publish(state, error = '', accountId = '', provider = '') {
    const next = { state, error: String(error || ''), accountId, provider };
    const previous = accountId ? accountStatuses.get(accountId) : status;
    if (previous && previous.state === next.state && previous.error === next.error && previous.provider === provider) return;
    if (accountId) remember(accountStatuses, accountId, next);
    else status = next;
    window.dispatchEvent(new CustomEvent('zcode-captcha-status', { detail: { ...next } }));
  }

  function bindingOf(config, accountId) {
    if (!config || typeof config !== 'object' || Array.isArray(config) || typeof config.enabled !== 'boolean') throw new Error('invalid_captcha_config');
    if (config.enabled && (!text(config.region) || !text(config.prefix) || !text(config.sceneId))) throw new Error('invalid_captcha_config');
    const bound = ['provider', 'accountId', 'configId', 'engine'].some(key => Object.prototype.hasOwnProperty.call(config, key));
    if (!bound) return null;
    if (!providerValid(config.provider) || config.accountId !== accountId || !text(config.configId)
      || (config.enabled ? config.engine !== 'aliyun' : !['none', 'aliyun'].includes(config.engine))) throw new Error('invalid_captcha_binding');
    return { provider: config.provider, accountId, configId: config.configId };
  }

  function matchingStats(value, accountId, provider) {
    if (!value || typeof value !== 'object' || Array.isArray(value)) return false;
    if (!accountId) return true;
    const bound = ['provider', 'accountId'].some(key => Object.prototype.hasOwnProperty.call(value, key));
    if (bound || provider) return providerValid(value.provider) && value.accountId === accountId && value.captchaAccountId === accountId
      && (!provider || value.provider === provider);
    return value.captchaAccountId === undefined || value.captchaAccountId === null || value.captchaAccountId === accountId;
  }

  function schedule(delay) {
    window.clearTimeout(timer);
    if (!stopped) timer = window.setTimeout(() => void check(), delay);
  }

  async function readStats(accountId = '', provider, signal) {
    const bridge = window.workbuddyDesktop;
    const epoch = generation;
    const origin = window.location.origin;
    const requestId = ++readSequence;
    if (accountId) remember(accountReads, accountId, requestId);
    const requireCurrent = () => {
      if (signal?.aborted || epoch !== generation || bridge !== window.workbuddyDesktop || origin !== window.location.origin) throw cancelled();
    };
    try {
      requireCurrent();
      if (!bridge?.zcodeCaptchaStats || (provider !== undefined && !providerValid(provider))) throw new Error('captcha_api_unavailable');
      const value = await bridge.zcodeCaptchaStats(accountId || undefined, provider);
      requireCurrent();
      if (!matchingStats(value, accountId, provider)) throw new Error('invalid_captcha_scope');
      if (value.error || value.configError) throw new Error('captcha_status_unavailable');
      if (accountId) {
        if (accountReads.get(accountId) === requestId) remember(accountStats, accountId, value);
      } else stats = value;
      return value;
    } catch (error) {
      requireCurrent();
      if (error?.name === 'AbortError') throw error;
      throw new Error(wbI18n.t('验证码状态读取失败，请刷新后重试'));
    }
  }

  function stateOf(value, accountId = '', provider = '') {
    if (value.error || value.configError) return publish('error', wbI18n.t('验证码状态读取失败，请刷新后重试'), accountId, provider);
    if (value.required === false) return publish('disabled', '', accountId, provider);
    if (Number(value.startPlanAccounts) > 0 && value.needsTokens === true) return publish('needs_verification', '', accountId, provider);
    publish('idle', '', accountId, provider);
  }

  async function check() {
    if (running || stopped) return;
    if ((window.wbApp?.currentPage || '') !== 'accounts') {
      schedule(IDLE_POLL_MS);
      return;
    }
    running = true;
    const epoch = generation;
    try {
      const value = await readStats();
      if (epoch !== generation || stopped) return;
      stateOf(value);
      const poll = Number(value.pollMs) || POLL_MS;
      schedule(value.demandActive === true ? Math.min(30000, Math.max(1000, poll)) : IDLE_POLL_MS);
    } catch {
      if (epoch === generation && !stopped) {
        publish('error', wbI18n.t('验证码状态读取失败，请刷新后重试'));
        schedule(IDLE_POLL_MS);
      }
    } finally {
      running = false;
    }
  }

  async function verify(accountId, signal, onPhase) {
    if (active) throw new Error(wbI18n.t('已有验证码流程正在进行，请先完成或取消'));
    const bridge = window.workbuddyDesktop;
    if (!text(accountId) || !bridge?.zcodeClaimCaptchaConfig || !bridge?.pushZcodeCaptchaTokens) {
      throw new Error(wbI18n.t('验证码令牌入池接口不可用'));
    }
    const current = { controller: new AbortController(), bridge, origin: window.location.origin, epoch: generation };
    const onAbort = () => current.controller.abort();
    signal?.addEventListener('abort', onAbort, { once: true });
    if (signal?.aborted) onAbort();
    active = current;
    let binding = null;
    let submitting = false;
    let accepted = false;
    const requireCurrent = () => {
      if (active !== current || current.controller.signal.aborted || current.epoch !== generation
        || current.bridge !== window.workbuddyDesktop || current.origin !== window.location.origin) throw cancelled();
    };
    const reportPhase = phase => { requireCurrent(); onPhase?.(phase); };
    publish('verifying', '', accountId);
    try {
      requireCurrent();
      reportPhase('preparing');
      const config = Object.freeze({ ...await bridge.zcodeClaimCaptchaConfig(accountId) });
      requireCurrent();
      binding = bindingOf(config, accountId);
      const value = await readStats(accountId, binding?.provider, current.controller.signal);
      requireCurrent();
      if (value.required === false) {
        stateOf(value, accountId, binding?.provider || '');
        return false;
      }
      if (!config.enabled) throw new Error('captcha_config_unavailable');
      const captcha = window.wbAliyunCaptcha;
      if (!captcha?.requestToken) throw new Error('captcha_component_unavailable');
      const token = await captcha.requestToken(config, current.controller.signal, { onPhase: reportPhase });
      requireCurrent();
      if (!text(token?.param)) throw new Error('invalid_captcha_token');
      reportPhase('submitting');
      submitting = true;
      const outcome = await bridge.pushZcodeCaptchaTokens(binding
        ? { ...binding, tokens: [{ param: token.param }] }
        : [{ param: token.param, region: config.region }]);
      requireCurrent();
      if (outcome?.accepted !== 1 || binding && (outcome.provider !== binding.provider || outcome.accountId !== accountId)) {
        throw new Error('captcha_submit_unconfirmed');
      }
      accepted = true;
      // POST 只是回执，不写入任何库存；回读同一个账号后才更新该账号状态。
      const refreshed = await readStats(accountId, binding?.provider, current.controller.signal);
      requireCurrent();
      stateOf(refreshed, accountId, binding?.provider || '');
      return true;
    } catch (error) {
      const stale = current.epoch !== generation || current.bridge !== window.workbuddyDesktop || current.origin !== window.location.origin;
      const wasCancelled = stale || current.controller.signal.aborted || ['AbortError', 'CaptchaCancelledError'].includes(error?.name);
      const message = wasCancelled ? '' : accepted ? wbI18n.t('验证码已提交，但状态刷新失败，请手动刷新')
        : submitting ? wbI18n.t('验证码提交结果未确认，请刷新状态后重新验证；不会自动重发')
          : wbI18n.t('验证码验证未完成，请刷新配置后重试');
      if (!stale) publish(wasCancelled ? 'paused' : 'error', message, accountId, binding?.provider || '');
      throw wasCancelled ? cancelled() : new Error(message);
    } finally {
      signal?.removeEventListener('abort', onAbort);
      if (active === current) active = null;
    }
  }

  function start() {
    stopped = false;
    schedule(1500);
  }

  function stop() {
    stopped = true;
    generation++;
    active?.controller.abort();
    accountStats.clear();
    accountStatuses.clear();
    accountReads.clear();
    stats = null;
    status = idle('');
    window.clearTimeout(timer);
  }

  window.wbZcodeCaptchaPool = {
    start, stop, verify, refresh: readStats,
    status: accountId => ({ ...(accountId ? accountStatuses.get(accountId) || idle(accountId) : status) }),
    stats: accountId => accountId ? accountStats.get(accountId) || null : stats,
  };
  window.addEventListener('wb:page', event => {
    if (event.detail === 'accounts' && !stopped) schedule(0);
  });
  window.addEventListener('pagehide', stop);
  window.addEventListener('pageshow', start);
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start, { once: true });
  else start();
})();
