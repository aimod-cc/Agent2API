/* 只协作后端正在等待的领取任务；按账号轮询，不预生成票据或直接领取。 */
(() => {
  const POLL_MS = 5000;
  const IDLE_MS = 30000;
  const clientId = window.crypto?.randomUUID?.() || `claim-${Date.now()}-${Math.random().toString(36).slice(2)}`;
  let timer = 0;
  let polling = false;
  let stopped = false;
  let generation = 0;
  let nextPollAt = 0;
  let lastAccount = '';
  let active = null;
  let value = null;
  let pausedUntil = 0;
  let unsubscribe = null;
  const handled = new Map();
  const t = (key, params) => window.wbI18n?.t?.(key, params) || key;
  const providerValid = provider => provider === 'zcode' || provider === 'zcode-intl';
  const text = item => typeof item === 'string' && item.trim().length > 0 && item.length <= 512 && !/[\u0000-\u001f\u007f]/.test(item);
  const cancelled = () => new DOMException('请求已取消', 'AbortError');
  const accountKey = account => JSON.stringify([account.provider, account.id]);

  function rememberTask(task) {
    const now = performance.now();
    for (const [key, until] of handled) if (until <= now) handled.delete(key);
    const key = JSON.stringify([task.provider, task.accountId, task.requestId]);
    if (handled.has(key)) return false;
    // 租约最多 50 秒；只记请求标识，不存票据，避免完成回执丢失后再次处理同一任务。
    handled.set(key, now + 60000);
    return true;
  }

  function publish(next) {
    value = next;
    window.dispatchEvent(new CustomEvent('zcode-claim-verification', { detail: next }));
  }

  function schedule(delay) {
    window.clearTimeout(timer);
    if (!stopped) timer = window.setTimeout(() => void poll(), Math.max(delay, nextPollAt - Date.now(), 0));
  }

  function accounts() {
    const rows = window.wbApp?.getState?.()?.accounts?.accounts;
    if (!Array.isArray(rows)) return [];
    const seen = new Set();
    return rows.filter(account => {
      if (!account || !text(account.id) || !providerValid(account.provider) || account.available === false) return false;
      const key = accountKey(account);
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    });
  }

  function sourceCurrent(flow) {
    return flow.bridge === window.workbuddyDesktop && flow.origin === window.location.origin;
  }

  function accountCurrent(flow) {
    return accounts().some(account => account.id === flow.task.accountId && account.provider === flow.task.provider);
  }

  function current(flow) {
    return active === flow && !stopped && flow.generation === generation && sourceCurrent(flow) && accountCurrent(flow);
  }

  function requireCurrent(flow) {
    if (performance.now() >= flow.deadline) abort(flow, 'timeout');
    if (!current(flow) || flow.controller.signal.aborted) throw cancelled();
  }

  function canVerify() {
    if (Date.now() < pausedUntil) return false;
    if (!window.wbAliyunCaptcha?.requestToken || window.wbAliyunCaptcha.isBusy()) return false;
    if (window.workbuddyDesktop?.platform === 'web' && document.visibilityState !== 'visible') return false;
    return !document.querySelector('[data-slot="dialog-content"][data-open], [data-slot="alert-dialog-content"][data-open], [data-slot="sheet-content"][data-open]');
  }

  function bindingOf(config, accountId, provider) {
    if (!config || typeof config !== 'object' || Array.isArray(config) || config.enabled !== true
      || ![config.prefix, config.sceneId, config.region].every(text)) throw new Error('invalid_claim_config');
    if (!['engine', 'provider', 'accountId', 'configId'].some(key => Object.prototype.hasOwnProperty.call(config, key))) return null;
    if (config.engine !== 'aliyun' || config.provider !== provider || config.accountId !== accountId || !text(config.configId)) {
      throw new Error('invalid_claim_binding');
    }
    return Object.freeze({ provider, accountId, configId: config.configId });
  }

  function taskOf(raw, account) {
    if (!raw || typeof raw !== 'object' || !text(raw.requestId) || !text(raw.accountId) || !text(raw.planId)
      || raw.provider !== account.provider || raw.accountId !== account.id
      || Object.prototype.hasOwnProperty.call(raw, 'clientId') && raw.clientId !== clientId
      || !Number.isFinite(raw.remainingMs) || raw.remainingMs <= 0 || !Number.isFinite(raw.expiresAt) || raw.expiresAt <= 0) {
      throw new Error('invalid_claim_task');
    }
    const config = Object.freeze({ ...raw.config });
    const binding = bindingOf(config, account.id, account.provider);
    if (Object.prototype.hasOwnProperty.call(raw, 'configId') && raw.configId !== binding?.configId) throw new Error('invalid_claim_binding');
    return {
      task: Object.freeze({ requestId: raw.requestId, provider: raw.provider, accountId: raw.accountId, planId: raw.planId,
        planName: text(raw.planName) ? raw.planName : raw.planId, remainingMs: raw.remainingMs, expiresAt: raw.expiresAt, config }),
      binding,
    };
  }

  function abortable(promise, signal) {
    return new Promise((resolve, reject) => {
      const abort = () => reject(cancelled());
      signal.addEventListener('abort', abort, { once: true });
      if (signal.aborted) abort();
      Promise.resolve(promise).then(resolve, reject).finally(() => signal.removeEventListener('abort', abort));
    });
  }

  function complete(flow, status, param) {
    if (flow.completion) return flow.completion;
    if (!flow.validated || !sourceCurrent(flow)) return Promise.resolve(null);
    flow.submitted = true;
    flow.completionStatus = status;
    // 绑定字段仅取自本次任务；失败/取消/超时回执绝不带票据或上游错误正文。
    const payload = { action: 'complete', provider: flow.task.provider, accountId: flow.task.accountId,
      clientId, requestId: flow.task.requestId, planId: flow.task.planId,
      ...(flow.binding ? { configId: flow.binding.configId } : {}), status,
      ...(status === 'verified' ? { param } : {}) };
    flow.completion = new Promise((resolve, reject) => {
      const timeout = window.setTimeout(() => reject(new Error('claim_submit_timeout')), 10000);
      Promise.resolve().then(() => {
        if (!sourceCurrent(flow)) throw cancelled();
        if (status === 'verified') requireCurrent(flow);
        return flow.bridge.zcodeClaimVerification(payload);
      }).then(resolve, reject).finally(() => window.clearTimeout(timeout));
    });
    return flow.completion;
  }

  function abort(flow, status = 'cancelled') {
    if (!flow || active !== flow) return;
    flow.cancelStatus = status;
    flow.controller.abort();
    if (!flow.submitted) void complete(flow, status).catch(() => {});
  }

  function guard() {
    if (active && !current(active)) abort(active);
  }

  function subscribe() {
    if (!unsubscribe && window.wbApp?.subscribeRefresh) unsubscribe = window.wbApp.subscribeRefresh(guard);
  }

  async function poll() {
    if (stopped) return;
    if (polling) { schedule(POLL_MS); return; }
    subscribe();
    guard();
    const bridge = window.workbuddyDesktop;
    if (!bridge?.zcodeClaimVerification || active?.controller.signal.aborted) { schedule(IDLE_MS); return; }
    const list = accounts();
    const owned = active;
    const selected = owned ? list.find(account => account.id === owned.task.accountId && account.provider === owned.task.provider)
      : list[(list.findIndex(account => accountKey(account) === lastAccount) + 1) % Math.max(1, list.length)];
    if (!selected) { schedule(IDLE_MS); return; }
    if (!owned) lastAccount = accountKey(selected);
    polling = true;
    const epoch = generation;
    const origin = window.location.origin;
    const selectedReady = !owned && canVerify();
    const startedAt = performance.now();
    nextPollAt = Date.now() + POLL_MS;
    let delay = POLL_MS;
    try {
      const result = await bridge.zcodeClaimVerification({ action: 'poll', provider: selected.provider,
        accountId: selected.id, clientId, ready: selectedReady });
      if (stopped || epoch !== generation || bridge !== window.workbuddyDesktop || origin !== window.location.origin) return;
      if (!result || typeof result !== 'object'
        || Object.prototype.hasOwnProperty.call(result, 'accountId') && result.accountId !== selected.id
        || Object.prototype.hasOwnProperty.call(result, 'provider') && result.provider !== selected.provider
        || Object.prototype.hasOwnProperty.call(result, 'clientId') && result.clientId !== clientId) return;
      if (owned) {
        if (active !== owned || owned.submitted || !current(owned)) return;
        // 只相信本账号的空任务；其它账号的回应不能取消正在处理的任务。
        if (result?.task && (result.task.accountId !== owned.task.accountId || result.task.provider !== owned.task.provider)) return;
        if (result?.supported === false || result?.task === null) abort(owned, 'timeout');
        else if (result?.task) {
          try {
            const next = taskOf(result.task, selected);
            if (next.task.requestId !== owned.task.requestId || next.task.planId !== owned.task.planId
              || next.binding?.configId !== owned.binding?.configId
              || ['prefix', 'sceneId', 'region'].some(key => next.task.config[key] !== owned.task.config[key])) abort(owned, 'timeout');
            else {
              owned.deadline = Math.min(owned.deadline, startedAt + next.task.remainingMs - 750);
              if (performance.now() >= owned.deadline) abort(owned, 'timeout');
            }
          } catch { abort(owned, 'failed'); }
        }
        return;
      }
      if (result?.supported === false) { delay = list.length > 1 ? POLL_MS : IDLE_MS; return; }
      if (!active && result?.task && selectedReady && canVerify()) void verify(taskOf(result.task, selected), bridge, epoch, startedAt);
    } catch {
      // 不重试领取或回传票据；下一拍只发新的只读账号轮询。
      delay = active ? POLL_MS : IDLE_MS;
    } finally {
      polling = false;
      if (epoch === generation) schedule(delay);
    }
  }

  async function verify(candidate, bridge, epoch, startedAt) {
    if (active || stopped || epoch !== generation) return;
    const { task, binding } = candidate;
    const remaining = Math.min(50000, task.remainingMs) - (performance.now() - startedAt);
    if (remaining <= 1000 || !rememberTask(task)) return;
    const controller = new AbortController();
    const flow = { task, binding, bridge, origin: window.location.origin, generation: epoch, controller,
      deadline: performance.now() + remaining - 750, validated: Boolean(binding), submitted: false,
      completion: null, completionStatus: '', cancelStatus: 'cancelled' };
    active = flow;
    const account = accounts().find(account => account.id === task.accountId && account.provider === task.provider);
    const info = { requestId: task.requestId, provider: task.provider, accountId: task.accountId,
      accountName: account?.displayName || account?.name || task.accountId, planName: task.planName };
    const phase = next => {
      if (current(flow) && !controller.signal.aborted && performance.now() < flow.deadline) publish({ ...info, phase: next });
    };
    phase('preparing');
    const timeout = window.setTimeout(() => abort(flow, 'timeout'), remaining - 750);
    try {
      requireCurrent(flow);
      if (!binding) {
        // 账号元数据没有执行体标志：只有账号配置也明确为旧形状，才兼容旧原生任务。
        if (!bridge.zcodeClaimCaptchaConfig) throw new Error('legacy_claim_unverified');
        const official = await abortable(bridge.zcodeClaimCaptchaConfig(task.accountId), controller.signal);
        requireCurrent(flow);
        if (bindingOf(official, task.accountId, task.provider)
          || ['prefix', 'sceneId', 'region'].some(key => official[key] !== task.config[key])) throw new Error('legacy_claim_changed');
        flow.validated = true;
      }
      const token = await abortable(window.wbAliyunCaptcha.requestToken(task.config, controller.signal, { onPhase: phase }), controller.signal);
      requireCurrent(flow);
      if (typeof token?.param !== 'string' || !token.param.trim() || token.param.length > 16384 || /[\r\n\0]/.test(token.param)) {
        throw new Error('invalid_claim_token');
      }
      phase('submitting');
      const result = await complete(flow, 'verified', token.param);
      if (current(flow) && result?.accepted !== true) {
        window.wbApp?.toast(t('领取验证任务已结束，请到套餐明细查看结果'), 'warn');
      }
      // accepted 只代表后端接收了验证结果，不代表套餐领取成功。
    } catch (error) {
      if (epoch === generation && sourceCurrent(flow)) pausedUntil = Date.now() + 5 * 60000;
      const wasCancelled = controller.signal.aborted || ['CaptchaCancelledError', 'AbortError'].includes(error?.name);
      if (!flow.submitted) {
        try { await complete(flow, wasCancelled ? flow.cancelStatus : 'failed'); } catch {}
      }
      if (!stopped && epoch === generation && sourceCurrent(flow)) {
        window.wbApp?.toast(flow.completionStatus === 'verified'
          ? t('验证提交结果未确认，请查看签到记录，勿重复领取')
          : t('本次自动领取未完成验证，请在套餐明细中手动领取'), 'warn');
      }
    } finally {
      window.clearTimeout(timeout);
      if (active === flow) { active = null; publish(null); }
      if (epoch === generation) schedule(POLL_MS);
    }
  }

  function cancel() {
    if (active && !active.submitted) abort(active);
  }

  function start() {
    stopped = false;
    subscribe();
    schedule(1000);
  }

  function stop() {
    stopped = true;
    generation++;
    window.clearTimeout(timer);
    const flow = active;
    if (flow) { abort(flow); active = null; }
    publish(null);
    unsubscribe?.();
    unsubscribe = null;
  }

  window.wbZcodeClaimVerification = { cancel, status: () => value };
  window.addEventListener('wb:app-ready', () => { subscribe(); if (!stopped) schedule(0); });
  window.addEventListener('pagehide', stop);
  window.addEventListener('pageshow', start);
  window.addEventListener('visibilitychange', () => { if (!stopped) { guard(); schedule(0); } });
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start, { once: true });
  else start();
})();
