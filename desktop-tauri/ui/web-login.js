/* Agent2API · 「网页登录」交互引擎（WorkBuddy 与小浣熊共用）

   两家 provider 的网页登录在界面上是同一种交互：一个「打开网页登录」按钮、
   一个「取消等待」按钮、一行 hint，等待期间按钮禁用并转圈，成功后关弹窗刷新列表。
   差别只有三处（发起时传什么参数、按钮/提示的文案、成功后提示什么），
   因此这里把**流程**实现一次，各家只交一份配置（见 create 的 config 说明）。
   为什么不是复制两份：等待态与按钮复位必须与主进程的真实状态对齐
   （applyLoginState 的注释解释了这点），两份实现迟早会在某一次改动后分叉，
   而分叉的表现是「关掉弹窗后按钮永远卡在禁用态」这种很难复现的故障。

   依赖 app.js 的顶层全局（经典 script 的顶层声明在全局可见）：toast / busy /
   releaseBusy。脚本在页面的 DOMContentLoaded 之前就位，表单挂载后建立控制器。

   与主进程的契约：同一时刻只允许一个登录流程（壳侧 start_login 会拒绝第二个），
   因此这里维护一份**全局**的 loginActive / loginProvider，而不是每个控制器各存一份；
   某一家在等待时，另一家的发起按钮也要禁用（点下去必然报错）。 */

