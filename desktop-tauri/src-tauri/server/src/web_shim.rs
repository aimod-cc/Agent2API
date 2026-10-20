//! 网页端桥接（headless 托管面板时注入的 `window.workbuddyDesktop` 实现）。
//!
//! ── 为什么需要它 ────────────────────────────────────────────
//! 界面代码（desktop-tauri/ui/）只依赖 `window.workbuddyDesktop` 这个接口，
//! 桌面壳由 `bridge.rs` 在页面脚本执行前注入同名对象（Tauri IPC → 壳 → 网关）。
//! headless 形态没有壳：本模块生成一份**纯 HTTP** 的同接口实现，由
//! `static_files` 注入进 index.html —— 界面代码零改动地跑在浏览器里，
//! 「界面不感知壳」的既有设计在这里兑现。
//!
//! ── 与桌面 bridge 的三条差异（其余方法一一对应）─────────────
//!   · 传输：`call` 直接 `fetch` 同源 `/api/*`，自动带 `x-api-key`；
//!     信封解包（`{success, data}` / 401）与桌面 `gateway::unwrap_envelope`
//!     逐条对齐，界面的 try/catch 语义不变。
//!   · 凭证：桌面壳自动带第一把 Key（进程内读）；网页端由**用户输入**
//!     （401 时弹出输入层，存 localStorage 后重试一次）。
//!   · 事件：Tauri 的 push 事件不存在，改为**模拟** ——
//!     `accounts:state-changed` 在写类请求成功后拉一次 /api/session 重放
//!     （300ms 防抖，合并连续写）；`login:state` 由本文件的登录状态机
//!     本地驱动；`backend:error` / `accounts:auto-maintained` 没有对应物，
//!     订阅返回空操作（桌面端它们也只是「去查一次」的提醒）。
//!
//! ── 本地 headless 与远程部署的分界（`desktopAccess`）─────────
//! 面板有两种宿主：① Docker / 公网服务器（访问者远程，`platform` 保持
//! `'web'`，所有壳侧能力收起）；② 本地 Windows 下载 zip 双击运行
//! （下载者就在这台机器上，回环访问）。后者的面板进程与桌面壳进程
//! 一样**能读本机桌面客户端的登录态文件**（服务端实现含 DPAPI，本来
//! 就在网关进程里跑），所以入口在「只听回环」时调
//! [`set_desktop_access`]，把 `desktopAccess: true` 注入桥接对象；
//! 界面侧据此单独放行「导入桌面端登录态」，其余 web 裁剪照旧。
//! 未调用时默认 false（保守：桌面壳不走本模块，远程 headless 必须显式
//! 声明才放行）。
//!
//! ── 登录链路在网页端的形态 ──────────────────────────────────
//! 桌面的 startLogin 是「壳开窗口 + 壳捕获回调」；网页端改为
//! `POST /api/session/login/start` 拿 authUrl → 打开新标签 → 轮询
//! `GET /api/session/login/wait?state=` 直到 done（与桌面壳的等待语义一致，
//! 返回最终 session；取消/错误原样上抛）。适用面（对齐各上游回调机制）：
//!   · WorkBuddy / Qoder / Cline：设备授权轮询，网页端完全可用；
//!   · AutoClaw（OAuth）：优先接收 loopback 回调，远程面板由网页端粘贴
//!     最终回调地址兜底；CatPaw 由服务端 poll-token 兜底；
//!   · 小浣熊：网页端把授权页切换到官方 `redirect` 分支，直接回到网关的
//!     HTTP 回调 —— 前提是浏览器以回环地址访问面板（上游白名单只收
//!     http://127.0.0.1 / localhost / [::1] + 显式端口，见
//!     `raccoonCallbackOriginProblem`），不满足时发起前就弹指引拦下；
//!     Trae / Accio / CodeArts / Antigravity：网页端自动提供“粘贴回调地址”兜底。
//!
//! ── 壳特有命令的降级 ────────────────────────────────────────
//! 窗口主题、托盘、改端口、软件更新安装、桌面设置、文件对话框导入导出
//! 这些只有壳能做的事按「能映射则映射，不能则明确拒绝」处理：
//! 拒绝给出可读文案（界面会 toast 出来），绝不静默假成功。

use std::sync::OnceLock;

/// 「本面板进程能读本机桌面客户端登录态」的部署级声明（模块头有完整说明）。
/// 由 headless 入口在解析监听地址后调用一次；未调用 = 远程部署，不放行。
static DESKTOP_ACCESS: OnceLock<bool> = OnceLock::new();

/// 声明面板进程能否读本机桌面客户端的登录态（重复调用忽略，先到为准）。
pub fn set_desktop_access(value: bool) {
    let _ = DESKTOP_ACCESS.set(value);
}

/// 当前部署是否放行壳侧的本机能力（见 [`set_desktop_access`]）。
pub fn is_desktop_access() -> bool {
    DESKTOP_ACCESS.get().copied().unwrap_or(false)
}

/// 装配好的桥接脚本（`SHIM_TEMPLATE` 替换掉占位符之后的结果）。
///
/// 缓存的理由：脚本本身有十来万字符，而 `static_files` 每命中一次
/// index.html 都要取它 —— 不缓存就是每次页面加载都整串 replace 一遍。
/// 取值只取决于 [`DESKTOP_ACCESS`]，它在启动时置位、之后不再变，所以
/// 「第一次取用时算一次」与「每次现算」结果恒等。
static SHIM_JS: OnceLock<String> = OnceLock::new();

/// 注入进 index.html 的桥接脚本全文（在所有界面脚本之前执行）。
///
/// 用 `r#"…"#` 原始字符串：脚本里不出现 `"#` 序列（字符串一律单引号），
/// 不需要任何转义。唯一的占位符 `__DESKTOP_ACCESS__` 在装配时按
/// [`is_desktop_access`] 替换成 `true` / `false` 字面量。
pub fn shim_js() -> &'static str {
    SHIM_JS.get_or_init(|| {
        let flag = if is_desktop_access() { "true" } else { "false" };
        SHIM_TEMPLATE.replace("__DESKTOP_ACCESS__", flag)
    })
}

