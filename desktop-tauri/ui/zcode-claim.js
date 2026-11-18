/* Agent2API · ZCode「领套餐」的探测与领取（网络侧；界面在套餐明细弹窗里）

   ── 这一家为什么没有「签到」而是这个 ────────────────────────
   其余各家的运营玩法是每日签到（见 core/auto_checkin），ZCode 是**限时发放的
   体验套餐**（官方叫 start-plan，客户端里点一下就领）。因此本家在账号行与
   签到中心给的都是「领套餐」而不是「签到」。

   ── 每日一期：2026-09-28 起那期（ZCode Trust Build）────────
   活动窗口里**每天换一个新套餐**（plan_id 带日期段：…-trust-0928 → …-0929），
   所以活动期内每天都能领一次。而「已领取过」是上游**按套餐**判的：同一份当天
   再领回 1003，换一份（比如每日额度那份）照样能领 —— 因此状态只能逐份比。
   逐份比对与日界判定在账号页域层（`claimedPlanIdsToday`），本模块不抄一份。

   ── 两步，而且只有第二步要验证码 ────────────────────────────
     ① 探测  POST /api/accounts/{id}/zcode-claim/preview   —— 不要验证码
     ② 领取  POST /api/accounts/{id}/zcode-claim           —— **要**验证码

   探测在**结果为空**时会先补报一次激活事件（`app_launch` / `app_daily_active`）
   再重探一遍 —— 领取资格由激活事件发放，而本网关不跑官方客户端，那一步不会自动
   发生（见后端 `providers::zcode::activation`）。所以「探测」在那一档不是纯只读；
   响应里的 `activated` / `activationError` 如实说明这次报了没报，界面不读它也不影响。

   上游的领取接口通常要求 `X-Aliyun-Captcha-Verify-Param`（阿里云无痕验证），
   而解它的唯一可行方式是在 webview 里跑阿里云官方 SDK —— 那正是
   `ui/aliyun-captcha.js` 提供的东西（同一个求解器也给 AutoClaw 的 OAuth 用）。
   要不要弹滑块由上游的风控配置说了算（见 `claim` 的注释）。

   ── 为什么探测与领取要分开暴露 ──────────────────────────────
   探测不需要验证码，所以「当前有没有可领的套餐」这条信息是**免费**的；而一旦
   开始领取就要用户拖一次滑块。界面（`wbZcodePlans` 套餐明细弹窗）因此在打开时
   就调 `preview` 把可领清单画出来、由用户逐份点「领取」才走 `claim` ——
   本模块只做网络与验证码，**不再自己弹确认框**（旧版那套「探测 → 原生确认框 →
   领取」已拆掉：确认框里既看不到逐份状态、也显示不了刚领到还没生效的套餐）。

   ── 404 与「业务失败」都不是错误（后端已归一）───────────────
     · `deployed: false`（上游活动接口未部署）→ 说一句「当前没有可领套餐」；
     · 领取返回 `ok: false` + `failure`（如 `already_claimed` / `quota_exhausted`
       / `captcha`）→ 按 `failureLabel` 提示。这两种都不该报红。
       只有 HTTP 非 2xx（账号不存在 / 缺 jwt / 网络故障）才当失败处理。

   ── 返回值：把业务结果交回调用方 ────────────────────────────
   `preview()` 把后端的探测响应原样 return、失败时 **throw**（界面把错误渲染在
   弹窗里，不吞成一句 toast：用户正看着那份清单）；`claim()` 返回本次的业务结果
   （`{ok, failure, planId}` 或 null），调用方据此决定要不要刷余额与账号状态。

   依赖：`window.workbuddyDesktop`（桥接方法，见 web_shim.rs 的
   zcodeClaim* 三个）、`window.wbAliyunCaptcha`、`window.wbApp.toast`。
   脚本顺序见 index.html。 */