(() => {
  const $ = id => document.getElementById(id);
  const { toast } = window.wbApp;

  /** 所有已登记的控制器（按 create 顺序） */
  const controllers = [];
  /** 是否有登录流程在等待（主进程的真实状态，见 applyShellState） */
  let loginActive = false;
  /** 等待中的那家 provider；主进程状态里没带 provider 时按 workbuddy 兜底 */
  let loginProvider = '';
  /** 一次 start() 正在收尾：此时主进程状态可能还没回到「空闲」，
   *  期间任何「顺手取消」（关弹窗时）都必须让路 —— 否则会把一次
   *  刚成功的登录取消掉，界面上表现为「登录成功却提示已取消」。 */
  let flowSettling = false;
  /** 只记公开说明与进度，不存授权 URL、回调或 inputs。 */
  let loginInstructions = '';
  let loginHint = '';

  const plainText = value => typeof value === 'string' ? value : '';
  const pendingResult = value => value && typeof value === 'object'
    && (value.pending === true || (value.pending && typeof value.pending === 'object')
      || value.accepted === true || value.done === false || value.state === 'pending'
      || ((value.flow || Object.prototype.hasOwnProperty.call(value, 'url')
        || Object.prototype.hasOwnProperty.call(value, 'authUrl')) && value.done !== true && value.ok !== true));

  // 注册名（「AutoClaw 国内版」这类）过一遍词典再进文案，非中文界面才翻得动
  const providerLabel = id => {
    const name = window.wbProviders?.labelOf?.(id) || id;
    return window.wbI18n?.t?.(name) || name;
  };

  /**
   * 建一个登录控制器。
   *
   * config：
   *   provider   本控制器对应的 provider id（与壳侧 start_login 的参数一致）
   *   buttonId / cancelId / hintId   三个 DOM id（各家的块用前缀区分）
   *   texts()    返回 { button, hint }：当前该显示的按钮文案与提示。
   *              WorkBuddy 的文案随打开方式变化，具体值由表单提供。
   *   onStateChange(state)  React 表单接收等待态，不让引擎写入它的子节点。
   *                        未提供时保留按 DOM id 同步的兼容路径。
   *   busyText   等待中的按钮文案，转圈由表单中的 Spinner 渲染
   *   start()    真正发起登录，返回壳命令的结果（`{canceled:true}` 表示用户取消）
   *   onSuccess(result)  登录成功后的收尾（关弹窗、刷新列表、提示）
   */
  function create(config) {
    const button = () => $(config.buttonId);
    const cancelButton = () => $(config.cancelId);
    const hint = () => $(config.hintId);

    let starting = false;
    // 成功路径的取消请求通常由关闭弹窗顺手触发，等待其收尾前不得重入。
    const publish = state => {
      if (config.onStateChange) {
        config.onStateChange(state);
        return;
      }
      const button_ = button();
      if (button_) {
        button_.disabled = state.disabled;
        button_.textContent = state.label;
      }
      const cancel_ = cancelButton();
      if (cancel_) cancel_.hidden = !state.waiting;
      const hint_ = hint();
      if (hint_) {
        hint_.textContent = state.hint;
        hint_.style.whiteSpace = 'pre-wrap';
      }
    };

    /** 同一时刻只能有一家等待登录，其余入口同步禁用。 */
    const applyState = () => {
      const texts = config.texts ? config.texts() : { button: '', hint: '' };
      const waiting = starting || (loginActive && loginProvider === config.provider);
      const progress = loginActive && loginProvider === config.provider
        ? [...new Set([loginInstructions, loginHint].flatMap(text => text.split(/\r?\n/)).map(line => line.trim()).filter(Boolean))].join('\n') : '';
      publish({
        label: waiting ? (config.busyText || wbI18n.t('等待登录完成…')) : (texts.button || ''),
        hint: loginActive && loginProvider !== config.provider
          ? wbI18n.t('正在等待{name}登录完成；完成或取消后才能发起新的登录', {
            name: providerLabel(loginProvider),
          })
          : (progress || texts.hint || ''),
        disabled: waiting || loginActive,
        waiting,
      });
    };

    const controller = {
      get provider() { return config.provider; },
      /** 提示与按钮文案随本家的其它控件变化时调用（等待中不覆盖，见 applyState） */
      syncTexts: () => {
        if (!loginActive && !starting) applyState();
      },
      applyState,
      /** 发起登录；与 takeBusy 同一把锁（弹窗里的按钮互斥） */
      async start() {
        if (busy || loginActive || starting || flowSettling) return;
        busy = true;
        starting = true;
        applyState();
        try {
          const result = await config.start();
          if (result?.canceled) {
            // silent：取消是别处（「取消等待」按钮 / 关闭添加账号弹窗）发起的，
            // 那边已经提示过，这里不再重复 toast
            if (!result.silent) toast(wbI18n.t('已取消登录等待'));
            return;
          }
          // 结果归一由共享 UI bridge 负责；这里只守终态，防止空响应或 accepted
          // 被旧控制器的 onSuccess 误报成账号已添加，不改桌面 payload/Web session。
          if (!result || pendingResult(result) || pendingResult(result.payload)) {
            throw new Error('登录尚未完成，请继续等待网关确认');
          }
          if (result.ok === false) throw new Error('登录桥没有确认登录完成');
          flowSettling = true;
          await config.onSuccess(result);
        } catch (error) {
          toast(wbI18n.t('登录失败：{message}', { message: error.message }), 'err');
        } finally {
          starting = false;
          flowSettling = false;
          releaseBusy();
          // 以主进程状态为准复位按钮，避免这里与真实状态不一致
          await syncShellState();
        }
      },
      /** 取消等待（本家在等待时才发请求） */
      async cancel() {
        if (flowSettling || !loginActive || loginProvider !== config.provider) {
          applyState();
          return;
        }
        const canceled = await cancelLogin();
        toast(canceled ? wbI18n.t('已取消登录等待') : wbI18n.t('登录已结束'));
        await syncShellState();
      },
      dispose() {
        const index = controllers.indexOf(controller);
        if (index >= 0) controllers.splice(index, 1);
      },
    };
    controllers.push(controller);
    applyState();
    return controller;
  }

  /** 通知主进程取消等待中的登录（不区分是哪一家：主进程只记一个活动登录） */
  async function cancelLogin() {
    try {
      await window.workbuddyDesktop.cancelLogin();
      return true;
    } catch (error) {
      toast(wbI18n.t('取消失败：{message}', { message: error.message }), 'err');
      return false;
    }
  }

  /** 把主进程的登录状态落到所有控制器 */
  function applyShellState(active, provider, progress) {
    const nextActive = Boolean(active);
    const nextProvider = nextActive ? (provider || 'workbuddy') : '';
    if (!nextActive || nextProvider !== loginProvider || !loginActive) {
      loginInstructions = '';
      loginHint = '';
    }
    loginActive = nextActive;
    // 老版本壳不带 provider 字段：那时只有 workbuddy 一家有网页登录。
    loginProvider = nextProvider;
    if (nextActive && progress) {
      if (Object.prototype.hasOwnProperty.call(progress, 'instructions')) {
        loginInstructions = plainText(progress.instructions);
      }
      if (Object.prototype.hasOwnProperty.call(progress, 'hint')) loginHint = plainText(progress.hint);
    }
    for (const controller of controllers) controller.applyState();
  }

  /** 问一次主进程的真实状态并落定（弹窗打开、流程结束时调用）。

      名字不叫 refresh：那个名字在 app.js 里是「刷新账号列表」，两者都会在
      同一个函数里被调用，重名会让读者以为是同一件事。 */
  async function syncShellState() {
    try {
      const state = await window.workbuddyDesktop.getLoginState();
      applyShellState(state?.active, state?.provider, state);
    } catch {
      applyShellState(false, '');
    }
  }

  window.wbWebLogin = {
    create,
    refresh: syncShellState,
    isActive: () => loginActive,
    activeProvider: () => loginProvider,
    /**
     * 放弃等待中的登录。给定 provider 时只取消该家的流程，避免干扰其它提供商。
     * 返回是否真的发了取消请求。
     */
    async cancelIfActive(provider) {
      if (!loginActive || flowSettling) return false;
      if (provider && loginProvider !== provider) return false;
      const canceled = await cancelLogin();
      await syncShellState();
      return canceled;
    },
  };

  // 登录进行状态由主进程推送：等待结束（成功/失败/取消）后按钮自动复位
  window.workbuddyDesktop.onLoginState?.(payload =>
    applyShellState(payload?.active, payload?.provider, payload));
  // 不改变旧 LoginState 查询形状；独立进度事件只更新仍在等的那一轮。
  window.workbuddyDesktop.onLoginProgress?.(payload => {
    if (loginActive && payload?.provider === loginProvider) {
      applyShellState(true, loginProvider, payload);
    }
  });
})();
