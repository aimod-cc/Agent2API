/* 定时领取验证：只处理后端正在等待的任务，不预生成令牌，也不参与模型调用。 */
(() => {
  const POLL_MS = 5000;
  const IDLE_MS = 30000;
  const clientId = window.crypto?.randomUUID?.() || `claim-${Date.now()}-${Math.random().toString(36).slice(2)}`;
  let timer = 0;
  let polling = false;
  let stopped = false;
  let active = null;
  let value = null;
  let pausedUntil = 0;
  const t = (key, params) => window.wbI18n?.t?.(key, params) || key;

  function publish(next) {
    value = next;
    window.dispatchEvent(new CustomEvent('zcode-claim-verification', { detail: next }));
  }

  function schedule(delay) {
    window.clearTimeout(timer);
    if (!stopped) timer = window.setTimeout(() => void poll(), delay);
  }

  function hasAccounts() {
    const accounts = window.wbApp?.getState?.()?.accounts?.accounts;
    return Array.isArray(accounts) && accounts.some(account => account.provider === 'zcode' || account.provider === 'zcode-intl');
  }

  function canVerify() {
    if (Date.now() < pausedUntil) return false;
    if (!window.wbAliyunCaptcha?.requestToken || window.wbAliyunCaptcha.isBusy()) return false;
    // 不打断手动操作中的弹层；桌面最小化仍可无感验证，浏览器后台页则让给前台。
    if (window.workbuddyDesktop?.platform === 'web' && document.visibilityState !== 'visible') return false;
    return !document.querySelector('[data-slot="dialog-content"][data-open], [data-slot="alert-dialog-content"][data-open], [data-slot="sheet-content"][data-open]');
  }

  async function poll() {
    if (polling || stopped) return;
    const bridge = window.workbuddyDesktop;
    if (!bridge?.zcodeClaimVerification || !active && !hasAccounts()) {
      schedule(IDLE_MS);
      return;
    }
    polling = true;
    let delay = POLL_MS;
    try {
      const result = await bridge.zcodeClaimVerification({ action: 'poll', clientId, ready: !active && canVerify() });
      if (stopped) return;
      if (result?.supported === false) {
        delay = IDLE_MS;
        active?.controller.abort();
        return;
      }
      if (active && result?.task?.requestId !== active.task.requestId && !active.submitting) {
        active.expired = true;
        active.controller.abort();
      }
      if (!active && result?.task) void verify(result.task);
    } catch {
      // 网络与登录状态由主界面呈现；验证流程自己有截止时间，不刷重复错误提示。
      delay = IDLE_MS;
    } finally {
      polling = false;
      schedule(delay);
    }
  }

  async function verify(task) {
    if (!task.requestId || !task.accountId || !task.planId || !task.config) return;
    const remaining = Math.min(50000, Number(task.remainingMs) || 0);
    if (remaining <= 1000) return;
    const controller = new AbortController();
    const flow = { task, controller, expired: false, submitting: false };
    active = flow;
    const accounts = window.wbApp?.getState?.()?.accounts?.accounts || [];
    const account = accounts.find(account => account.id === task.accountId);
    const info = {
      requestId: task.requestId, accountId: task.accountId, planName: task.planName || task.planId,
      accountName: account?.displayName || account?.name || task.accountId, phase: 'preparing',
    };
    const setPhase = phase => { if (active === flow) publish({ ...info, phase }); };
    setPhase('preparing');
    const timeout = window.setTimeout(() => {
      flow.expired = true;
      controller.abort();
    }, remaining - 750);
    const complete = payload => new Promise((resolve, reject) => {
      const timer = window.setTimeout(() => reject(new Error(t('验证码业务请求超时，请刷新状态后重试'))), 10000);
      Promise.resolve().then(() => window.workbuddyDesktop.zcodeClaimVerification({
        action: 'complete', clientId, requestId: task.requestId,
        accountId: task.accountId, planId: task.planId, ...payload,
      })).then(resolve, reject).finally(() => window.clearTimeout(timer));
    });
    try {
      const token = await window.wbAliyunCaptcha.requestToken(task.config, controller.signal, { onPhase: setPhase });
      if (controller.signal.aborted || stopped || active !== flow) throw new DOMException('请求已取消', 'AbortError');
      flow.submitting = true;
      setPhase('submitting');
      const result = await complete({ status: 'verified', param: token.param });
      if (!result?.accepted && !stopped) {
        window.wbApp?.toast(t('领取验证任务已结束，请到套餐明细查看结果'), 'warn');
      }
    } catch (error) {
      pausedUntil = Date.now() + 5 * 60000;
      const cancelled = controller.signal.aborted || error?.name === 'CaptchaCancelledError' || error?.name === 'AbortError';
      if (!flow.submitting) {
        try {
          await complete({
            status: flow.expired ? 'timeout' : cancelled ? 'cancelled' : 'failed',
            message: cancelled ? '' : String(error?.message || error || '').slice(0, 240),
          });
        } catch {}
      }
      if (!stopped) window.wbApp?.toast(flow.submitting
        ? t('验证提交结果未确认，请查看签到记录，勿重复领取')
        : t('本次自动领取未完成验证，请在套餐明细中手动领取'), 'warn');
    } finally {
      window.clearTimeout(timeout);
      if (active === flow) {
        active = null;
        publish(null);
      }
      schedule(POLL_MS);
    }
  }

  function cancel() {
    if (active && !active.submitting) active.controller.abort();
  }

  function start() {
    stopped = false;
    schedule(1000);
  }

  function stop() {
    stopped = true;
    window.clearTimeout(timer);
    cancel();
  }

  window.wbZcodeClaimVerification = { cancel, status: () => value };
  window.addEventListener('pagehide', stop);
  window.addEventListener('pageshow', start);
  window.addEventListener('visibilitychange', () => { if (!stopped) schedule(0); });
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start, { once: true });
  else start();
})();
