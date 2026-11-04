/* 套餐池状态守卫只轮询；验证码只能由用户主动发起。 */
(() => {
  const POLL_MS = 2500;
  const IDLE_POLL_MS = 15000;
  let timer = 0;
  let stopped = false;
  let running = false;
  let verifying = false;
  let stats = null;
  let status = { state: 'idle', error: '' };

  function publish(state, error = '') {
    const message = String(error || '');
    if (status.state === state && status.error === message) return;
    status = { state, error: message };
    window.dispatchEvent(new CustomEvent('zcode-captcha-status', { detail: { ...status } }));
  }

  function schedule(delay) {
    window.clearTimeout(timer);
    if (!stopped) timer = window.setTimeout(() => void check(), delay);
  }

  async function readStats() {
    const bridge = window.workbuddyDesktop;
    if (!bridge?.zcodeCaptchaStats) throw new Error(wbI18n.t('验证码令牌池接口不可用'));
    const value = await bridge.zcodeCaptchaStats();
    if (value?.error) throw new Error(String(value.error));
    if (!value) throw new Error(wbI18n.t('验证码令牌池未返回状态'));
    stats = value;
    return value;
  }

  function stateOf(value) {
    if (value.configError) return publish('error', value.configError);
    if (value.required === false) return publish('disabled');
    if (!Number(value.startPlanAccounts)) return publish('idle');
    if (value.needsTokens === true) return publish('needs_verification');
    return publish('idle');
  }

  async function check() {
    if (running || stopped) return;
    // 令牌池只服务账号页的「套餐」列：人不在账号页时既不需要读状态，也不需要
    // 每 15 秒打一发管理接口。留在定时器上保持节拍，切回账号页时由 wb:page
    // 事件立即催一次。
    if ((window.wbApp?.currentPage || '') !== 'accounts') {
      schedule(IDLE_POLL_MS);
      return;
    }
    running = true;
    try {
      const value = await readStats();
      if (!verifying && !['paused', 'error'].includes(status.state)) stateOf(value);
      const poll = Number(value.pollMs) || POLL_MS;
      schedule(value.demandActive === true ? Math.min(30000, Math.max(1000, poll)) : IDLE_POLL_MS);
    } catch (error) {
      if (!verifying) publish('error', error?.message || error);
      schedule(IDLE_POLL_MS);
    } finally {
      running = false;
    }
  }

  async function verify(accountId, signal, onPhase) {
    if (verifying) throw new Error(wbI18n.t('已有验证码流程正在进行，请先完成或取消'));
    const bridge = window.workbuddyDesktop;
    if (!accountId || !bridge?.zcodeClaimCaptchaConfig || !bridge?.pushZcodeCaptchaTokens) {
      throw new Error(wbI18n.t('验证码令牌入池接口不可用'));
    }
    verifying = true;
    publish('verifying');
    try {
      const requireActive = () => {
        if (signal?.aborted) throw new DOMException('请求已取消', 'AbortError');
      };
      requireActive();
      onPhase?.('preparing');
      const config = await bridge.zcodeClaimCaptchaConfig(accountId);
      requireActive();
      const currentStats = await readStats();
      requireActive();
      if (currentStats.required === false) {
        stateOf(currentStats);
        return false;
      }
      const captcha = window.wbAliyunCaptcha;
      if (!captcha?.requestToken) throw new Error(wbI18n.t('验证码组件未加载，请重启应用后重试'));
      const token = await captcha.requestToken(config, signal, { onPhase });
      requireActive();
      onPhase?.('submitting');
      const outcome = await bridge.pushZcodeCaptchaTokens([{ param: token.param, region: config.region || '' }]);
      if (Number(outcome?.accepted) !== 1) throw new Error(wbI18n.t('验证码令牌未全部入池'));
      stats = outcome;
      publish('idle');
      return true;
    } catch (error) {
      const cancelled = error?.name === 'AbortError' || error?.name === 'CaptchaCancelledError';
      publish(cancelled ? 'paused' : 'error', cancelled ? '' : error?.message || error);
      throw error;
    } finally {
      verifying = false;
    }
  }

  function start() {
    stopped = false;
    schedule(1500);
  }

  function stop() {
    stopped = true;
    window.clearTimeout(timer);
  }

  window.wbZcodeCaptchaPool = { start, stop, verify, refresh: readStats, status: () => ({ ...status }), stats: () => stats };
  // 切到账号页时立刻补一次：页面首次打开或从别的页切回来都走这一条。
  window.addEventListener('wb:page', event => {
    if (event.detail === 'accounts' && !stopped) schedule(0);
  });
  window.addEventListener('pagehide', stop);
  window.addEventListener('pageshow', start);
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start, { once: true });
  else start();
})();