(() => {
  const bridge = () => window.workbuddyDesktop;

  /** unix 秒 → 本地可读时间（后端给的一律是秒，不是毫秒） */
  function formatTime(seconds) {
    const value = Number(seconds);
    if (!Number.isFinite(value) || value <= 0) return '';
    try {
      return new Date(value * 1000).toLocaleString();
    } catch {
      return '';
    }
  }

  /**
   * ① 探测**可领取**的套餐（不要验证码）。
   *
   * 成功返回后端那份响应（`{deployed, plans, activated, activationError}`），
   * 失败（网络 / 非 2xx，例如账号没有 jwt）**原样抛给调用方** —— 界面把这句话
   * 渲染在弹窗里，比吞成 toast 更好排查。
   */
  async function preview(accountId) {
    const api = bridge();
    const id = String(accountId || '');
    if (!id) throw new Error(wbI18n.t('账号信息不完整，请刷新后重试'));
    if (!api?.zcodeClaimPreview) throw new Error(wbI18n.t('当前环境不支持领取（桥接方法缺失）'));
    return api.zcodeClaimPreview(id);
  }

  const validProvider = value => value === 'zcode' || value === 'zcode-intl';
  const validText = value => typeof value === 'string' && value.trim() && !/[\u0000-\u001f\u007f]/.test(value);
  const manual = new Set();
  let generation = 0;

  function bindingOf(config, accountId) {
    if (!config || typeof config !== 'object' || Array.isArray(config) || typeof config.enabled !== 'boolean') throw new Error('invalid_claim_config');
    if (config.enabled && (!validText(config.prefix) || !validText(config.sceneId) || !validText(config.region))) throw new Error('invalid_claim_config');
    if (!['provider', 'accountId', 'configId', 'engine'].some(key => Object.prototype.hasOwnProperty.call(config, key))) return null;
    if (!validProvider(config.provider) || config.accountId !== accountId || !validText(config.configId)
      || (config.enabled ? config.engine !== 'aliyun' : !['none', 'aliyun'].includes(config.engine))) throw new Error('invalid_claim_binding');
    return { provider: config.provider, accountId, configId: config.configId };
  }

  // 只在已明确指定账号与套餐的用户操作中启动，不缓存验证结果或重试领取。
  async function claim(accountId, planId, signal, onPhase) {
    const api = bridge();
    const id = String(accountId || '');
    const target = String(planId || '');
    if (!validText(id) || !validText(target)) {
      window.wbApp?.toast(wbI18n.t('账号或套餐信息不完整，请刷新后重试'), 'warn');
      return null;
    }
    if (!api?.zcodeClaimCaptchaConfig || !api?.zcodeClaim) {
      window.wbApp?.toast(wbI18n.t('当前环境不支持领取（桥接方法缺失）'), 'warn');
      return null;
    }
    const controller = new AbortController();
    const flow = { api, controller, epoch: generation, origin: window.location.origin };
    const initialAccounts = window.wbApp?.getState?.()?.accounts?.accounts;
    const initial = Array.isArray(initialAccounts) ? initialAccounts.find(account => account.id === id) : null;
    let provider = initial?.provider;
    let submitted = false;
    manual.add(flow);
    const cancel = () => controller.abort();
    const guard = () => {
      if (signal?.aborted || flow.epoch !== generation || flow.api !== bridge() || flow.origin !== window.location.origin) { cancel(); return; }
      const accounts = window.wbApp?.getState?.()?.accounts?.accounts;
      if (!Array.isArray(accounts)) { cancel(); return; }
      const account = accounts.find(item => item.id === id);
      if (!account || account.available === false || !validProvider(account.provider)
        || provider !== undefined && provider !== account.provider) cancel();
      else provider = account.provider;
    };
    const requireCurrent = () => { guard(); if (controller.signal.aborted) throw new DOMException('请求已取消', 'AbortError'); };
    const phase = value => { requireCurrent(); onPhase?.(value); };
    signal?.addEventListener('abort', cancel, { once: true });
    const unsubscribe = window.wbApp?.subscribeRefresh?.(guard);
    try {
      requireCurrent();
      phase('preparing');
      const config = Object.freeze({ ...await api.zcodeClaimCaptchaConfig(id) });
      requireCurrent();
      const binding = bindingOf(config, id);
      if (binding && provider !== undefined && provider !== binding.provider) throw new Error('claim_provider_changed');
      if (binding) provider = binding.provider;
      let param = '';
      if (config.enabled) {
        const captcha = window.wbAliyunCaptcha;
        if (!captcha?.requestToken) throw new Error('captcha_unavailable');
        const token = await captcha.requestToken(config, controller.signal, { onPhase: phase });
        requireCurrent();
        if (!validText(token?.param)) throw new Error('invalid_captcha_token');
        param = token.param;
      }
      requireCurrent();
      phase('submitting');
      submitted = true;
      const result = binding
        ? await api.zcodeClaim(id, { ...binding, planId: target, captchaVerifyParam: param, captchaRegion: config.region || '' })
        : await api.zcodeClaim(id, target, param, config.region || '');
      requireCurrent();
      if (!result || typeof result.ok !== 'boolean' || result.planId && result.planId !== target) throw new Error('claim_result_unconfirmed');
      return reportOutcome(result);
    } catch (error) {
      guard();
      const cancelled = controller.signal.aborted || ['AbortError', 'CaptchaCancelledError'].includes(error?.name);
      if (flow.epoch !== generation || flow.api !== bridge() || flow.origin !== window.location.origin) return null;
      if (cancelled && !submitted) {
        window.wbApp?.toast(wbI18n.t('已取消领取'));
        return null;
      }
      const message = submitted ? wbI18n.t('领取提交结果未确认，请查看签到记录，勿重复领取')
        : wbI18n.t('领取验证未完成，请刷新套餐配置后重试');
      window.wbApp?.toast(message, 'warn');
      return null;
    } finally {
      manual.delete(flow);
      signal?.removeEventListener('abort', cancel);
      unsubscribe?.();
    }
  }

  window.addEventListener('pagehide', () => {
    generation++;
    for (const flow of manual) flow.controller.abort();
  });

  /**
   * 领取结果 → 提示 + 返回值。
   *
   * 业务失败按后端给的 `failureLabel` 提示（中文已由后端归一，前端不再按
   * `failure` 键自己写一套文案 —— 那样两处会漂移）。
   * 返回值原样透出后端那份结果（弹窗与账号页据此刷余额 / 逐份状态）。
   */
  function reportOutcome(result) {
    if (!result) return null;
    if (result.ok) {
      const window_ = [formatTime(result.startsAt), formatTime(result.endsAt)]
        .filter(Boolean)
        .join(' → ');
      window.wbApp.toast(
        wbI18n.t('✅ 领取成功：{planId}{period}', {
          planId: result.planId || '',
          // 括起来的是**上游给的这段有效期**，别省掉「有效期」三个字：这个时刻来自
          // 领取接口的 starts_at，与「什么时候开始生效」不是一回事（活动套餐常常
          // 先领、当晚 23:00 才生效，那个时间在账号页的「套餐明细」里，按权益的
          // effective_at 显示）。少了这三个字会被读成「现在生效了」。
          period: window_ ? wbI18n.t('（有效期 {window}）', { window: window_ }) : '',
        }),
      );
      // 领到的额度**不在**默认那条转发通道上（编码套餐走开放平台、活动套餐走
      // 官方活动端点），所以顺手说一句去哪儿切 —— 否则用户领完直接发请求，会
      // 拿到「套餐已到期」而完全不知道这回事（见 providers::zcode::plan）
      window.wbApp.toast(
        wbI18n.t('这份额度走「活动套餐」通道：若转发时提示编码套餐已到期，'
          + '在账号设置里把「使用套餐」切到活动套餐即可'),
      );
      return result;
    }
    const label = result?.failureLabel || wbI18n.t('未领取成功');
    window.wbApp.toast(`${label}${result?.message ? `：${result.message}` : ''}`, 'warn');
    return result;
  }

  // 「这个账号有没有领取能力」的判据**不在这里** —— 入口由签到中心的「活动福利」卡
  // 给出（判据是后端插件申报的 `plans` 位，见 api::checkin_center）。
  // 本模块只负责「探测一次」与「领一份」。
  window.wbZcodeClaim = { preview, claim };
})();