/// 桥接脚本模板（内容见 [`shim_js`]）。
const SHIM_TEMPLATE: &str = r#"(function () {
  'use strict';

  // ── 清掉登录页转来的放行标记（`?from=login`）─────────────
  // 服务端在 `/` 上会拦「会话无效的导航」并送回登录页（见
  // server::http::panel_boot_redirect）。中转环境里会话只存在于 localStorage
  // 的 `x-panel-token` 头，而顶层导航带不了自定义头，所以登录页跳过来时带这个
  // 标记、服务端放行这一趟。标记必须在页面跑起来后从地址栏抹掉：留着它，
  // 会话过期后用户刷新页面仍会绕过那道判定，进去又是一屏 401。
  (function () {
    try {
      var url = new URL(window.location.href);
      if (!url.searchParams.has('from')) return;
      url.searchParams.delete('from');
      var search = url.searchParams.toString();
      window.history.replaceState(null, '',
        url.pathname + (search ? '?' + search : '') + url.hash);
    } catch (e) { /* 老浏览器 / 被禁用：留着标记只是多一次放行，不影响正确性 */ }
  })();

  // ── API Key 的存取 ────────────────────────────────────────
  var KEY_STORAGE = 'agent2api.webKey';
  function readStoredKey() {
    try { return localStorage.getItem(KEY_STORAGE) || ''; } catch (e) { return ''; }
  }
  var apiKey = readStoredKey();
  function storeKey(value) {
    apiKey = value;
    try {
      if (value) localStorage.setItem(KEY_STORAGE, value);
      else localStorage.removeItem(KEY_STORAGE);
    } catch (e) { /* 隐私模式等：留在内存即可 */ }
  }

  // ── 面板双令牌的本地保管（PANEL_AUTH）──────────────────────
  // HttpOnly cookie 在「宿主中转」入口（fnOS docker 管理页这类面板跳板）
  // 下发/回带都会被掐掉：登录 POST 能到、Set-Cookie 回不来，主页第一个
  // 请求就 401 被踢回登录页。因此登录/刷新接口把裸令牌同时放进响应体
  // （见 panel.rs issue_response），前端存这里，后续请求带
  // `x-panel-token` / `x-panel-refresh` 头。直连环境下 cookie 照常下发
  // 且服务端优先认它，这套头是纯增量，互不干扰。
  var PANEL_AUTH = 'agent2api.panelAuth';
  function readPanelAuth() {
    try { return JSON.parse(localStorage.getItem(PANEL_AUTH) || 'null'); }
    catch (e) { return null; }
  }
  function storedPanelToken(kind) {
    var auth = readPanelAuth();
    var value = auth && auth[kind];
    if (typeof value === 'string' && value) return value;
    return '';
  }
  function storePanelAuth(access, refresh) {
    try {
      localStorage.setItem(PANEL_AUTH, JSON.stringify({ access: access, refresh: refresh }));
    } catch (e) { /* 隐私模式等：留在本次调用链即可 */ }
  }
  function clearPanelAuth() {
    try { localStorage.removeItem(PANEL_AUTH); } catch (e) { /* 无害 */ }
  }

  // ── 覆盖层（Key 输入 / 链接兜底 / 粘贴回调）：原生 DOM，界面样式不依赖 ──
  // options（可选）：
  //   cancelText  给了就多渲染一个次要按钮；点它或按 Esc 以 null 收尾 ——
  //               调用方据此区分「确认」（确认值永远不是 null）与「取消」。
  //   onMount(body, controls)  卡片挂上 DOM 后回调一次：正文是 HTML 字符串，
  //               要给它里面的按钮挂监听只有这一次机会；controls.cancel()
  //               让外部（例如弹窗关闭时的取消通知）也能以取消收尾。
  //
  // 带 cancelText 的覆盖层会登记到 activeOverlay：`cancelLogin` 与
  // `closeWebOverlay` 都要能拿到它（前者让用户点「取消等待」时弹层立刻收尾，
  // 后者负责在流程结束时摘掉 document 上的 Esc 监听 —— 不摘的话，那个监听
  // 会在弹层早已消失后继续响应 Esc，把上一轮残留的 Promise 解析掉）。
  var activeOverlay = null;
  // 被流程收尾收掉的弹层用这个哨兵值收尾：它既不是用户的输入，也不算取消
  var DISPOSED = {};
  function ensureOverlay(titleText, bodyHtml, confirmText, options) {
    options = options || {};
    // 先摘掉上一张（可能带着 cancelText 与 document 级 Esc 监听）。
    // 用 dispose 而不是 cancel：被新弹层替换 ≠ 用户点了取消，不能因此
    // 把上一轮登录判成取消（典型场景：401 弹出 API Key 框接管了屏幕）。
    closeWebOverlay(true);
    return new Promise(function (resolve) {
      var overlay = document.createElement('div');
      overlay.id = 'a2a-web-overlay';
      overlay.style.cssText = 'position:fixed;inset:0;z-index:2147483647;display:flex;'
        + 'align-items:center;justify-content:center;background:rgba(0,0,0,.55);';
      var card = document.createElement('div');
      card.style.cssText = 'background:#1e1f22;color:#e8e8e8;border-radius:10px;'
        + 'padding:22px 24px;width:min(420px,86vw);box-shadow:0 12px 40px rgba(0,0,0,.4);'
        + 'font:14px/1.6 system-ui,-apple-system,"Segoe UI",sans-serif;';
      var title = document.createElement('div');
      title.textContent = titleText;
      title.style.cssText = 'font-size:15px;font-weight:600;margin-bottom:10px;';
      var body = document.createElement('div');
      body.innerHTML = bodyHtml;
      card.appendChild(title);
      card.appendChild(body);
      var input = body.querySelector('input');
      var cancelText = options.cancelText || '';
      var done = false;
      var actions = document.createElement('div');
      actions.style.cssText = 'display:flex;gap:10px;margin-top:14px;';
      if (cancelText) {
        var cancelButton = document.createElement('button');
        cancelButton.textContent = cancelText;
        cancelButton.style.cssText = 'flex:1;padding:8px 0;border:1px solid #3a3b3f;'
          + 'border-radius:6px;background:transparent;color:#c9c9c9;font-size:14px;cursor:pointer;';
        cancelButton.addEventListener('click', function () { finish(null); });
        actions.appendChild(cancelButton);
      }
      var button = document.createElement('button');
      button.textContent = confirmText;
      button.style.cssText = 'flex:1;padding:8px 0;border:0;border-radius:6px;'
        + 'background:#4c7dff;color:#fff;font-size:14px;cursor:pointer;';
      actions.appendChild(button);
      card.appendChild(actions);
      overlay.appendChild(card);
      // Esc 与取消按钮同义。捕获阶段拦下并 stopPropagation：下层「添加账号」
      // 弹窗自己也监听 Esc，不拦住会顺带关掉它，再触发一次登录取消。
      var onKeydown = function (event) {
        if (event.key !== 'Escape' || !cancelText) return;
        event.preventDefault();
        event.stopPropagation();
        finish(null);
      };
      // disposeOnly：只摘监听与 DOM，用一个哨兵值让等待方安静退出。
      // 流程靠自动路径先完成时（pollWait 赢了 Promise.race），残留的这一支
      // 必须结束 —— 既不能 resolve(null)（会被当成「用户取消」去撤服务端任务），
      // 也不能永远挂着（那会把整条闭包留在内存里）。
      var finish = function (value, disposeOnly) {
        if (done) return;
        done = true;
        document.removeEventListener('keydown', onKeydown, true);
        overlay.remove();
        if (activeOverlay && activeOverlay.overlay === overlay) activeOverlay = null;
        resolve(disposeOnly ? DISPOSED : value);
      };
      document.body.appendChild(overlay);
      // 只有带取消按钮的覆盖层才挂 Esc 监听：其余的（API Key 输入、链接兜底）
      // 不该被 Esc 消掉，也不该在摘除后留下指向已消失弹层的 document 级监听
      if (cancelText) document.addEventListener('keydown', onKeydown, true);
      button.addEventListener('click', function () { finish(input ? input.value : true); });
      if (input) {
        input.addEventListener('keydown', function (event) {
          if (event.key === 'Enter') finish(input.value);
        });
        setTimeout(function () { input.focus(); }, 60);
      }
      if (cancelText) {
        activeOverlay = {
          overlay: overlay,
          cancelable: true,
          cancel: function () { finish(null); },
          dispose: function () { finish(undefined, true); },
        };
      } else {
        // 不带取消的层也登记：流程收尾时要把「浏览器拦截弹窗」这类提示一起摘掉
        activeOverlay = {
          overlay: overlay,
          cancelable: false,
          cancel: function () { /* 没有取消语义 */ },
          dispose: function () { finish(undefined, true); },
        };
      }
      if (options.onMount) {
        try { options.onMount(body, { cancel: function () { finish(null); } }); }
        catch (e) { /* 挂监听失败不该挡住弹层 */ }
      }
    });
  }

  /** 复制文本：面板常以局域网 http 访问（非安全上下文），clipboard API 不可用，
   *  因此保留 textarea + execCommand 的旧路径兜底。 */
  function copyText(text) {
    if (navigator.clipboard && window.isSecureContext) {
      return navigator.clipboard.writeText(text);
    }
    return new Promise(function (resolve, reject) {
      var area = document.createElement('textarea');
      area.value = text;
      area.setAttribute('readonly', '');
      area.style.cssText = 'position:fixed;top:0;left:0;opacity:0;pointer-events:none;';
      document.body.appendChild(area);
      area.select();
      var ok = false;
      try { ok = document.execCommand('copy'); } catch (e) { ok = false; }
      area.remove();
      if (ok) resolve();
      else reject(new Error('复制失败'));
    });
  }

  function askForKey() {
    if (!askForKey.promise) {
      askForKey.promise = ensureOverlay(
        '需要网关 API Key',
        '<div style="margin-bottom:10px;">此面板受网关鉴权保护，请输入一把已启用的 API Key'
        + '（在「API Keys」页创建）。</div>'
        + '<input type="password" placeholder="sk-…" style="width:100%;box-sizing:border-box;'
        + 'padding:8px 10px;border:1px solid #3a3b3f;border-radius:6px;background:#26272b;'
        + 'color:#e8e8e8;">',
        '保存并继续'
      ).then(function (value) {
        askForKey.promise = null;
        // 被别的弹层接管（见 DISPOSED）：当作没填，避免把这枚哨兵当成 Key 存起来
        if (value === DISPOSED) return '';
        if (value) storeKey(String(value).trim());
        return value;
      });
    }
    return askForKey.promise;
  }

  function showLinkFallback(url) {
    ensureOverlay(
      '浏览器拦截了弹出窗口',
      '<div>请点击下面的链接打开授权页：</div>'
      + '<a href="' + url.replace(/"/g, '&quot;') + '" target="_blank" rel="noopener" '
      + 'style="color:#7fa7ff;word-break:break-all;">' + url + '</a>',
      '已完成，关闭'
    );
  }

  // 关掉当前覆盖层。silent=false 时连等待中的 Promise 一起以「取消」收尾 ——
  // 「取消等待」按钮与关掉添加账号弹窗都走这条；流程正常结束（silent=true）
  // 时只摘 DOM，不 resolve 成「用户取消」，免得把已经成功的结果改写成取消。
  function closeWebOverlay(silent) {
    if (activeOverlay) {
      if (!silent && activeOverlay.cancelable) activeOverlay.cancel();
      else activeOverlay.dispose();
      return;
    }
    var overlay = document.getElementById('a2a-web-overlay');
    if (overlay) overlay.remove();
  }

  // 这些提供商的授权页会把浏览器导航到 loopback 地址。
  // Docker 远程面板里该地址属于浏览器所在电脑，不能自动回到容器，
  // 因此让用户把地址栏的最终 URL 粘回受保护接口。小浣熊单独改用
  // 官方授权页支持的 redirect 分支，直接把 authorization_code 导回网关。
  // 自动路径的窗口期：先给「浏览器自己跳回网关」那条链一段时间，再决定要不要
  // 把粘贴框推给用户。见 runLoginFlow 里那段说明。
  var MANUAL_CALLBACK_GRACE_MS = 10000;

  function needsManualCallback(provider) {
    return provider === 'trae'
      || provider === 'accio' || provider === 'accio-cn'
      || provider === 'codearts' || provider === 'autoclaw-intl'
      // LobsterAI 同属这一型：授权页完成后 302 回
      // `http://127.0.0.1:<网关端口>/auth/callback`，在 Docker 面板里那个
      // 地址属于浏览器所在电脑、回不到容器，只能让用户把最终 URL 粘回来。
      || provider === 'lobsterai'
      // Antigravity 也是这一型：它的 `redirect_uri` 由网关拼成
      // `http://localhost:<网关端口>/oauth-callback`（Google 的 desktop 型
      // client 只收 loopback，见 `providers/antigravity/oauth.rs`）。桌面端
      // 浏览器与网关同机，一跳就回来；服务器 / Docker 部署时那个 localhost
      // 属于浏览器所在电脑，回调永远到不了网关 —— 用户只能把地址栏整条 URL
      // 粘回弹窗，由后端 `submit_login_callback` 的 Antigravity 分支换码。
      // 粘贴不要求该地址可达：换码回传的 `redirect_uri` 与发起授权时同源
      // （都取自 `login_redirect_uri`），天然逐字一致。
      || provider === 'antigravity';
  }

  function escapeHtml(value) {
    return String(value == null ? '' : value)
      .replace(/&/g, '&amp;').replace(/</g, '&lt;')
      .replace(/>/g, '&gt;').replace(/"/g, '&quot;');
  }

  // 官方小浣熊授权页在 login_source=desktop 时固定跳 office-raccoon://，
  // 浏览器地址栏不会暴露授权码。该页面还支持 redirect 分支：去掉 desktop
  // 标记后，它会把 authorization_code 追加到 redirect URL 并导航过去。
  // 这里把回调地址放在当前面板 Origin，浏览器因此能直接访问 Docker 网关。
  //
  // ── 为什么发起前要先检查面板自身地址（raccoonCallbackOriginProblem）──
  // 上游授权页对 redirect 参数有一份写死的白名单（实测 2026-10）：http 协议
  // + 回环主机（127.0.0.1 / localhost / [::1]）+ 显式端口；域名、局域网 IP、
  // https、省略端口一律拒收 —— 服务端直接把授权上下文置空，页面渲染
  // 「授权链接无效，请从客户端重新发起登录。」这句话对我们的用户毫无信息量
  // （链接是面板自己拼的，用户没改过任何参数），所以在发起登录前先按同一
  // 口径检查面板地址，不满足就弹指引拦下，不送用户去授权页吃那句报错。
  function raccoonCallbackOriginProblem() {
    var hostname = window.location.hostname;
    var port = String(window.location.port || '');
    var loopback = hostname === '127.0.0.1' || hostname === 'localhost' || hostname === '[::1]';
    if (window.location.protocol === 'http:' && loopback && port !== '') return null;
    var reason;
    if (window.location.protocol !== 'http:') {
      reason = '小浣熊授权页只接受 http:// 的回调地址，当前面板以 '
        + window.location.protocol + ' 访问';
    } else if (!loopback) {
      reason = '小浣熊授权页只接受 127.0.0.1 / localhost 的回调地址，当前面板主机是 ' + hostname;
    } else {
      reason = '小浣熊授权页要求回调地址带显式端口，当前面板地址未带（默认端口访问）';
    }
    var displayPort = port || (window.location.protocol === 'https:' ? '443' : '80');
    var html = '<div style="margin-bottom:10px;">小浣熊官方授权页只接受<strong>浏览器本机的回环地址</strong>'
      + '（127.0.0.1 / localhost，http 且带端口）作为登录回调；当前面板地址是 <code>'
      + escapeHtml(window.location.origin) + '</code>，继续发起只会看到「授权链接无效」。</div>'
      + '<div style="margin-bottom:6px;">可行的做法：</div>'
      + '<div style="margin-bottom:6px;">① 在打开面板的这台电脑上，改用 <code>http://localhost:'
      + escapeHtml(displayPort) + '</code> 访问面板后再登录；面板部署在别的机器时，先做端口转发'
      + '（例如 <code>ssh -L ' + escapeHtml(displayPort) + ':127.0.0.1:' + escapeHtml(displayPort)
      + ' 用户@面板主机</code>，端口按实际改）。</div>'
      + '<div>② 改用「粘贴 token / refreshToken」方式添加小浣熊账号，不走授权页。</div>';
    return { title: '小浣熊网页登录无法在当前地址发起', html: html, reason: reason };
  }

  // 预检不过的完整处置：先弹指引层（模态，细节在这里讲清），用户关闭后再
  // reject —— 引擎会 toast 一次 reason 收尾（弹层承载细节，toast 只做结论）。
  async function reportRaccoonCallbackProblem(problem) {
    await ensureOverlay(problem.title, problem.html, '知道了');
    throw new Error(problem.reason);
  }

  function buildRaccoonWebAuthUrl(provider, state, authUrl) {
    if (provider !== 'raccoon') return authUrl;
    // 防御纵深：startLogin 已预检过（弹指引并拦下）。这里再查一次，是因为
    // 本函数是 redirect 分支的唯一入口 —— 未来新增调用方不必记得「必须回环
    // 访问」这条前提；走到这里才失败时，runLoginFlow 的 finally 会关掉窗口，
    // 已建的后台任务无人认领会自行超时落定。
    var problem = raccoonCallbackOriginProblem();
    if (problem) throw new Error(problem.reason);
    var authorizeUrl;
    try {
      authorizeUrl = new URL(authUrl, window.location.href);
    } catch (error) {
      throw new Error('小浣熊授权地址无效，无法建立远程回调');
    }
    var callbackUrl = new URL('/api/session/login/raccoon-callback', window.location.origin);
    callbackUrl.searchParams.set('state', state);
    authorizeUrl.searchParams.set('login_source', 'web');
    authorizeUrl.searchParams.set('redirect', callbackUrl.toString());
    return authorizeUrl.toString();
  }

  async function waitForManualCallback(provider, state, authUrl) {
    // 称号链必须与上面 `needsManualCallback` 的白名单**一一对应**：少一支，
    // 那一家就会顶着别家的名字弹框（`lobsterai` 曾经落到 `'AutoClaw'`，用户看到
    // 的是「AutoClaw 需要粘贴回调地址」而手里登的却是有道龙虾）。
    // 末尾兜底用 provider id 本身，不用任何一家的名字 —— 名字错的代价是误导，
    // 显示 id 最坏也只是不好看。
    var label = provider === 'raccoon' ? '小浣熊'
      : provider === 'trae' ? 'Trae'
      : provider.indexOf('accio') === 0 ? 'Accio'
      : provider === 'codearts' ? 'CodeArts'
      : provider === 'autoclaw-intl' ? 'AutoClaw 国际版'
      : provider === 'lobsterai' ? 'LobsterAI（有道龙虾）'
      : provider === 'antigravity' ? 'Antigravity'
      : provider;
    var callbackInstruction = '<strong>如果授权完成后账号没有自动出现</strong>，'
        + '再复制授权页浏览器地址栏中的<strong>完整地址</strong>粘到下面提交；'
        + '不要复制授权页原始地址，也不要改动参数。';
    var linkStyle = 'flex:1;padding:7px 0;border:1px solid #3a3b3f;border-radius:6px;'
        + 'background:transparent;color:#c9c9c9;font-size:13px;cursor:pointer;';
    while (true) {
      // 链接正文每轮重建：CodeArts 第一跳会把 authUrl 换成 portal 的下一跳，
      // 复用上一轮的 HTML 会让用户看到一个已经作废的地址
      var authLinkHtml = '<div style="margin-bottom:6px;color:#9a9a9a;">授权页地址</div>'
          + '<code style="display:block;margin-bottom:8px;padding:8px 10px;border-radius:6px;'
          + 'background:#26272b;color:#b8c7e6;font-size:12px;line-height:1.5;'
          + 'word-break:break-all;max-height:96px;overflow:auto;">' + escapeHtml(authUrl) + '</code>'
          + '<div style="display:flex;gap:8px;margin-bottom:14px;">'
          + '<button type="button" data-a2a-act="open" style="' + linkStyle + '">打开授权页</button>'
          + '<button type="button" data-a2a-act="copy" style="' + linkStyle + '">复制链接</button>'
          + '</div>';
      var callbackUrl = await ensureOverlay(
        label + '需要粘贴回调地址',
        '<div style="margin-bottom:10px;">' + callbackInstruction + '</div>'
        + authLinkHtml
        + '<input type="text" spellcheck="false" autocomplete="off" placeholder="http://127.0.0.1:…/callback?..." '
        + 'style="width:100%;box-sizing:border-box;padding:8px 10px;border:1px solid #3a3b3f;'
        + 'border-radius:6px;background:#26272b;color:#e8e8e8;">',
        '提交回调地址',
        {
          cancelText: '取消登录',
          onMount: function (body) {
            var openBtn = body.querySelector('[data-a2a-act="open"]');
            var copyBtn = body.querySelector('[data-a2a-act="copy"]');
            if (openBtn) {
              openBtn.addEventListener('click', function () {
                // noopener：授权页不该拿到面板的 window 引用
                window.open(authUrl, '_blank', 'noopener');
              });
            }
            if (copyBtn) {
              copyBtn.addEventListener('click', function () {
                copyText(authUrl).then(function () {
                  copyBtn.textContent = '已复制';
                  setTimeout(function () {
                    if (copyBtn.isConnected) copyBtn.textContent = '复制链接';
                  }, 1500);
                }, function () {
                  // 非安全上下文且 execCommand 也被禁：退回选中文本让用户手动复制
                  copyBtn.textContent = '请手动复制上方链接';
                });
              });
            }
          },
        }
      );
      // DISPOSED = 覆盖层被别处接管（例如 401 弹了 API Key 框）：这一轮
      // 粘贴已经不可能继续，按取消收尾。界面不需要再提示一次 —— 接管屏幕的
      // 那个弹层（要 Key）本身就是对用户唯一的、更准确的说明。
      if (callbackUrl === DISPOSED) {
        var takenOver = canceledError();
        takenOver.silent = true;
        throw takenOver;
      }
      // null = 用户点了「取消登录」或按了 Esc
      if (callbackUrl == null) return cancelWebLogin(state);
      callbackUrl = String(callbackUrl || '').trim();
      if (!callbackUrl) continue;
      try {
        var submitted = await call('POST', '/api/session/login/callback', {
          state: state,
          callbackUrl: callbackUrl,
        });
        if (submitted && submitted.nextUrl) {
          // CodeArts 第一跳只有 secret，需要先打开 portal 的下一跳；下一轮
          // 再粘贴浏览器最终回调地址即可完成换码。
          authUrl = String(submitted.nextUrl);
          continue;
        }
        // Trae 是把 query 注入现有 listener 后异步换证；其它几家也统一
        // 以 /wait 的最终状态为准，避免把“已收到回调”误报成“已登录”。
        return await pollWait(state, provider);
      } catch (error) {
        // 取消不是失败：提交后的 pollWait 也会因「取消等待」抛 canceled，
        // 若不先分流就会弹出一个莫名其妙的「回调提交失败」让用户重填。
        if (error && error.canceled) throw error;
        // 提交失败也留着「取消登录」：否则用户只剩「重新填写」一条路，
        // 想放弃时又得靠刷新页面
        var retry = await ensureOverlay(
          '回调提交失败',
          '<div style="margin-bottom:4px;">' + escapeHtml(error && error.message ? error.message : error)
          + '</div><div>请确认复制的是授权完成后的完整地址，再重新提交。</div>',
          '重新填写',
          { cancelText: '取消登录' }
        );
        // 覆盖层被别处接管：别再重开粘贴框（否则会在流程结束后留下一层幽灵遮罩）
        if (retry === DISPOSED) {
          var retryTakenOver = canceledError();
          retryTakenOver.silent = true;
          throw retryTakenOver;
        }
        if (retry == null) return cancelWebLogin(state);
      }
    }
  }

  /** 用户主动取消粘贴回调：通知服务端撤掉这一轮登录任务，并向上抛一个
   *  可被界面识别为「取消」的结果。
   *
   *  为什么要真的打 /cancel：Trae 那条链在网关侧持有回调监听端口，不撤要占满
   *  5 分钟超时才释放，用户随即重新发起会抢不到端口（与 `LoginService::cancel`
   *  注释里 AutoClaw / CodeArts 同一个坑）。 */
  async function cancelWebLogin(state) {
    var canceled = true;
    try {
      var result = await call('POST', '/api/session/login/cancel', { state: state });
      canceled = !!(result && result.canceled);
    } catch (error) { /* 任务已自行落定时会 404：当作已取消 */ }
    var err = new Error(canceled ? '已取消登录等待' : '登录已结束');
    err.canceled = true;
    throw err;
  }

  // ── 浏览器内核版本（设置页「版本信息」用）────────────────────
  // 桌面端由壳给 WebView2 / WKWebView 的版本；网页端只能从 UA 里认：
  // Chrome/Edge/Opera 都是 `Chrome/<ver>`、Firefox 是 `Firefox/<ver>`、
  // Safari 是 `Version/<ver> … Safari/`。认不出就给空串 —— 界面显示「—」，
  // 不编造一个看起来像版本号的字符串。
  function browserEngineVersion() {
    var ua = (typeof navigator !== 'undefined' && navigator.userAgent) || '';
    var patterns = [
      [/Edg(?:e|A|iOS)?\/(\d+(?:\.\d+)*)/, 'Edge '],
      [/OPR\/(\d+(?:\.\d+)*)/, 'Opera '],
      [/Chrome\/(\d+(?:\.\d+)*)/, 'Chrome '],
      [/Firefox\/(\d+(?:\.\d+)*)/, 'Firefox '],
      [/Version\/(\d+(?:\.\d+)*).*Safari/, 'Safari '],
    ];
    for (var i = 0; i < patterns.length; i += 1) {
      var match = patterns[i][0].exec(ua);
      if (match) return patterns[i][1] + match[1];
    }
    return '';
  }

  // ── 错误归一（与桌面 bridge 的 asError 同语义）─────────────
  function asError(failure) {
    if (failure instanceof Error) return failure;
    if (typeof failure === 'string') return new Error(failure);
    if (failure && typeof failure === 'object') {
      var message = failure.message != null ? failure.message
        : failure.error != null ? failure.error
        : failure.msg;
      if (typeof message === 'string' && message) return new Error(message);
      try { return new Error(JSON.stringify(failure)); } catch (e) { /* 落到下面 */ }
    }
    return new Error(String(failure == null ? '操作失败' : failure));
  }

  // ── HTTP 调用：信封解包与桌面 gateway::unwrap_envelope 对齐 ──
  async function httpCall(method, path, body, retried) {
    var headers = { 'Accept': 'application/json' };
    if (apiKey) headers['x-api-key'] = apiKey;
    // 面板会话的头部回退（见 PANEL_AUTH 段）：有存令牌就带上；
    // refresh/logout 只在 /api/panel/ 路径发 refresh，对齐
    // refresh cookie 的 Path=/api/panel 语义。
    var panelAccess = storedPanelToken('access');
    if (panelAccess) {
      headers['x-panel-token'] = panelAccess;
      // Authorization 是标准头：连自定义请求头都剥的中转也会放它过去
      headers['Authorization'] = 'Bearer ' + panelAccess;
    }
    if (path.indexOf('/api/panel/') === 0) {
      var panelRefresh = storedPanelToken('refresh');
      if (panelRefresh) headers['x-panel-refresh'] = panelRefresh;
    }
    var init = { method: method, headers: headers };
    var wantsBody = method === 'POST' || method === 'PUT' || method === 'PATCH'
      || !(body === null || body === undefined);
    if (wantsBody) {
      headers['Content-Type'] = 'application/json';
      init.body = JSON.stringify(body === null || body === undefined ? {} : body);
    }
    var response;
    try {
      response = await fetch(path, init);
    } catch (error) {
      throw new Error('无法连接网关：' + (error && error.message ? error.message : error));
    }
    var text = await response.text();
    if (response.status === 401 && !retried) {
      // 按错误类型分流。panel_login_required：管理员已注册、要登录 ——
      // 先用长效 refresh 静默换新（access 过期时用户无感），不行再整页
      // 跳独立登录页；其余 401 = 未配管理员的部署，弹 Key 框兜底。
      var probe = null;
      try { probe = JSON.parse(text); } catch (e) { /* 非 JSON */ }
      var errorType = probe && probe.error && probe.error.type;
      if (errorType === 'panel_login_required') {
        var refreshed = await tryRefresh();
        if (refreshed) return httpCall(method, path, body, true);
        // 续期失败 = 会话链真的没了：清掉本地令牌再跳登录页，
        // 否则下次进来还带着死 token，永远是同一轮失败。
        clearPanelAuth();
        window.location.href = '/login';
        // 页面即将整页跳转，返回一个挂起的承诺占位
        return new Promise(function () {});
      }
      var key = await askForKey();
      if (key) {
        storeKey(key);
        return httpCall(method, path, body, true);
      }
    }
    var payload;
    try { payload = JSON.parse(text); } catch (e) {
      throw new Error(text.trim() || '本地代理返回了空响应（HTTP ' + response.status + '）');
    }
    var ok = response.status >= 200 && response.status < 300;
    var success = typeof payload.success === 'boolean' ? payload.success : undefined;
    if (!ok || success === false) {
      var detail = payload.error != null ? payload.error
        : payload.message != null ? payload.message
        : payload.msg != null ? payload.msg
        : 'HTTP ' + response.status;
      throw asError(typeof detail === 'string' ? detail : (detail && detail.message) || JSON.stringify(detail));
    }
    return payload && Object.prototype.hasOwnProperty.call(payload, 'data') ? payload.data : payload;
  }

  function call(method, path, body) {
    return httpCall(method, path, body === undefined ? null : body);
  }

  // access 短效令牌过期后的静默续期。中转环境里 refresh cookie 到不了
  // 服务端，改为带 `x-panel-refresh` 头，并显式带 `x-panel-auth-mode: body`
  // 要求新令牌进响应体；响应体里的新令牌回写 PANEL_AUTH（轮换后旧 refresh 已
  // 作废，不回存下次必失败）。cookie 模式（本地没存过令牌，即不带
  // x-panel-refresh）下轮换随 Set-Cookie 完成，body 里没有令牌 ——
  // resp.ok 即续期成功，不能按失败处理。
  //
  // ⚠️ 标记**只在本地存过令牌时才发**（头与 query 两条通道一起，同一个条件）：
  // 早先这里把 `?auth-mode=body` 写成了无条件，于是直连环境每两小时一次的静默
  // 续期都会把裸令牌吐进响应体、并被 `storePanelAuth` 落进 localStorage ——
  // 「直连环境令牌不 JS 可读」这条性质在一次续期之后就没了。服务端把标记当
  // **显式覆盖**（见 `api::panel::tokens_in_body`），所以不发标记时它会自己按
  // 探针 cookie 判：直连 → 仍走 cookie（响应体无令牌）；中转 → 探针判不通，
  // 令牌照样进响应体。两侧行为都不比原来差，只有直连侧少暴露一份凭据。
  // ── 并发 401 的单飞 ───────────────────────────────────────
  // access 过期那一刻，面板首屏一次会并发十几条 /api/*，它们同时收到 401、
  // 同时带着同一把 refresh 来换 —— 各发各的话，先到的轮换成功，其余就撞上
  // 「已轮换」判定。服务端对一分钟内的情况有宽限（access::ROTATION_GRACE_MS），
  // 但那只兜跨页面上下文的场（多标签页）；同一页面内的并发在这里合并掉：
  // 同一时刻只放行一次轮换请求，其余的等同一个结果再各自动重试。
  var refreshInFlight = null;
  async function tryRefresh() {
    if (refreshInFlight) return refreshInFlight;
    refreshInFlight = doRefresh().finally(function () { refreshInFlight = null; });
    return refreshInFlight;
  }

  async function doRefresh() {
    try {
      var headers = { 'Accept': 'application/json' };
      var refreshToken = storedPanelToken('refresh');
      var url = '/api/panel/refresh';
      if (refreshToken) {
        headers['x-panel-refresh'] = refreshToken;
        headers['x-panel-auth-mode'] = 'body';
        // 标记走两条通道（头 + query）：中转剥自定义请求头时 query 照常生效
        url += '?auth-mode=body';
      }
      var resp = await fetch(url, { method: 'POST', headers: headers });
      if (!resp.ok) return false;
      var payload = await resp.json();
      var data = payload && payload.data;
      if (data && typeof data.accessToken === 'string' && data.accessToken
        && typeof data.refreshToken === 'string' && data.refreshToken) {
        storePanelAuth(data.accessToken, data.refreshToken);
      }
      return true;
    } catch (e) {
      return false;
    }
  }

  // ── 事件模拟 ──────────────────────────────────────────────
  var stateListeners = new Set();
  var stateEmitTimer = null;
  function emitStateSoon() {
    if (stateEmitTimer) return;
    stateEmitTimer = setTimeout(async function () {
      stateEmitTimer = null;
      try {
        var next = await call('GET', '/api/session');
        stateListeners.forEach(function (cb) { try { cb(next); } catch (e) { console.warn(e); } });
      } catch (e) { /* 面板关闭中的正常失败 */ }
    }, 300);
  }

  // ── 登录状态机（对齐桌面壳 login:state 的 {active, provider}）──
  var loginActive = false;
  var loginProvider = '';
  var loginListeners = new Set();
  function emitLogin() {
    var payload = { active: loginActive, provider: loginProvider };
    loginListeners.forEach(function (cb) { try { cb(payload); } catch (e) { console.warn(e); } });
  }

  var sleep = function (ms) { return new Promise(function (r) { setTimeout(r, ms); }); };

  // 超时文案必须给出下一步。对走 loopback 回调的几家来说，最常见的原因不是
  // 「忘了授权」，而是浏览器与网关不在同一台机器上（服务器 / Docker 部署）：
  // 回环地址属于用户本机，回调永远到不了网关，只能把地址栏整条 URL 粘回来。
  // 只说「等待超时」，用户不知道该干什么。
  var MANUAL_CALLBACK_TIMEOUT_HINT = '：如果浏览器与网关不在同一台机器上，'
    + '授权完成后的回调会落到你本机而不是网关 —— 请复制浏览器地址栏里的'
    + '完整回调地址，粘回面板重试。';

  async function pollWait(state, provider) {
    var deadline = Date.now() + 5 * 60 * 1000;
    while (Date.now() < deadline) {
      if (isFlowCanceled(state)) throw canceledError();
      await sleep(3000);
      if (isFlowCanceled(state)) throw canceledError();
      var wait = await call('GET', '/api/session/login/wait?state=' + encodeURIComponent(state));
      if (wait && wait.done) {
        if (wait.error) throw new Error(wait.error);
        return wait.session == null ? {} : wait.session;
      }
    }
    var hint = provider && needsManualCallback(provider) ? MANUAL_CALLBACK_TIMEOUT_HINT : '';
    throw new Error('登录等待超时（5 分钟）' + hint);
  }

  /** 取消信号：用 `canceled` 标记与普通失败区分（界面据此提示「已取消」而非报错） */
  function canceledError(message) {
    var err = new Error(message || '已取消登录等待');
    err.canceled = true;
    return err;
  }

  /** 当前这一轮登录的 state 与取消标记。只记一轮（壳/桥本身也只允许一轮），
   *  换了新流程就整体替换 —— 上一轮的迟到轮询凭 state 比对自然失效。 */
  var currentFlow = null;
  function isFlowCanceled(state) {
    return !!(currentFlow && currentFlow.state === state && currentFlow.canceled);
  }

  /** 发起一次登录流程：先开窗口（用户手势还在时占位成功率高），再拿地址导航。 */
  async function runLoginFlow(provider, startRequest) {
    loginActive = true;
    loginProvider = provider;
    emitLogin();
    var popup = null;
    // state 要等 /start 回来才知道，先把对象挂上 —— 取消可能发生在等待
    // /start 的那几秒里（此时 currentFlow 已存在，标记能打上）
    var flow = { state: '', canceled: false };
    currentFlow = flow;
    try { popup = window.open('about:blank', 'a2a-login'); } catch (e) { /* 拦截时走兜底 */ }
    try {
      var started = await startRequest;
      var authUrl = started && started.authUrl;
      if (!authUrl) throw new Error('网关未返回授权地址');
      flow.state = String(started.state || '');
      // 等 /start 的那段时间里用户已经点了取消：state 现在才拿到，补发一次
      // 撤销请求，别让服务端那轮任务空转到 5 分钟超时（Trae 会占着回调端口）
      if (flow.canceled) {
        await cancelWebLogin(flow.state);
      }
      authUrl = buildRaccoonWebAuthUrl(provider, started.state, authUrl);
      if (popup && !popup.closed) {
        popup.location.href = authUrl;
      } else {
        var second = null;
        try { second = window.open(authUrl, '_blank'); } catch (e) { /* 落到链接兜底 */ }
        if (!second) showLinkFallback(authUrl);
      }
      if (needsManualCallback(provider)) {
        // 先给自动路径一段窗口期，再上兜底 —— 不是一上来就把粘贴框推到用户脸上。
        //
        // 这条链在「面板经回环地址访问」（桌面端、或远程机器上做了端口映射）时
        // 是**自己走得完**的：网关给的 `redirect_uri` 就是
        // `http://127.0.0.1:<网关端口>/…`，浏览器一跳就回来了。原来的写法把
        // `waitForManualCallback` 直接放进 `Promise.race`，弹框几乎与授权页
        // 同时出现，用户自然会去粘贴 —— 明明不需要（自动那条往往几秒就完成）。
        //
        // 真回不来的远端场景（浏览器与网关不同机且没有端口映射）照样会弹，
        // 只是晚 10 秒 —— 那时的代价只是多等一会儿，比「让人以为必须手工粘」轻。
        // 两边都保留：先完成的一方结束流程，finally 会关掉残留覆盖层。
        //
        // 两个分支各自把结果包成 {session} / {error} 再进竞速：落败的那一支
        // 随后才会被 finally 唤醒（dispose）或被下一拍轮询判成取消，如果让它
        // 直接 reject，Promise.race 已经 settle，那个 rejection 没人接 ——
        // 控制台会冒出「Uncaught (in promise)」，用户则以为页面出了错。
        var outcome = await Promise.race([
          pollWait(started.state, provider).then(
            function (session) { return { session: session }; },
            function (error) { return { error: error }; }
          ),
          (async function () {
            await sleep(MANUAL_CALLBACK_GRACE_MS);
            // 窗口期里这一轮已经落定（自动路径成功 / 用户取消 / 用户关了
            // 添加账号弹窗）：不能再弹粘贴框，否则流程结束后会留下一层
            // 摘不掉的遮罩（currentFlow 已被 finally 清空）。
            if (currentFlow !== flow || flow.canceled) throw canceledError();
            return waitForManualCallback(provider, started.state, authUrl);
          })().then(
            function (session) { return { session: session }; },
            function (error) { return { error: error }; }
          ),
        ]);
        if (outcome.error) {
          if (outcome.error.canceled) {
            // 两种静默来源：外部发起的取消（那边已 toast）、或被其它弹层接管
            var silent = flow.externalCancel || outcome.error.silent;
            return silent ? { canceled: true, silent: true } : { canceled: true };
          }
          throw outcome.error;
        }
        return outcome.session;
      }
      return await pollWait(started.state, provider);
    } catch (error) {
      // 用户主动取消 → 返回 {canceled:true}，界面按「已取消」提示而不是报错。
      // externalCancel：取消由外部（「取消等待」按钮 / 关掉添加账号弹窗）发起，
      // 那边已经 toast 过一次；error.silent：被别的弹层接管，由那个弹层说明。
      // 两种情况都带上 silent，让引擎别再重复提示。
      if (error && error.canceled) {
        var quiet = flow.externalCancel || error.silent;
        return quiet ? { canceled: true, silent: true } : { canceled: true };
      }
      throw error;
    } finally {
      if (popup && !popup.closed) { try { popup.close(); } catch (e) { /* 无害 */ } }
      closeWebOverlay(true);
      if (currentFlow === flow) currentFlow = null;
      loginActive = false;
      loginProvider = '';
      emitLogin();
    }
  }

  // ── 壳特有命令的网页端降级表 ──────────────────────────────
  var SHELL_UNAVAILABLE = '该操作在网页端不可用（仅桌面端支持）';

  var shellCommands = {
    api_request: function (args) {
      var request = (args && args.request) || {};
      return httpCall(request.method || 'GET', request.path || '/', request.body);
    },
    check_update: function () {
      // 网页端没有「壳打包的版本号」这个概念：不带 current，服务端按自己的
      // 编译期版本比较（见 api/update 的 check 分支）。返回里带 selfUpdate /
      // serverAsset / backupAvailable —— 面板据此决定显示「一键更新」、
      // 「回滚」还是退回手动升级命令，不再由这里硬编码。
      return call('GET', '/api/update/check');
    },
    get_update_status: function () { return call('GET', '/api/update/status'); },
    // 下载三件套：下载落在服务端的数据目录（/data/updates），进度由面板轮询。
    // 桌面端这三个走壳命令（壳要往后端转发），网页端直接打接口 —— 桥的两侧
    // 名字一致，面板代码不感知差异。
    download_update: function (args) {
      var payload = args || {};
      return call('POST', '/api/update/download', {
        url: String(payload.url || ''),
        name: payload.name != null ? String(payload.name) : '',
      });
    },
    update_progress: function () { return call('GET', '/api/update/progress'); },
    cancel_update: function () { return call('POST', '/api/update/cancel', {}); },
    backend_status: function () {
      return Promise.resolve({ ready: true, port: null, portFromEnv: false, failure: null });
    },
    port_occupant: function () { return Promise.resolve(null); },
    get_app_settings: function () {
      // 桌面设置（关窗到托盘 / 开机自启 / 轻量模式）在网页端没有宿主，固定默认值
      return Promise.resolve({ closeToTray: false, autostart: false, proxyPort: 0, lightweightMode: false });
    },
    open_release_page: function (args) {
      if (args && args.url) { try { window.open(args.url, '_blank'); } catch (e) { /* 无害 */ } }
      return Promise.resolve({ url: (args && args.url) || '' });
    },
    export_accounts: function () {
      return downloadFile('GET', '/api/accounts/export', 'agent2api-accounts.json');
    },
    export_logs: function () {
      return downloadFile('GET', '/api/logs/download', 'agent2api-logs.txt');
    },
  };

  /** 网页端的「文件对话框」：拉成 Blob 触发浏览器下载 */
  async function downloadFile(method, path, filename) {
    var headers = authHeaders({ 'Accept': '*/*' });
    var response = await fetch(path, { method: method, headers: headers });
    if (!response.ok) throw new Error('导出失败（HTTP ' + response.status + '）');
    var blob = await response.blob();
    // 管理 API 的响应带 { success, data } 信封：落盘前剥掉，存 data 本体 ——
    // 与桌面端的导出文件同一格式；信封壳留着，这份文件导回去时顶层没有
    // accounts，只会得到「缺少 accounts 字段」。顺带把账号数带回给调用方
    //（设置页用它提示「已导出 N 个账号」，拿不到会误报「没有可导出的账号」）
    var summary = {};
    try {
      var body = JSON.parse(await blob.text());
      if (body && body.success === true && body.data && typeof body.data === 'object') {
        var accounts = Array.isArray(body.data.accounts) ? body.data.accounts : null;
        var providers = Array.isArray(body.data.customProviders) ? body.data.customProviders : null;
        if (accounts) summary.count = accounts.length;
        if (providers) summary.customProviders = providers.length;
        blob = new Blob([JSON.stringify(body.data, null, 2)], { type: 'application/json' });
      }
    } catch (e) { /* 非 JSON 响应（日志下载）按原文保存 */ }
    saveBlob(blob, filename);
    summary.saved = true;
    return summary;
  }

  /** 浏览器落盘：Blob → 临时 object URL → 隐藏的 <a download> */
  function saveBlob(blob, filename) {
    var url = URL.createObjectURL(blob);
    var link = document.createElement('a');
    link.href = url;
    link.download = filename;
    document.body.appendChild(link);
    link.click();
    link.remove();
    setTimeout(function () { URL.revokeObjectURL(url); }, 5000);
  }

  /** 面板 / API Key 的请求头（下载、上传这类非 JSON 请求共用） */
  function authHeaders(extra) {
    var headers = extra || {};
    if (apiKey) headers['x-api-key'] = apiKey;
    var panelAccess = storedPanelToken('access');
    if (panelAccess) {
      headers['x-panel-token'] = panelAccess;
      headers['Authorization'] = 'Bearer ' + panelAccess;
    }
    return headers;
  }

  /**
   * 非 JSON 请求（整库下载 / 上传）的鉴权重试，401 分流与 `httpCall` 同一口径：
   * panel_login_required → 静默续期，其余 → 弹 Key 框。
   *
   * `buildInit` 是工厂而不是现成对象：续期后令牌已经换了，重试必须重新取头。
   * 401 的响应体很小（错误 JSON），`clone()` 读它不影响后续正常路径。
   */
  async function fetchAuthed(path, buildInit, retried) {
    var response = await fetch(path, buildInit());
    if (response.status !== 401 || retried) return response;
    var probe = null;
    try { probe = JSON.parse(await response.clone().text()); } catch (e) { /* 非 JSON */ }
    var errorType = probe && probe.error && probe.error.type;
    if (errorType === 'panel_login_required') {
      var refreshed = await tryRefresh();
      if (refreshed) return fetchAuthed(path, buildInit, true);
      clearPanelAuth();
      window.location.href = '/login';
      return new Promise(function () {});
    }
    var key = await askForKey();
    if (!key) return response;
    storeKey(key);
    return fetchAuthed(path, buildInit, true);
  }

  /**
   * 整库备份（网页端）：`GET /api/backup/download` → 浏览器下载 .sql。
   *
   * 返回形状与桌面壳 `commands.rs::export_database` 对齐
   * （`{ bytes, tables, file }`），设置页的成功提示两端共用一套渲染。
   * 表计数在后端放进 `X-Backup-Tables` 响应头 —— 响应体是几百 MB 的 SQL，
   * 不能为了带元数据把它包成 JSON。
   */
  async function exportDatabase(includeLogs) {
    var path = '/api/backup/download?includeLogs=' + (includeLogs === false ? '0' : '1');
    var response = await fetchAuthed(path, function () {
      return { method: 'GET', headers: authHeaders({ 'Accept': '*/*' }) };
    });
    if (!response.ok) {
      // 失败时响应体是管理信封（错误文案在里面），交给统一的错误归一
      var failure = null;
      try { failure = JSON.parse(await response.text()); } catch (e) { /* 非 JSON */ }
      var detail = failure && failure.error != null ? failure.error : ('HTTP ' + response.status);
      throw asError(typeof detail === 'string' ? detail : (detail && detail.message) || '导出失败');
    }
    var tables = [];
    try { tables = JSON.parse(response.headers.get('x-backup-tables') || '[]'); } catch (e) { tables = []; }
    var filename = fileNameFromDisposition(response.headers.get('content-disposition'))
      || ('agent2api-backup-' + downloadStamp() + '.sql');
    var blob = await response.blob();
    saveBlob(blob, filename);
    return { bytes: blob.size, tables: tables, file: filename, saved: true };
  }

  /**
   * 整库恢复（网页端）：选一个 .sql 文件 → `POST /api/backup/upload`。
   *
   * 请求体直接放 File 对象，浏览器按文件流式发送 —— 不能 `file.text()`：
   * 几百 MB 的备份在手机上会先把整份内容读成字符串，内存翻倍且容易 OOM。
   * 返回形状与桌面壳 `commands.rs::import_database` 一致
   * （`{ tables, safetyBackup, warning? }`）。
   */
  async function importDatabase() {
    var picked = await pickFile('.sql');
    if (picked.canceled) return { canceled: true };
    var file = picked.file;
    var response = await fetchAuthed('/api/backup/upload', function () {
      return {
        method: 'POST',
        headers: authHeaders({ 'Content-Type': 'application/sql', 'Accept': 'application/json' }),
        body: file,
      };
    });
    var text = await response.text();
    var payload = null;
    try { payload = JSON.parse(text); } catch (e) { /* 非 JSON（如代理层 413） */ }
    var ok = response.status >= 200 && response.status < 300;
    var success = payload && typeof payload.success === 'boolean' ? payload.success : undefined;
    if (!ok || success === false) {
      if (response.status === 413) {
        throw new Error('备份文件超过 2GB 上限，请改用精简导出（不含日志）后重试');
      }
      var detail = payload && payload.error != null ? payload.error : ('HTTP ' + response.status);
      throw asError(typeof detail === 'string' ? detail : (detail && detail.message) || '恢复失败');
    }
    return payload && Object.prototype.hasOwnProperty.call(payload, 'data') ? payload.data : payload;
  }

  /** `Content-Disposition` 里的 filename（取不到就返回空串，由调用方兜底） */
  function fileNameFromDisposition(value) {
    if (!value) return '';
    var match = /filename\*?=(?:UTF-8'')?"?([^";]+)"?/i.exec(value);
    if (!match) return '';
    try { return decodeURIComponent(match[1]); } catch (e) { return match[1]; }
  }

  /** 下载文件名的时间戳（与后端 `backup_api::file_stamp` 同形，本地时间） */
  function downloadStamp() {
    var now = new Date();
    function pad(value) { return (value < 10 ? '0' : '') + value; }
    return String(now.getFullYear()) + pad(now.getMonth() + 1) + pad(now.getDate())
      + '-' + pad(now.getHours()) + pad(now.getMinutes()) + pad(now.getSeconds());
  }

  /** 壳命令的总入口（同时接住 UI 里两处直接的 internals.invoke('api_request')） */
  function invokeShell(command, args) {
    var handler = shellCommands[command];
    if (handler) return handler(args);
    // 文件对话框类：导入要走文件选择，单独给一条带输入框的链路
    if (command === 'import_accounts') return importAccountsViaFile();
    return Promise.reject(new Error(
      command === 'run_installer'
        ? '网页端不启动安装程序：容器内的更新请用「一键更新」（设置 → 软件更新）'
        : SHELL_UNAVAILABLE
    ));
  }

  /**
   * 网页端的「打开文件」：浏览器没有原生文件对话框，用 <input type=file> 顶替。
   *
   * 移动端的三处坑（都踩过，改动时别退回去）：
   *   · 输入框**不能** `display:none` —— iOS Safari 会忽略对隐藏 file input 的
   *     程序化 `click()`，表现成「点了没反应」。改成视觉隐藏（离屏 + 透明）。
   *   · 在系统选择器里点「取消」时 `change` 不触发，Promise 永远挂着、按钮卡在
   *     「导入中…」。用 window focus / visibilitychange 兜底：回到页面后仍没拿到
   *     文件就按取消落定（与桌面壳返回 `{canceled:true}` 同一契约）。
   *   · `accept` 只写扩展名：Android 各家文件管理器对 MIME 形（application/json）
   *     支持不一，带上它反而可能把 .json 过滤掉。
   *
   * 只负责「选到文件」，读内容与解析交给调用方 —— 整库恢复要求请求体直接放
   * File（流式上传），账号导入要读成文本解析，两者的后续步骤没有交集。
   * 解析口径与桌面壳 `commands.rs::import_accounts` 对齐，见 buildImportPayload。
   */
  function pickFile(accept) {
    return new Promise(function (resolve, reject) {
      var input = document.createElement('input');
      input.type = 'file';
      input.accept = accept;
      input.setAttribute('aria-hidden', 'true');
      input.tabIndex = -1;
      // 视觉隐藏而非 display:none（理由见上）
      input.style.position = 'fixed';
      input.style.left = '-9999px';
      input.style.top = '0';
      input.style.width = '1px';
      input.style.height = '1px';
      input.style.opacity = '0';

      var settled = false;      // 结果只落定一次
      var picked = false;       // change 是否已同步拿到文件（压过取消兜底）
      var settleTimer = null;

      function cleanup() {
        window.removeEventListener('focus', onReturn);
        document.removeEventListener('visibilitychange', onVisibility);
        if (settleTimer !== null) { window.clearTimeout(settleTimer); settleTimer = null; }
        input.remove();
      }
      function settleCancel() {
        if (settled) return;
        settled = true;
        cleanup();
        resolve({ canceled: true });
      }
      function onReturn() {
        if (settleTimer !== null) return;
        // 给系统选择器一点时间派发 change，再判定「没选」= 取消
        settleTimer = window.setTimeout(function () {
          settleTimer = null;
          if (!picked) settleCancel();
        }, 400);
      }
      function onVisibility() {
        if (document.visibilityState === 'visible') onReturn();
      }

      input.addEventListener('change', function () {
        var file = input.files && input.files[0];
        if (!file) { settleCancel(); return; }
        picked = true;          // 同步置位：兜底那条链不再当取消
        settled = true;
        cleanup();
        resolve({ canceled: false, file: file });
      });

      document.body.appendChild(input);
      window.addEventListener('focus', onReturn);
      document.addEventListener('visibilitychange', onVisibility);
      input.click();
    });
  }

  /** 网页端的「导入账号」：选 .json → 解析 → `POST /api/accounts/import` */
  async function importAccountsViaFile() {
    var picked = await pickFile('.json');
    if (picked.canceled) return { canceled: true };
    var text = await picked.file.text();
    var payload;
    try {
      payload = buildImportPayload(JSON.parse(text));
    } catch (error) {
      throw asError(error);
    }
    return call('POST', '/api/accounts/import', payload);
  }

  /**
   * 把用户选的文件整成 `POST /api/accounts/import` 的请求体。
   * 与桌面壳 `commands.rs::import_accounts` + `unwrap_envelope_file` 同口径：
   * 「导出文件」（顶层 accounts）、「裸账号数组」、以及网页端旧版把 `{success,data}`
   * 信封整个存进文件的情况 —— 三种形状都认，自定义提供商定义一并带上。
   */
  function buildImportPayload(parsed) {
    var document_ = parsed;
    if (document_ && !Array.isArray(document_)
      && !document_.accounts && document_.data && document_.data.accounts) {
      document_ = document_.data;
    }
    var accounts = Array.isArray(document_) ? document_ : (document_ && document_.accounts);
    if (!Array.isArray(accounts)) {
      throw new Error('文件里没有 accounts 数组（不是本程序导出的账号文件？）');
    }
    var body = { accounts: accounts };
    if (document_ && !Array.isArray(document_) && document_.customProviders) {
      body.customProviders = document_.customProviders;
    }
    return body;
  }

  // ── 装配对外接口 ─────────────────────────────────────────
  // __TAURI_INTERNALS__ 先装：界面里两处直接 internals.invoke('api_request')
  // （sms-login 的兜底与添加账号的 postAccount）依赖它存在。
  window.__TAURI_INTERNALS__ = {
    invoke: function (command, args) {
      return invokeShell(command, args).catch(asError).then(function (value) {
        if (value instanceof Error) throw value;
        return value;
      });
    },
    transformCallback: function (callback) {
      // 事件订阅占位：网页端没有 push 事件，返回一个无害 id 即可
      return 0;
    },
  };

  window.workbuddyDesktop = {
    // 平台标识：界面据此裁剪本机功能（导入桌面端登录态等在 web 下隐藏）。
    // 恒为 'web' —— 所有「无桌面壳」的裁剪行为只认它，本地 headless 也不
    // 搞特殊（端口面板、标题栏、整库恢复对话框继续收起）。
    platform: 'web',

    // 面板进程能否读本机桌面客户端的登录态：由 headless 入口在解析监听
    // 地址后置位（只听回环 ⇒ 访问者都在本机 ⇒ %APPDATA% 下的客户端文件
    // 可读）。导入桌面端登录态这一条链路的服务端实现（含 DPAPI 解密）本来
    // 就在网关进程里，网页端之前一律裁掉只是因为「浏览器可能连的是远程
    // 网关」；本地形态下这个前提不成立，逐项放行。
    desktopAccess: __DESKTOP_ACCESS__,

    // ── 会话 ──
    getState: function () { return call('GET', '/api/session'); },
    startLogin: function (edition, mode, provider) {
      var target = provider || 'workbuddy';
      // 小浣熊的 redirect 分支只有在浏览器以回环地址访问面板时才成立
      // （前提见 raccoonCallbackOriginProblem 的注释）。预检不过就不发
      // /start（省一个注定失败的登录任务），弹指引并 reject，由引擎 toast
      // 收尾 —— 用户看到的不再是授权页那句「授权链接无效」。
      if (target === 'raccoon') {
        var problem = raccoonCallbackOriginProblem();
        if (problem) return reportRaccoonCallbackProblem(problem);
      }
      return runLoginFlow(target, call('POST', '/api/session/login/start', {
        edition: edition || 'cn',
        provider: target,
      }));
    },
    submitLoginCallback: function (state, callbackUrl) {
      return call('POST', '/api/session/login/callback', {
        state: String(state || ''),
        callbackUrl: String(callbackUrl || ''),
      });
    },
    getLoginState: function () {
      return Promise.resolve({ active: loginActive, provider: loginProvider });
    },
    cancelLogin: async function () {
      // 桌面壳只记一个活动登录；网页端由本状态机驱动。
      //
      // 三步缺一不可（早先只做了第一步，于是「取消等待」之后粘贴回调那层
      // 覆盖层还赖在屏幕上，用户除了刷新页面别无他法）：
      //   ① 给当前这一轮打取消标记 —— pollWait 下一拍（≤3 秒）就会抛
      //      canceled，10 秒窗口期那一支也会在计时结束时直接退出；
      //   ② 收掉覆盖层：有粘贴框时走它的 cancel()，顺带发 /cancel 撤掉
      //      服务端任务（Trae 的回调端口靠这一步立即释放）；
      //   ③ 没有覆盖层（还在窗口期里）时自己发一次 /cancel。
      //
      // 取消来源在这里统一登记（externalCancel）：界面那一侧在发起取消时
      // 自己已经 toast 过一次，流程这边不要再重复提示。
      var wasActive = loginActive;
      var state = currentFlow && currentFlow.state;
      if (currentFlow) { currentFlow.canceled = true; currentFlow.externalCancel = true; }
      if (activeOverlay && activeOverlay.cancelable) {
        // 粘贴框自己会发 /cancel（见 cancelWebLogin），这里不重复发
        closeWebOverlay();
      } else {
        closeWebOverlay(true);
        if (state) {
          call('POST', '/api/session/login/cancel', { state: state }).catch(function () {});
        }
      }
      loginActive = false;
      loginProvider = '';
      emitLogin();
      return wasActive;
    },
    startAutoclawOauthLogin: function (state, authUrl, mode) {
      if (!state || !authUrl) return Promise.reject(new Error('缺少授权参数（state / authUrl）'));
      // AutoClaw OAuth 控制器与桌面 bridge 共用 `{ok, session}` 契约。
      // 网页端轮询拿到的是原始 session，直接返回会被前端误判为取消。
      // 用户取消时 runLoginFlow 给的是 `{canceled:true}`：必须映射成
      // `ok:false`，否则会被当成「拿到了一份叫 canceled 的 session」。
      return runLoginFlow('autoclaw-intl', Promise.resolve({ state: state, authUrl: authUrl }))
        .then(function (session) {
          if (session && session.canceled) return { ok: false, canceled: true };
          return { ok: true, session: session };
        });
    },
    getAutoclawOauthCaptchaConfig: function (provider) {
      return call('POST', '/api/session/login/oauth/captcha-config',
        provider ? { provider: String(provider) } : {});
    },
    startAutoclawOauth: function (provider, vendor, captchaVerifyParam) {
      return call('POST', '/api/session/login/oauth/start', {
        provider: provider ? String(provider) : undefined,
        vendor: String(vendor || ''),
        captchaVerifyParam: String(captchaVerifyParam || ''),
      });
    },
    // ── ZCode 限时套餐领取（三个薄封装，直接打账号子路径接口）──────
    // 与上面的 AutoClaw 三个方法同一形态：界面只管传参，路径与请求体形状
    // 由这里对着后端 `api::zcode_claim` 的三个处理器写死一处。
    //
    // 契约（详见 `api/zcode_claim.rs` 的模块头）：
    //   · captchaConfig 拿阿里云风控配置（前端用它初始化滑块 SDK）；
    //     返回 `{enabled:false}` 表示上游此刻不要验证码 —— 前端**不该**弹滑块；
    //   · preview 只读探测，返回 `{plans:[...], deployed}`；
    //     `deployed:false` = 活动接口尚未部署（开抢前的正常状态，不是错误）；
    //   · claim 真正领取。`captchaVerifyParam` **可以为空**：上游此刻不要验证码
    //     时它就不带那个头（发空头会被当成无效验证串，见 `claim::claim` 的注释）。
    //     **业务失败也走 200**，由 `ok:false` + `failure` 表达（前端据此选提示
    //     文案）；`claimedAt` 是领取状态的落库时刻，前端拿它把按钮切成「今日已领」。
    zcodeClaimCaptchaConfig: function (accountId) {
      return call('POST', '/api/accounts/' + encodeURIComponent(String(accountId || ''))
        + '/zcode-claim/captcha-config');
    },
    zcodeClaimPreview: function (accountId) {
      return call('POST', '/api/accounts/' + encodeURIComponent(String(accountId || ''))
        + '/zcode-claim/preview');
    },
    zcodeClaim: function (accountId, planId, captchaVerifyParam, captchaRegion) {
      return call('POST', '/api/accounts/' + encodeURIComponent(String(accountId || ''))
        + '/zcode-claim', {
        planId: planId ? String(planId) : '',
        captchaVerifyParam: String(captchaVerifyParam || ''),
        captchaRegion: captchaRegion ? String(captchaRegion) : '',
      });
    },
    // ── ZCode 活动套餐通道的验证码令牌池（见 api/zcode_captcha.rs）──────
    // 与上面三条**不是一回事**：那三条服务「领套餐」（用户动作）；这两条服务
    // **转发** —— 活动套餐的推理端点每条请求都要一个当次铸的令牌，令牌只能由
    // 界面静默铸造后推进池子（唯一调用方 ui/zcode-captcha-pool.js）。
    //
    // ★ 这两条**必须**在这里给 web 端实现，理由就是本模块存在的理由：铸造器
    // 只认 `window.workbuddyDesktop`，桌面端由 bridge.rs 注入、headless 由这里
    // 注入。早先只给了桌面端，于是 headless（Docker / 浏览器面板）下铸造器
    // 每轮都读到「没有这个方法」→ 安静退让 → 池子永远是空的，活动套餐通道
    // 整条不可用（Issue #163）。铸造本身仍发生在**浏览器**里（阿里云 SDK 跑在
    // 页面上），所以 headless 只要有人开着面板页就能维持池子 —— 与桌面端一致。
    zcodeCaptchaStats: function () {
      return call('GET', '/api/zcode/captcha');
    },
    pushZcodeCaptchaTokens: function (tokens) {
      return call('POST', '/api/zcode/captcha', {
        tokens: Array.isArray(tokens) ? tokens : [],
      });
    },
    onLoginState: function (callback) {
      loginListeners.add(callback);
      return function () { loginListeners.delete(callback); };
    },
    refreshSession: async function () {
      await call('POST', '/api/session/refresh', {});
      return call('GET', '/api/session');
    },
    logout: async function () {
      await call('POST', '/api/session/logout', {});
      return call('GET', '/api/session');
    },

    // ── 配置 ──
    getConfig: function () { return call('GET', '/api/config'); },
    saveConfig: function (payload) { return call('POST', '/api/config', payload); },

    // ── 模型清单 ──
    // 入参原样透传（与桌面 bridge 对齐）：`{accounts: {providerId: accountId},
    // providers: [id, ...]}` —— 「模型来源」点名的账号与本次刷新的范围都在里面。
    // 早先这里写死 `{}`，两个可选项一起丢了：范围收窄失效（界面上看不到的家
    // 也进结果，多出一批「缺少登录态」的噪音行），点名账号同样不生效。
    refreshModels: function (payload) { return call('POST', '/api/models/refresh', payload || {}); },
    getModelManage: function () { return call('GET', '/api/models/manage'); },
    setModelState: function (payload) { return call('POST', '/api/models/state', payload); },
    // 第 4 / 第 5 个参数（思考等级 / 映射开关）都按「有没有传」决定是否进请求体：
    // 后端按「请求体里有没有这个键」区分三态，undefined 的键不会进 JSON
    addModelMapping: function (alias, target, provider, reasoning, enabled) {
      var payload = { alias: alias, target: target, provider: provider };
      if (reasoning !== undefined) payload.reasoning = reasoning;
      if (enabled !== undefined) payload.enabled = enabled;
      return call('POST', '/api/models/mappings', payload);
    },
    removeModelMapping: function (alias, target, provider) {
      return call('POST', '/api/models/mappings/remove', { alias: alias, target: target, provider: provider });
    },
    addCustomModel: function (provider, id) { return call('POST', '/api/models/custom', { provider: provider, id: id }); },
    removeCustomModel: function (provider, id) { return call('POST', '/api/models/custom/remove', { provider: provider, id: id }); },
    // 能力位覆盖（只服务内置家）：`capabilities` 各键三态 —— 不给 = 不改、
    // null = 恢复清单原值、给值 = 覆盖。自定义家走 /api/custom-providers/models
    // 的整表保存（见后端 handler 的说明），不经过这里。
    setModelCapabilities: function (provider, id, capabilities) {
      return call('POST', '/api/models/capabilities', { provider: provider, id: id, capabilities: capabilities });
    },
    // 模型测试：真打上游、会消耗额度；payload 原样透传（与桌面 bridge 对齐），
    // 各键都可选（account_id / prompt / system_prompt / reasoning / stream / test_id）。
    // 结论失败也返回 2xx —— 上游的错误在返回值的 status / error 里
    testModel: function (payload) { return call('POST', '/api/models/test', payload || {}); },

    // ── 网关 API Key（多把）──
    getKeys: function () { return call('GET', '/api/keys'); },
    createKey: function (payload) { return call('POST', '/api/keys', payload || {}); },
    updateKey: function (id, patch) { return call('PATCH', '/api/keys/' + encodeURIComponent(id), patch); },
    deleteKey: function (id) { return call('DELETE', '/api/keys/' + encodeURIComponent(id)); },

    // ── 多账号 ──
    switchAccount: function (id) { return call('POST', '/api/accounts/current', { id: id }); },
    refreshAccountToken: function (id) { return call('POST', '/api/accounts/refresh', id ? { id: id } : {}); },
    removeAccount: function (id) { return call('DELETE', '/api/accounts/' + encodeURIComponent(id)); },
    updateAccount: function (id, patch) { return call('PATCH', '/api/accounts/' + encodeURIComponent(id), patch); },
    // 整队重排（账号页分组模式拖拽的落库通道；与桌面 bridge.rs 的同名方法成对）。
    // ★ 这个方法曾整个漏在网页桥里：界面调用点是 `bridge?.reorderAccounts?.(ids)`
    // —— 两层可选调用，缺了这一侧不报错、只是空操作，于是 Docker / 浏览器面板上
    // 拖完顺位照样弹「✅ 已保存」，刷新又跳回旧顺序（库里的顺序从没被写过）。
    // 队列是「全局一条按优先级升序尝试的队列」（见 account_store::priority），
    // 组顺位 / 组内顺位由前端展开成整条 id 数组一次性交上来，后端编 0、1、2…，
    // 所以这里原样透传，不做任何归一（空值整形与桌面桥一致）。
    reorderAccounts: function (queue) {
      return call('POST', '/api/accounts/reorder', {
        queue: Array.isArray(queue) ? queue.map(function (item) { return String(item); }) : [],
      });
    },
    moveAccount: function (id, direction) {
      return call('POST', '/api/accounts/' + encodeURIComponent(id) + '/move', {
        direction: direction === 'down' ? 'down' : 'up',
      });
    },
    clearRateLimits: function (id, model) {
      return call('POST', '/api/accounts/' + encodeURIComponent(id) + '/rate-limits/clear',
        model ? { model: model } : {});
    },
    batchAccounts: function (payload) {
      payload = payload || {};
      var body = {
        action: String(payload.action || ''),
        ids: Array.isArray(payload.ids) ? payload.ids : [],
      };
      if (Object.prototype.hasOwnProperty.call(payload, 'proxy')) body.proxy = payload.proxy;
      if (Object.prototype.hasOwnProperty.call(payload, 'maxConcurrent')) {
        body.maxConcurrent = payload.maxConcurrent;
      }
      return call('POST', '/api/accounts/batch', body);
    },

    // ── 账号路由（与 bridge.rs 的同名方法成对，缺一边会报「桥接方法缺失」）──
    getRoutingConfig: function () { return call('GET', '/api/routing'); },
    setRoutingConfig: function (patch) { return call('PUT', '/api/routing', patch || {}); },
    getRoutingQueue: function (model) {
      return call('GET', '/api/routing/queue' + (model ? '?model=' + encodeURIComponent(model) : ''));
    },

    // ── 出网代理 ──
    getProxies: function () { return call('GET', '/api/proxies'); },
    testProxy: function (payload) {
      var body = {};
      if (payload && typeof payload === 'object' && !Array.isArray(payload)) {
        if (typeof payload.id === 'string' && payload.id) body.id = payload.id;
        else if (Object.prototype.hasOwnProperty.call(payload, 'proxy')) body.proxy = payload.proxy;
      }
      return call('POST', '/api/proxies/test', body);
    },
    // Agent（本机 AI 客户端的配置接管；与桌面壳 bridge.rs 的同名方法成对
    // 存在 —— 只加一边时另一形态下的页面会报「桥接方法缺失」。契约见
    // api::agents 的模块头：写操作都返回整份 {clients}，就地替换）
    getAgents: function () { return call('GET', '/api/agents'); },
    saveAgentProfiles: function (payload) { return call('POST', '/api/agents/profiles', payload || {}); },
    applyAgentProfile: function (payload) { return call('POST', '/api/agents/apply', payload || {}); },
    previewAgentProfile: function (payload) { return call('POST', '/api/agents/preview', payload || {}); },
    restoreAgentClient: function (payload) { return call('POST', '/api/agents/restore', payload || {}); },
    importAgentClient: function (payload) { return call('POST', '/api/agents/import', payload || {}); },
    // 代理池（「网络代理」页；与桌面壳 bridge.rs 的同名方法成对存在，
    // 契约见 api::proxies 的模块头 —— 只加一边时另一形态下的页面会报
    // 「桥接方法缺失」，标题栏那一族有同样的教训）
    getProxyPool: function () { return call('GET', '/api/proxies/pool'); },
    createProxyPoolItem: function (payload) { return call('POST', '/api/proxies/pool', payload || {}); },
    updateProxyPoolItem: function (payload) { return call('POST', '/api/proxies/pool/update', payload || {}); },
    // 启用 / 禁用（三种来源都放行；订阅节点禁用时后端会走关闭监听端口流程，
    // 响应带 `listener`：none / closing / opening —— 前端据此轮询列表看端口落地）
    setProxyPoolItemEnabled: function (payload) { return call('POST', '/api/proxies/pool/enable', payload || {}); },
    removeProxyPoolItem: function (id) { return call('POST', '/api/proxies/pool/remove', { id: String(id || '') }); },
    testProxyPoolItem: function (id) { return call('POST', '/api/proxies/pool/test', { id: String(id || '') }); },
    syncClashToProxyPool: function () { return call('POST', '/api/proxies/pool/sync-clash', {}); },
    // 代理设置（「网络代理」页的设置弹窗；与桌面壳 bridge.rs 的同名方法
    // 成对存在，契约见 api::proxies 的模块头 settings 两条）
    getProxySettings: function () { return call('GET', '/api/proxies/settings'); },
    saveProxySettings: function (payload) { return call('POST', '/api/proxies/settings', payload || {}); },
    // 订阅（「网络代理」页的订阅区块；与桌面壳 bridge.rs 的同名方法成对存在，
    // 契约见 api::subscriptions 的模块头 —— 写操作后端投一条内核命令（异步），
    // 节点数与运行状态由轮询 kernel 看结果）
    getSubscriptions: function () { return call('GET', '/api/subscriptions'); },
    addSubscription: function (payload) { return call('POST', '/api/subscriptions/add', payload || {}); },
    saveSubscription: function (payload) { return call('POST', '/api/subscriptions/save', payload || {}); },
    refreshSubscription: function (id) { return call('POST', '/api/subscriptions/refresh', { id: String(id || '') }); },
    removeSubscription: function (id) { return call('POST', '/api/subscriptions/remove', { id: String(id || '') }); },
    decodeQrImage: function (payload) { return call('POST', '/api/subscriptions/decode-qr', payload || {}); },
    getKernelStatus: function () { return call('GET', '/api/subscriptions/kernel'); },
    downloadKernel: function (payload) { return call('POST', '/api/subscriptions/kernel/download', payload || {}); },
    cancelKernelDownload: function () { return call('POST', '/api/subscriptions/kernel/download/cancel', {}); },
    restartKernel: function () { return call('POST', '/api/subscriptions/kernel/restart', {}); },
    // 手动关闭内核（与桌面壳 bridge.rs 的同名方法成对存在；只停进程，订阅与端口注册表保留）
    stopKernel: function () { return call('POST', '/api/subscriptions/kernel/stop', {}); },
    removeKernel: function () { return call('POST', '/api/subscriptions/kernel/remove', {}); },
    // 文件夹选择框是**壳的能力**（web 版没有壳）：回 `unsupported`，界面据此提示
    // 手输路径 —— 只回 `canceled` 的话，点一下按钮什么都不会发生，用户会以为卡了
    pickFolder: function () { return Promise.resolve({ canceled: true, unsupported: true }); },

    // ── 积分 / 签到 ──
    getUsage: function () { return call('GET', '/api/usage'); },
    getCheckinStatus: function () { return call('GET', '/api/checkin/status'); },
    claimCheckin: function () { return call('POST', '/api/checkin', {}); },
    getAllBalances: function (id) {
      return call('GET', '/api/accounts/usage' + (id ? '?id=' + encodeURIComponent(id) : ''));
    },
    getBalancesSnapshot: function () { return call('GET', '/api/accounts/usage/snapshot'); },
    getAccountConnections: function () { return call('GET', '/api/accounts/connections'); },
    checkinAllAccounts: function (id) { return call('POST', '/api/accounts/checkin', id ? { id: id } : {}); },
    // ── Loomy 新手任务（查询 / 一键领取）──
    // 与桌面 `bridge.rs` 的同名方法成对维护：签到后界面查询任务状态、有未领取才
    // 弹窗领取（accounts-dialog-onboarding）。
    // `refresh` = 界面手点「查询任务」→ `?refresh=1` 强制实查上游；不带则吃后端
    // 的结算记忆（一次性福利领完就不再问上游，见 api::onboarding 的模块说明）。
    getOnboardingTasks: function (id, refresh) {
      return call('GET', '/api/accounts/' + encodeURIComponent(String(id || '')) + '/onboarding'
        + (refresh ? '?refresh=1' : ''));
    },
    claimOnboardingTasks: function (id) {
      return call('POST', '/api/accounts/' + encodeURIComponent(String(id || '')) + '/onboarding/claim', {});
    },
    // ── WorkBuddy 成长任务（国内版：盘点 / 单任务执行 / 开学季 / 猫猫旅行）──
    // 与桌面 `bridge.rs` 的同名方法**必须成对存在**（理由上面 Loomy 那段已述：
    // 只改一边时另一形态会静默失效）。端点在 server 的 `api::growth`。
    // 执行拆成单任务端点（60 秒转发超时，见 api::growth 模块头）。
    getGrowthStatus: function (id) {
      return call('GET', '/api/accounts/' + encodeURIComponent(String(id || '')) + '/growth');
    },
    runGrowthTask: function (id, code, mode) {
      return call('POST', '/api/accounts/' + encodeURIComponent(String(id || '')) + '/growth/task',
        { code: code, mode: mode || 'full' });
    },
    runGrowthSchool: function (id, lottery) {
      return call('POST', '/api/accounts/' + encodeURIComponent(String(id || '')) + '/growth/school',
        { lottery: lottery === true });
    },
    runGrowthTravel: function (id) {
      return call('POST', '/api/accounts/' + encodeURIComponent(String(id || '')) + '/growth/travel', {});
    },

    // ── 手机验证码登录（AutoClaw 国内版 / Loomy）──
    // 与桌面 `bridge.rs` 的同名方法**必须成对存在**（理由见下面 ZCode 那段的
    // 说明）：界面只知道「手机号 + 中间态」，走哪个端点由两份桥各自决定。
    //
    // ── 为什么这里要按 provider 选端点（曾经漏过，issue #93）──────
    // Loomy 是**另一条链路**（自己的签名算法与站点，中间态叫 msgid 而不是
    // deviceId），端点在服务端就是分开挂的（`api::session::login_loomy_*`）。
    // 只补了桌面那份桥、漏了这里时，浏览器面板发的 loomy 会落进 AutoClaw
    // 端点：验证码由 AutoClaw 发出、账号也存成 AutoClaw，界面却因为文案取自
    // 卡片 label 而显示「Loomy 账号已添加」—— 全程没有一条报错。
    sendSmsCode: function (input) {
      var isObject = input && typeof input === 'object';
      var phone = String((isObject ? input.phone : input) || '');
      var provider = isObject && input.provider ? String(input.provider) : '';
      var path = provider === 'loomy'
        ? '/api/session/login/loomy/sms/send'
        : '/api/session/login/sms/send';
      return call('POST', path, {
        phone: phone,
        provider: provider || undefined,
      });
    },
    verifySmsLogin: function (payload) {
      payload = payload || {};
      var provider = payload.provider ? String(payload.provider) : '';
      var path = provider === 'loomy'
        ? '/api/session/login/loomy/sms/verify'
        : '/api/session/login/sms/verify';
      var body = {
        phone: String(payload.phone || ''),
        code: String(payload.code || ''),
      };
      // deviceId（AutoClaw）/ msgid（Loomy）：两个中间态都按「有值才带」整形
      // —— 空串会被后端当成一个真值带上去。字段名由界面按各自链路给（见
      // ui/sms-login.js 的 SMS_PROFILES），这里只透传，不替它做归一。
      if (payload.deviceId) body.deviceId = String(payload.deviceId);
      if (payload.msgid) body.msgid = String(payload.msgid);
      if (payload.name) body.name = String(payload.name);
      if (provider) body.provider = provider;
      return call('POST', path, body);
    },

    // ── 定时签到 ──
    getAutoCheckin: function () { return call('GET', '/api/auto-checkin'); },
    saveAutoCheckin: function (patch) { return call('POST', '/api/auto-checkin', patch); },
    runAutoCheckinNow: function () { return call('POST', '/api/auto-checkin/run', {}); },
    // 定时签到的**条目**（与 bridge.rs 的同名方法成对维护）
    getCheckinSchedules: function () { return call('GET', '/api/checkin-schedules'); },
    saveCheckinSchedule: function (patch) { return call('POST', '/api/checkin-schedules', patch); },
    deleteCheckinSchedule: function (id) {
      return call('POST', '/api/checkin-schedules/' + encodeURIComponent(String(id || '')) + '/delete', {});
    },
    runCheckinSchedule: function (id) {
      return call('POST', '/api/checkin-schedules/' + encodeURIComponent(String(id || '')) + '/run', {});
    },
    // WorkBuddy 国际版日活保活的模型链（与 bridge.rs 的同名方法成对维护）
    getCheckinKeepalive: function () { return call('GET', '/api/checkin-keepalive'); },
    saveCheckinKeepalive: function (models) { return call('POST', '/api/checkin-keepalive', { models: models }); },
    // 猫猫旅行定时排程（国内版；与 bridge.rs 的同名方法成对维护）
    getTravelSchedule: function () { return call('GET', '/api/travel-schedule'); },
    saveTravelSchedule: function (patch) { return call('POST', '/api/travel-schedule', patch); },
    runTravelScheduleNow: function () { return call('POST', '/api/travel-schedule/run', {}); },
    // 成长任务每日排程（国内版；与 bridge.rs 的同名方法成对维护）
    getGrowthTaskSchedule: function () { return call('GET', '/api/growth-task-schedule'); },
    saveGrowthTaskSchedule: function (patch) { return call('POST', '/api/growth-task-schedule', patch); },
    runGrowthTaskScheduleNow: function () { return call('POST', '/api/growth-task-schedule/run', {}); },
    // WorkBuddy 国际版日活任务的手动粒度入口（mode: full | claim | keepalive）
    runCheckinActivity: function (id, mode) { return call('POST', '/api/checkin-activity', { id: id, mode: mode }); },
    // 签到中心的聚合快照（与 bridge.rs 的同名方法同一路径）
    getCheckinCenter: function () { return call('GET', '/api/checkin-center'); },
    // Qoder UMID 组件（设备风控身份）的状态与安装（与 bridge.rs 的同名方法成对）。
    // 路径挂 protected：headless 形态必须带面板凭据，所以页面只能走桥、不能裸 fetch。
    getQoderUmid: function () { return call('GET', '/api/qoder-umid'); },
    installQoderUmid: function () { return call('POST', '/api/qoder-umid/install', {}); },
    // 签到记录明细台账（查询 / 清空；参数对象里空值不带，与 bridge.rs 成对）
    getCheckinRecords: function (params) {
      const query = new URLSearchParams();
      for (const key of Object.keys(params || {})) {
        const value = params[key];
        if (value !== undefined && value !== null && value !== '') query.set(key, String(value));
      }
      const text = query.toString();
      return call('GET', '/api/checkin-records' + (text ? '?' + text : ''));
    },
    clearCheckinRecords: function (params) {
      const query = new URLSearchParams();
      for (const key of Object.keys(params || {})) {
        const value = params[key];
        if (value !== undefined && value !== null && value !== '') query.set(key, String(value));
      }
      const text = query.toString();
      return call('DELETE', '/api/checkin-records' + (text ? '?' + text : ''));
    },

    // ── 间隔型定时任务 ──
    getScheduledTasks: function () { return call('GET', '/api/scheduled-tasks'); },
    saveScheduledTask: function (id, patch) {
      return call('PATCH', '/api/scheduled-tasks/' + encodeURIComponent(String(id || '')), patch);
    },
    runScheduledTask: function (id) {
      return call('POST', '/api/scheduled-tasks/' + encodeURIComponent(String(id || '')) + '/run', {});
    },

    // ── 软件更新（检查 / 下载 / 容器内更新；只有「跑安装程序」是桌面专有）──
    checkUpdate: function () { return invokeShell('check_update'); },
    getUpdateStatus: function () { return call('GET', '/api/update/status'); },
    // 设置页「版本信息」：与桌面 bridge 的同名方法成对存在（两边必须一起加，
    // 少一边时那一半形态会报「桥接方法缺失」）。网页端没有壳侧的
    // package_info / WebView 版本，这里补的是**浏览器**那份：
    //   · runtime.webviewVersion ← 浏览器内核版本（从 UA 里取，取不到给空串）；
    //   · app.version 不覆盖 —— 网页端没有「装的是哪个包」这回事，
    //     后端编译期版本就是唯一真值。
    // 脱敏开关走查询参数（后端做，见 api::system_info 的模块头）。
    getSystemInfo: function (options) {
      var hide = !!(options && options.hideUsername);
      return call('GET', '/api/system-info' + (hide ? '?hideUsername=1' : ''))
        .then(function (info) {
          if (info && info.runtime) {
            info.runtime.webviewVersion = browserEngineVersion();
          }
          return info;
        });
    },
    // 「软件更新」的出网线路（null = 直连）：与桌面 bridge 对齐，面板下拉读写
    getUpdateProxy: function () { return call('GET', '/api/update/proxy'); },
    setUpdateProxy: function (payload) { return call('POST', '/api/update/proxy', payload); },
    // GitHub 令牌：读只报 {filled, origin}，写传 {token: '…' | null}
    getUpdateToken: function () { return call('GET', '/api/update/token'); },
    setUpdateToken: function (payload) { return call('POST', '/api/update/token', payload); },
    // 「加入 beta 版」：读只报 {enabled}，写传 {enabled: true|false}（与桌面 bridge
    // 逐字同形 —— 两侧接口名与签名必须一致，界面代码零改动地跑在两边）
    getUpdateBeta: function () { return call('GET', '/api/update/beta'); },
    setUpdateBeta: function (payload) { return call('POST', '/api/update/beta', payload); },
    downloadUpdate: function (payload) { return invokeShell('download_update', payload); },
    updateProgress: function () { return invokeShell('update_progress'); },
    cancelUpdate: function () { return invokeShell('cancel_update'); },
    runInstaller: function (path, restart) {
      return invokeShell('run_installer', { path: String(path || ''), restart: restart !== false });
    },
    openReleasePage: function (url) { return invokeShell('open_release_page', { url: String(url || '') }); },
    // ── 容器内更新（Linux headless / Docker）：安装 → 重启 ──
    // 三步走：下载（上面三条已接）→ installUpdate（校验 sha256 后原地替换
    // 二进制与 ui 目录）→ restartService（进程优雅退出，容器由 restart 策略
    // 拉起，同一个容器、可写层保留）。未开 AGENT2API_SELF_UPDATE 的部署由
    // 后端 400 拒绝；面板按 check 返回的 selfUpdate 字段决定是否显示这些入口。
    // 回滚用的是 install 留下的 .backup 备份（同一套替换逻辑的反向操作）。
    installUpdate: function () { return call('POST', '/api/update/install', {}); },
    rollbackUpdate: function () { return call('POST', '/api/update/rollback', {}); },
    restartService: function () { return call('POST', '/api/update/restart', {}); },

    // ── 出站指纹脱敏 ──
    getSanitize: function () { return call('GET', '/api/sanitize'); },
    saveSanitize: function (enabled) {
      return call('PUT', '/api/sanitize', { sanitizeBlacklistFingerprints: enabled === true });
    },

    // ── 开发者角色兼容 ──
    // 与桌面桥（src/bridge.rs 的 getDeveloperRoleCompat / saveDeveloperRoleCompat）
    // 同一映射：两边接口名与签名必须一致，界面代码零改动地跑在两边
    //（漏一侧的症状见 getClineHeaders 那段）。
    getDeveloperRoleCompat: function () { return call('GET', '/api/developer-role-compat'); },
    saveDeveloperRoleCompat: function (enabled) {
      return call('PUT', '/api/developer-role-compat', { developerRoleCompat: enabled === true });
    },

    // ── Cline 伪装头（转发头逐键覆盖）──
    // 与桌面桥（src/bridge.rs 的 getClineHeaders / saveClineHeaders）同一映射。
    // 这两个方法曾经漏在网页桥里：设置页调用 `getClineHeaders` 抛
    // 「不是函数」，面板因此一直显示「不可用」——后端接口本身是好的，
    // 缺的只是这座桥。
    getClineHeaders: function () { return call('GET', '/api/cline/headers'); },
    saveClineHeaders: function (overrides) {
      // 整体替换覆盖表（非增量 merge），与后端 PUT 同语义；界面传的就是
      // 「编辑后的整张表」。返回值与 GET 同形（后端 PUT 回的就是那份三表），
      // 界面拿它直接重画。
      return call('PUT', '/api/cline/headers', { overrides: overrides || {} });
    },

    // ── 网关面跨域访问（/v1/*，默认关）──
    getCors: function () { return call('GET', '/api/cors'); },
    saveCors: function (enabled) {
      return call('PUT', '/api/cors', { corsEnabled: enabled === true });
    },

    // ── 通知与告警（Webhook）──
    // 与壳侧 bridge.rs 的同名方法逐字同形（那侧走 Tauri IPC、这侧走同源 fetch，
    // 界面代码零改动地跑在两边，漏一侧的症状见上面 getCors 段的注释）。
    getNotify: function () { return call('GET', '/api/notify'); },
    saveNotify: function (payload) { return call('PUT', '/api/notify', payload || {}); },
    testNotify: function (payload) { return call('POST', '/api/notify/test', payload || {}); },

    // ── 系统提示词与内容拦截降级 ──
    getPrompt: function () { return call('GET', '/api/prompt'); },
    savePrompt: function (payload) {
      payload = payload || {};
      return call('PUT', '/api/prompt', {
        promptMode: payload.promptMode != null ? String(payload.promptMode) : null,
        promptFile: payload.promptFile != null ? String(payload.promptFile) : null,
        // 界面里编辑的提示词正文（空串 = 清除这一份、回落文件 / 内置默认）
        promptText: payload.promptText != null ? String(payload.promptText) : null,
        // 按提供商的覆盖：整张稀疏表原样透传（值是对象，或 null = 删掉这一家）。
        // 语义由 /api/prompt 定 —— 这里再解析一遍只会多一处可能与后端分叉的实现
        promptProviders: payload.promptProviders != null ? payload.promptProviders : null,
        // 网关自带提示词的逐家开关（另一维，值是布尔或 null）—— 同样原样透传
        promptGateway: payload.promptGateway != null ? payload.promptGateway : null,
        // 网关自带提示词的正文覆盖（`{"<id>": {identity?, stable?, dynamic?} | null}`）
        promptGatewayText: payload.promptGatewayText != null ? payload.promptGatewayText : null,
        clearDegrade: !!payload.clearDegrade,
      });
    },

    // ── 运行日志 ──
    getLogs: function (query) { return call('GET', '/api/logs' + toQuery(query)); },
    getLogStats: function () { return call('GET', '/api/logs/stats'); },
    clearLogs: function (query) { return call('DELETE', '/api/logs' + toQuery(query)); },
    exportLogs: function () { return invokeShell('export_logs'); },

    // ── 请求统计报表 / 数据保留 ──
    // dates（`{start, end}`，`YYYY-MM-DD`）只在 range=custom 时给，
    // 与桌面壳 bridge.rs 的同名方法同一口径（toQuery 负责跳过空值）
    getStatsSummary: function (range, dates) {
      return call('GET', '/api/stats/summary' + toQuery({
        range: range,
        startDate: dates && dates.start,
        endDate: dates && dates.end,
      }));
    },
    getStatsRequests: function (query) { return call('GET', '/api/stats/requests' + toQuery(query)); },
    getStatsRequestFilters: function () { return call('GET', '/api/stats/requests/filters'); },
    clearStatsRequests: function (query) { return call('DELETE', '/api/stats/requests' + toQuery(query)); },
    // 按 id 取单条请求的原始正文（详情弹窗「预览对话」的数据源；找不到给 404）
    getStatsRequestRaw: function (id) {
      return call('GET', '/api/stats/requests/raw' + toQuery({ id: id }));
    },
    // 掐掉一条在途请求（请求日志的「终止」与模型测试弹窗共用；桌面壳的
    // invoke 没有 abort，这是前端唯一能中止在途请求的手段）。
    // ★ 与上面两条 ZCode 令牌池同一类补齐：桌面端有这个桥接方法、web 端漏了，
    // 而调用点是 `?.` 可选调用 —— 漏了不会报错，只会「点了没反应」。
    terminateStatsRequest: function (id) {
      return call('POST', '/api/stats/requests/terminate' + toQuery({ id: id }));
    },
    // 清理弹窗的预览统计（与 DELETE 共用同一份筛选解析，预览与执行必须同源）
    getStatsClearPreview: function (query) {
      return call('GET', '/api/stats/requests/clear-preview' + toQuery(query));
    },
    // 后台压缩数据库：重复触发 409，进度看 clear-preview 的 vacuumRunning
    compactStatsDb: function () { return call('POST', '/api/stats/requests/compact'); },
    getRetention: function () { return call('GET', '/api/retention'); },
    saveRetention: function (patch) { return call('PUT', '/api/retention', patch); },

    // ── 面板登录（headless 托管面板才有「登录面板」的概念）──
    panelLogout: async function () {
      await call('POST', '/api/panel/logout', {});
      // 服务端已撤销会话链；本地保管的双令牌一并清掉。
      clearPanelAuth();
    },

    // ── 直连网关的凭证（AI 助手页专用；桌面 bridge 的同名方法见 bridge.rs）
    // 助手页为流式对话直接 fetch 网关基址、不经 call（见 ui-islands 的
    // assistant-state.ts），那条请求带什么凭证由这里说了算 —— 口径必须与
    // httpCall 完全一致，否则会出现「设置页能读、助手页 401」这种分叉：
    //   · 直连环境：会话在 HttpOnly cookie 里，同源请求浏览器自动带，
    //     这张表通常为空（令牌不进 JS 正是我们想保住的性质）；
    //   · 宿主中转（cookie 往返不通）：令牌在 localStorage，必须放进
    //     `x-panel-token`（自定义头被剥时还有标准的 Authorization 兜底）；
    //   · 未注册管理员的部署：用户输入的网关 Key 是唯一凭证。
    directCredentials: function () {
      var headers = {};
      var access = storedPanelToken('access');
      if (access) {
        headers['x-panel-token'] = access;
        headers['Authorization'] = 'Bearer ' + access;
      }
      if (apiKey) headers['x-api-key'] = apiKey;
      return Promise.resolve(headers);
    },

    // ── 机器人校验开关（登录 / 注册的 ALTCHA proof-of-work，分两端）──
    // 与桌面 bridge 的同名方法逐字同形：patch 是 `{captchaDesktop?, captchaWeb?}`
    // （部分更新）；旧签名传布尔值时按「两端一起改」处理（旧界面的调用形态）。
    getCaptchaSetting: function () { return call('GET', '/api/captcha'); },
    saveCaptchaSetting: function (patch) {
      return call(
        'PUT',
        '/api/captcha',
        typeof patch === 'boolean' ? { captchaEnabled: patch } : (patch || {})
      );
    },

    // ── 面板会话（网页端由浏览器自己的 cookie / localStorage 承担）──
    // 桌面壳这三个方法服务「窗口在 tauri:// 下加载登录页、令牌交给壳保管」这条
    // 通道；浏览器里同源 cookie 已经够用，这里只给出与桌面同形的结果，避免
    // 界面分端判断。
    panelSessionStatus: function () {
      return call('GET', '/api/panel/status').then(function (state) {
        if (!state || state.registered !== true) return { registered: false, authenticated: false };
        return call('GET', '/api/session').then(
          function () { return { registered: true, authenticated: true }; },
          function () {
            return call('GET', '/api/panel/status').then(function () {
              return { registered: true, authenticated: false };
            }, function () { return { registered: true, authenticated: false }; });
          }
        );
      });
    },
    panelSessionStore: function () { return Promise.resolve({ saved: true }); },
    panelSessionClear: function () { return Promise.resolve({ cleared: true }); },

    // ── 面板账户（设置页「账户」分类）──
    getPanelAccount: function () { return call('GET', '/api/panel/account'); },
    changePanelPassword: function (oldPassword, newPassword) {
      return call('POST', '/api/panel/change-password', {
        oldPassword: String(oldPassword || ''),
        newPassword: String(newPassword || ''),
      });
    },

    // ── models.dev 模型元数据（能力位与逐模型思考档位）──
    // 与上面几个开关同形：GET 读（开关 + 索引状态）、PUT 写开关；
    // refresh 那条会真打公网，返回里同时带一行结果与刷新后的状态。
    getModelsDev: function () { return call('GET', '/api/models/metadata'); },
    saveModelsDev: function (on) {
      return call('PUT', '/api/models/metadata', { modelsDevEnabled: on === true });
    },
    refreshModelsDev: function () { return call('POST', '/api/models/metadata/refresh'); },

    // ── 数据存储概况 ──
    getStorage: function () { return call('GET', '/api/storage'); },

    // ── 云同步（设置页「数据 → 云同步」）──
    // 桌面端的上传 / 下载走壳命令（为了放宽超时），网页端直接打接口：
    // fetch 没有总超时，长传输由服务端自己的传输层预算控制。
    getCloudSync: function () { return call('GET', '/api/cloud-sync'); },
    saveCloudSync: function (payload) { return call('PUT', '/api/cloud-sync', payload || {}); },
    testCloudSync: function (payload) { return call('POST', '/api/cloud-sync/test', payload || {}); },
    getCloudSyncRemote: function () { return call('GET', '/api/cloud-sync/remote'); },
    cloudSyncUpload: function () { return call('POST', '/api/cloud-sync/upload', {}); },
    cloudSyncDownload: function () { return call('POST', '/api/cloud-sync/download', {}); },
    importCloudSyncKey: function (password) {
      return call('POST', '/api/cloud-sync/import-key', { password: password });
    },

    // ── 数据结构升级 ──
    getUpgrade: function () { return call('GET', '/api/upgrade'); },
    runUpgrade: function () { return call('POST', '/api/upgrade/run', {}); },

    // ── 请求重试 ──
    getRetry: function () { return call('GET', '/api/retry'); },
    saveRetry: function (patch) { return call('PUT', '/api/retry', patch); },
    // ── 上游请求超时（四项）──
    getTimeouts: function () { return call('GET', '/api/timeouts'); },
    saveTimeouts: function (patch) { return call('PUT', '/api/timeouts', patch); },
    // ── 排队等待（次数 / 单次秒数）──
    getQueue: function () { return call('GET', '/api/queue'); },
    saveQueue: function (patch) { return call('PUT', '/api/queue', patch); },
    // ── 验证码令牌池（ZCode 活动套餐通道的补货节奏）──
    getCaptchaPool: function () { return call('GET', '/api/zcode/captcha-pool'); },
    saveCaptchaPool: function (patch) { return call('PUT', '/api/zcode/captcha-pool', patch); },

    // ── 调试模式 ──
    getDebug: function () { return call('GET', '/api/debug'); },
    saveDebug: function (enabled) { return call('PUT', '/api/debug', { debugMode: enabled }); },
    getDebugTraffic: function (id) {
      return call('GET', '/api/debug/traffic?id=' + encodeURIComponent(id));
    },

    // ── 事件 ──
    onStateChanged: function (callback) {
      stateListeners.add(callback);
      return function () { stateListeners.delete(callback); };
    },
    // 后端主动推送的维护提醒在网页端没有对应物：订阅合法但不触发
    onAutoMaintained: function () { return function () {}; },
    onBackendError: function () { return function () {}; },

    // ── 壳特有：后端就绪与端口处置 ──
    getBackendStatus: function () { return invokeShell('backend_status'); },
    getPortOccupant: function () { return invokeShell('port_occupant'); },
    endPortOccupant: function () { return Promise.reject(new Error(SHELL_UNAVAILABLE)); },
    checkPort: function () {
      return Promise.reject(new Error('网页端不探测端口：端口由 AGENT2API_PROXY_PORT 环境变量决定'));
    },
    changePort: function () {
      return Promise.reject(new Error('网页端不支持改端口：请设置环境变量 AGENT2API_PROXY_PORT 后重启容器'));
    },
    restartApp: function () {
      return Promise.reject(new Error('网页端不支持重启：请重启容器（docker compose restart）'));
    },

    // ── 窗口主题：没有窗口主题可钉，交给系统/浏览器偏好 ──
    setWindowTheme: function () { return Promise.resolve(); },
    // ── 界面缩放：网页端的缩放归浏览器自己（Ctrl +/- 与浏览器菜单的缩放档位）──
    // 空实现而不是拒绝：设置页里这一项在网页端是禁用状态、根本点不到，
    // 只有 app.js 的启动应用会调到这里 —— 抛错只会在控制台留一条无意义的噪声。
    setZoom: function () { return Promise.resolve(); },

    // ── 自定义标题栏的窗口三键：网页端没有应用窗口 ──
    // 按「能映射则映射，不能则明确拒绝」的惯例处理：三个动作明确拒绝，
    // isMaximized 是查询而非动作，照 backend_status 的口径返回常态 false；
    // 事件订阅照 onAutoMaintained 的口径返回空操作。
    // 实际上标题栏在网页端根本不渲染（titlebar.js 的 platform 守卫：
    // 本 shim 注入 platform='web'），这组方法只是兜底防误调。
    windowMinimize: function () {
      return Promise.reject(new Error(SHELL_UNAVAILABLE + '：浏览器里没有应用窗口'));
    },
    windowToggleMaximize: function () {
      return Promise.reject(new Error(SHELL_UNAVAILABLE + '：浏览器里没有应用窗口'));
    },
    windowClose: function () {
      return Promise.reject(new Error(SHELL_UNAVAILABLE + '：浏览器里没有应用窗口'));
    },
    windowIsMaximized: function () { return Promise.resolve(false); },
    onWindowResize: function () { return function () {}; },

    // ── 应用设置与账号导入导出 ──
    getAppSettings: function () { return invokeShell('get_app_settings'); },
    saveAppSettings: function (patch) {
      // 无处持久化也不该报错：界面保存成功即可（本次会话内忽略）
      return Promise.resolve(Object.assign({ closeToTray: false, autostart: false, proxyPort: 0, lightweightMode: false }, patch));
    },
    exportAccounts: function () { return invokeShell('export_accounts'); },
    importAccounts: function () { return invokeShell('import_accounts'); },

    // ── 整库备份 / 恢复 ──
    // 桌面壳走系统文件对话框（bridge.rs 的 exportDatabase / importDatabase），
    // 网页端只能「下载附件」与「上传文件」，对应后端新加的一对 HTTP 端点
    // （见 api::backup_api 的模块头）。返回形状与桌面壳逐字段一致，设置页
    // 的提示文案与结果区两端共用。
    exportDatabase: function (includeLogs) { return exportDatabase(includeLogs); },
    importDatabase: function () { return importDatabase(); },
  };

  // 把筛选条件转成查询串（与桌面 bridge 的 toQuery 同语义：空值跳过）
  function toQuery(query) {
    if (!query) return '';
    if (typeof query === 'string') {
      return query.indexOf('?') === 0 || query === '' ? query : '?' + query;
    }
    if (query instanceof URLSearchParams) {
      var text = query.toString();
      return text ? '?' + text : '';
    }
    var params = new URLSearchParams();
    for (var key in query) {
      if (!Object.prototype.hasOwnProperty.call(query, key)) continue;
      var value = query[key];
      if (value === undefined || value === null || value === '') continue;
      params.append(key, String(value));
    }
    var result = params.toString();
    return result ? '?' + result : '';
  }

  // 写类请求成功后重放一次状态（模拟桌面的 accounts:state-changed 推送）：
  // 拦一层 call，只对管理 API 的非 GET 生效，/v1/* 与登录轮询不受影响。
  var rawCall = call;
  call = function (method, path, body) {
    var promise = rawCall(method, path, body);
    if (method !== 'GET' && path.indexOf('/api/') === 0) {
      promise.then(function () { emitStateSoon(); }, function () {});
    }
    return promise;
  };
})();
"#;
