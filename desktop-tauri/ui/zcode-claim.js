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

  const describeError = error => {
    if (error instanceof Error && error.message) return error.message;
    const text = String(error ?? '').trim();
    return text || wbI18n.t('未知错误');
  };

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

  /**
   * ② 领取一份套餐（要验证码）。
   *
   * `planId` 为空时由后端按优先级挑一份（与旧的默认目标同口径）。流程：
   * 先问上游**此刻要不要**人机验证（风控配置）—— 不要就直接领（后端允许不带
   * 验证码参数，只在该有值时发那个头），要就拉起阿里云滑块、拿到 verifyParam
   * 再领。硬弹一次滑块会让用户在本不需要验证的时候被拦一道，而且滑块可能根本
   * 初始化不出来。
   *
   * 业务结果（成功 / `already_claimed` / 其它失败）都走 `reportOutcome` 播报，
   * 并把后端那份结果送回调用方。
   */
  async function claim(accountId, planId) {
    const api = bridge();
    const id = String(accountId || '');
    if (!id) {
      window.wbApp.toast(wbI18n.t('账号信息不完整，请刷新后重试'), 'warn');
      return null;
    }
    if (!api?.zcodeClaimCaptchaConfig || !api?.zcodeClaim) {
      window.wbApp.toast(wbI18n.t('当前环境不支持领取（桥接方法缺失）'), 'warn');
      return null;
    }
    const target = String(planId || '');

    let captchaConfig = { enabled: false };
    try {
      captchaConfig = await api.zcodeClaimCaptchaConfig(id) || { enabled: false };
    } catch (error) {
      // 取配置失败不等于不能领：按「不需要验证码」试一次，上游要的话会回
      // 3007，那时再如实告诉用户「验证码校验未通过」
      window.wbApp.toast(wbI18n.t('获取风控配置失败，将直接尝试领取：{message}', { message: describeError(error) }), 'warn');
    }

    const callClaim = captchaVerifyParam => api.zcodeClaim(
      id,
      target,
      captchaVerifyParam || '',
      captchaConfig.region || '',
    );

    // 不需要验证码：一次请求就完事
    if (!captchaConfig?.enabled) {
      try {
        return reportOutcome(await callClaim(''));
      } catch (error) {
        window.wbApp.toast(wbI18n.t('领取失败：{message}', { message: describeError(error) }), 'warn');
        return null;
      }
    }

    // ── 需要验证码：滑块 → 拿 verifyParam 直接领取 ──────────
    // 注意与 AutoClaw 的 OAuth 不同 —— 那边拿到串之后是去换授权地址、再开窗口
    // 等回调（长流程，必须放在 solve 之外）；这里是**拿串直接领取**，全部动作
    // 都在 `request` 回调里完成，因此不存在「长流程被 120 秒验证码超时误杀」
    // 的问题（见 aliyun-captcha.js 里 solve 的说明）。
    const captcha = window.wbAliyunCaptcha;
    if (!captcha) {
      window.wbApp.toast(wbI18n.t('验证码组件未加载，请重启应用后重试'), 'warn');
      return null;
    }

    let outcome;
    try {
      outcome = await captcha.solve(
        {
          region: captchaConfig.region || 'ga',
          prefix: captchaConfig.prefix,
          sceneId: captchaConfig.sceneId,
        },
        async captchaVerifyParam => {
          const result = await callClaim(captchaVerifyParam);
          // 业务结果借返回值带回上层（`solve` 会原样 resolve 它）
          return { captchaResult: true, bizResult: true, result };
        },
      );
    } catch (error) {
      // 用户主动取消验证码不算失败（与 AutoClaw 登录那边同一处置）
      if (error instanceof captcha.CaptchaCancelledError) {
        window.wbApp.toast(wbI18n.t('已取消领取'));
        return null;
      }
      window.wbApp.toast(wbI18n.t('领取失败：{message}', { message: describeError(error) }), 'warn');
      return null;
    }
    return reportOutcome(outcome?.result);
  }

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

  // 「这个账号有没有领取能力」的判据**不在这里** —— 它走账号页的能力表
  // （accounts-domain.ts 的 `claim` 位 + 后端给的 `canClaim`，见 `supportsClaim`）。
  // 本模块只负责「探测一次」与「领一份」。
  window.wbZcodeClaim = { preview, claim };
})();
