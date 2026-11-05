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

/// 注入进 index.html 的桥接脚本全文（在所有界面脚本之前执行）。
///
/// 用 `r#"…"#` 原始字符串：脚本里不出现 `"#` 序列（字符串一律单引号），
/// 不需要任何转义；占位符也没有 —— 平台标识直接写死 `'web'`。
pub fn shim_js() -> &'static str {
    r#"(function () {
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
  /**
   * 本地记下的 access 到期时刻（毫秒；0 = 不知道）。
   *
   * **不是凭据**：它只回答「该不该在下个请求前先续期」（见 `ensureFreshSession`）。
   * 服务端仍然按会话表判有效期，这里算错只会退回「401 → 续期」的老路径。
   * 存在同一份 PANEL_AUTH 里而不是单开一个键：清会话时一处删干净。
   *
   * 这个变量只是**存储不可用时的内存兜底**（隐私模式）：正常情况下一律以
   * localStorage 为准 —— 多标签页共用一份罐子，另一个标签页续期后这里必须
   * 立刻看到新的到期时刻，不能按页面加载时那份旧值去判断。
   */
  var panelExpiresAt = 0;
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
  /** 当前的到期时刻：localStorage 优先，存储不可用时回落到内存值 */
  function currentExpiry() {
    var auth = readPanelAuth();
    var stored = Number(auth && auth.expiresAt);
    if (isFinite(stored) && stored > 0) return stored;
    return panelExpiresAt;
  }

  /** 记下到期时刻。`expiresIn` 是服务端下发的剩余秒数（见 panel.rs issue_response） */
  function rememberExpiry(expiresIn) {
    var seconds = Number(expiresIn);
    if (!isFinite(seconds) || seconds <= 0) return;
    panelExpiresAt = Date.now() + seconds * 1000;
    var auth = readPanelAuth() || {};
    auth.expiresAt = panelExpiresAt;
    try { localStorage.setItem(PANEL_AUTH, JSON.stringify(auth)); }
    catch (e) { /* 隐私模式等：本次会话内内存值仍然有效 */ }
  }

  function storePanelAuth(access, refresh) {
    try {
      localStorage.setItem(PANEL_AUTH, JSON.stringify({
        access: access, refresh: refresh, expiresAt: panelExpiresAt || currentExpiry(),
      }));
    } catch (e) { /* 隐私模式等：留在本次调用链即可 */ }
  }
  function clearPanelAuth() {
    panelExpiresAt = 0;
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
    // url 已过登录导航校验；仍逐项转义，查询串只能当文本/属性，不能插入 HTML。
    ensureOverlay(
      '浏览器拦截了弹出窗口',
      '<div>请点击下面的链接打开授权页：</div>'
      + '<a href="' + escapeHtml(url) + '" target="_blank" rel="noopener noreferrer" '
      + 'style="color:#7fa7ff;word-break:break-all;">' + escapeHtml(url) + '</a>',
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

  // 只供没有 options 的旧调用使用；动态流程完全取所选 method 的 metadata。
  function legacyNeedsManualCallback(provider) {
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

  function hasMethodIndex(options) {
    return !!(options && Object.prototype.hasOwnProperty.call(options, 'methodIndex'));
  }

  // GET 动态元数据同时完成运行时 provider/索引校验，不复制 BUILTIN 或放行任意 id。
  async function selectedLoginMethod(provider, options) {
    if (!Number.isSafeInteger(options.methodIndex) || options.methodIndex < 0) {
      throw new Error('登录方式索引必须是非负整数');
    }
    var metadata = await call('GET', '/api/session/login/methods?provider=' + encodeURIComponent(provider));
    var method = metadata && Array.isArray(metadata.methods)
      && metadata.methods.find(function (item) { return item && item.methodIndex === options.methodIndex; });
    if (!method) throw new Error('该登录方式已不在提供商的运行时清单中，请重新选择');
    return {
      manualCallback: method.manualCallback === true,
      callbackInput: method.callbackInput === 'code' ? 'code' : 'url',
      localOnly: method.localOnly === true,
      instructions: typeof metadata.instructions === 'string' ? metadata.instructions : '',
    };
  }

  function loginCallbackTargets(authUrl) {
    if (!authUrl) return [];
    var auth;
    try { auth = new URL(authUrl); } catch (e) { return []; }
    var targets = [];
    var collect = function (value, key) {
      if (['redirect_uri', 'redirect', 'redirect_url', 'callback_url', 'auth_callback_url',
        'navigate_uri', 'return_url'].indexOf(key) < 0) return;
      try {
        var target = new URL(value);
        if (target.hostname && !target.username && !target.password) targets.push(target);
      } catch (e) { /* 非 URL 的参数不构成合法 callback */ }
    };
    auth.searchParams.forEach(collect);
    var question = auth.hash.indexOf('?');
    if (question >= 0) new URLSearchParams(auth.hash.slice(question + 1)).forEach(collect);
    return targets;
  }

  function checkedLoginUrl(value, started, targets) {
    if (typeof value !== 'string' || /[\u0000-\u0020\u007f]/.test(value)) {
      throw new Error('登录链接包含无效字符');
    }
    var parsed;
    try { parsed = new URL(value); } catch (e) { throw new Error('登录链接无效'); }
    if (!parsed.hostname || parsed.username || parsed.password) throw new Error('登录链接无效');
    var hosts = started && started.allowedHosts;
    if (hosts != null && (!Array.isArray(hosts) || hosts.some(function (host) {
      return typeof host !== 'string' || !host.trim().replace(/^\*\./, '').replace(/\.$/, '');
    }))) throw new Error('登录域名白名单格式无效');
    var regularHosts = [];
    var loopbackPages = [];
    (Array.isArray(hosts) ? hosts : []).forEach(function (host) {
      if (typeof host !== 'string') return;
      var text = host.trim().toLowerCase();
      // 插件申报的本机登录页（明确带端口，只认回环主机）
      var page = /^(127\.0\.0\.1|localhost|\[::1\]):(\d+)$/.exec(text);
      if (page) { loopbackPages.push({ host: page[1], port: page[2] }); return; }
      var allowed = text.replace(/^\*\./, '').replace(/\.$/, '');
      if (allowed) regularHosts.push(allowed);
    });
    var permittedHost = !Array.isArray(hosts) || regularHosts.length === 0 || regularHosts.some(function (allowed) {
      return parsed.hostname.toLowerCase() === allowed
        || parsed.hostname.toLowerCase().endsWith('.' + allowed);
    });
    var callbackHttp = parsed.protocol === 'http:'
      && ['localhost', '127.0.0.1', '[::1]'].indexOf(parsed.hostname) >= 0
      && (targets || []).some(function (target) {
        return target.protocol === parsed.protocol && target.hostname === parsed.hostname
          && target.port === parsed.port && target.pathname === parsed.pathname;
      });
    var loopbackPage = parsed.protocol === 'http:'
      && loopbackPages.some(function (page) {
        return parsed.hostname.toLowerCase() === page.host && (parsed.port || '80') === page.port;
      });
    if (!permittedHost || (parsed.protocol !== 'https:' && !callbackHttp && !loopbackPage)) {
      throw new Error('登录链接不是允许的 HTTPS 站点或合法本机回调地址');
    }
    return value; // 校验不重写原文，保留授权地址中的编码、顺序与签名。
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
    // 通用中转路由：网关只把查询串转给插件，参数名与协议形态由插件解释
    var callbackUrl = new URL('/api/session/login/remote-callback', window.location.origin);
    callbackUrl.searchParams.set('state', state);
    authorizeUrl.searchParams.set('login_source', 'web');
    authorizeUrl.searchParams.set('redirect', callbackUrl.toString());
    return authorizeUrl.toString();
  }

  async function waitForManualCallback(provider, flow, authUrl, waitPromise) {
    var label = window.wbProviders && window.wbProviders.labelOf
      ? window.wbProviders.labelOf(provider) : provider;
    var codeInput = flow.callbackInput === 'code';
    var callbackInstruction = codeInput
      ? '仅粘贴此登录方式要求的<strong>本人已授权令牌或一次性 code</strong>；不是设备码或完整回调地址。'
        + '正文将原样通过受保护的 POST 提交；接受后仍须等待网关确认并保存账号。'
      : '<strong>如果授权完成后账号没有自动出现</strong>，'
        + '复制浏览器地址栏中的<strong>完整回调地址</strong>粘贴提交；'
        + '不要改动参数。回调被接受后仍须等待网关确认并保存账号。';
    var linkStyle = 'flex:1;padding:7px 0;border:1px solid #3a3b3f;border-radius:6px;'
      + 'background:transparent;color:#c9c9c9;font-size:13px;cursor:pointer;';
    while (currentFlow === flow && !flow.canceled) {
      // 空 URL 流程也能手工交回 code；不渲染空链接，更不能拿它当完成信号。
      var authLinkHtml = authUrl ? '<div style="margin-bottom:6px;color:#9a9a9a;">授权页地址</div>'
        + '<code style="display:block;margin-bottom:8px;padding:8px 10px;border-radius:6px;'
        + 'background:#26272b;color:#b8c7e6;font-size:12px;line-height:1.5;'
        + 'word-break:break-all;max-height:96px;overflow:auto;">' + escapeHtml(authUrl) + '</code>'
        + '<div style="display:flex;gap:8px;margin-bottom:14px;">'
        + '<button type="button" data-a2a-act="open" style="' + linkStyle + '">打开授权页</button>'
        + '<button type="button" data-a2a-act="copy" style="' + linkStyle + '">复制链接</button>'
        + '</div>' : '';
      var callbackUrl = await ensureOverlay(
        label + (codeInput ? '需要粘贴授权令牌或 code' : '需要粘贴回调地址'),
        '<div style="margin-bottom:10px;">' + callbackInstruction + '</div>'
        + '<div data-a2a-login-progress style="white-space:pre-wrap;margin-bottom:10px;">'
        + escapeHtml(loginProgressText(flow)) + '</div>'
        + authLinkHtml
        + '<input type="password" spellcheck="false" autocomplete="off" autocapitalize="off" autocorrect="off" '
        + 'placeholder="' + (codeInput ? '此方式要求的授权令牌或一次性 code' : '本次登录的完整回调地址') + '" '
        + 'style="width:100%;box-sizing:border-box;padding:8px 10px;border:1px solid #3a3b3f;'
        + 'border-radius:6px;background:#26272b;color:#e8e8e8;">',
        codeInput ? '提交授权令牌或 code' : '提交回调地址',
        {
          cancelText: '取消登录',
          onMount: function (body) {
            var openBtn = body.querySelector('[data-a2a-act="open"]');
            var copyBtn = body.querySelector('[data-a2a-act="copy"]');
            if (openBtn) openBtn.addEventListener('click', function () {
              if (currentFlow !== flow || flow.canceled) return;
              try {
                var checked = checkedLoginUrl(authUrl, flow.dynamic ? flow.started : null, flow.callbackTargets);
                window.open(checked, '_blank', 'noopener,noreferrer');
              } catch (e) {
                flow.hint = '浏览器无法打开授权页，请复制上方链接手动打开。';
                emitLogin();
              }
            });
            if (copyBtn) copyBtn.addEventListener('click', function () {
              copyText(authUrl).then(function () {
                copyBtn.textContent = '已复制';
                setTimeout(function () {
                  if (copyBtn.isConnected) copyBtn.textContent = '复制链接';
                }, 1500);
              }, function () { copyBtn.textContent = '请手动复制上方链接'; });
            });
          },
        }
      );
      if (callbackUrl === DISPOSED || currentFlow !== flow) {
        var takenOver = canceledError();
        takenOver.silent = true;
        throw takenOver;
      }
      if (callbackUrl == null) return cancelWebLogin(flow.state);
      // 完整 URL 和裸 code 保真传递，只用 trim 判空，不裁剪/解码/重组正文。
      callbackUrl = String(callbackUrl);
      if (!callbackUrl.trim()) continue;
      var submitted;
      try {
        if (currentFlow !== flow || flow.canceled) throw canceledError();
        submitted = await call('POST', '/api/session/login/callback', {
          state: flow.state, callbackUrl: callbackUrl,
        });
        callbackUrl = '';
        if (currentFlow !== flow || flow.canceled) throw canceledError();
        if (submitted && typeof submitted.nextUrl === 'string' && submitted.nextUrl) {
          authUrl = checkedLoginUrl(submitted.nextUrl, flow.dynamic ? flow.started : null, flow.callbackTargets);
          continue;
        }
      } catch (error) {
        callbackUrl = '';
        if (error && error.canceled) throw error;
        var failureMessage = codeInput ? '授权令牌或 code 未被接受；请检查此方式的要求。'
          : error && error.message ? error.message : error;
        var retry = await ensureOverlay(
          '回调提交失败',
          '<div style="margin-bottom:4px;">' + escapeHtml(failureMessage)
          + '</div><div>' + (codeInput
            ? '请确认粘贴的是此方式要求的本人授权令牌或一次性 code，不是设备码或回调地址，再重新提交。'
            : '请确认复制的是本次授权完成后的完整回调地址，再重新提交。') + '</div>',
          '重新填写', { cancelText: '取消登录' }
        );
        if (retry === DISPOSED || currentFlow !== flow) {
          var retryTakenOver = canceledError();
          retryTakenOver.silent = true;
          throw retryTakenOver;
        }
        if (retry == null) return cancelWebLogin(flow.state);
        continue;
      }
      var pendingHint = submitted && (typeof submitted.hint === 'string' ? submitted.hint
        : submitted.pending && submitted.pending.hint);
      if (typeof pendingHint === 'string') flow.hint = pendingHint;
      else flow.hint = '回调已提交，正在等待网关确认登录并保存账号…';
      emitLogin();
      // accepted/Pending/甚至 callback 的 Done 都不是这里的成功出口：只复用
      // 同一个 /wait Promise，避免双轮询消费流程或虚报「已添加」。
      return await waitPromise;
    }
    throw canceledError();
  }

  /** 用户主动取消粘贴回调：通知服务端撤掉这一轮登录任务，并向上抛一个
   *  可被界面识别为「取消」的结果。
   *
   *  为什么要真的打 /cancel：Trae 那条链在网关侧持有回调监听端口，不撤要占满
   *  5 分钟超时才释放，用户随即重新发起会抢不到端口（与 `LoginService::cancel`
   *  注释里 AutoClaw / CodeArts 同一个坑）。 */
  async function cancelWebLogin(state) {
    if (currentFlow && currentFlow.state === state) currentFlow.canceled = true;
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
  async function httpCall(method, path, body, retried, selectedKey) {
    // 到期前先把令牌换掉：这一步让「access 过期 → 401 → 续期」不再发生，
    // 也就没有那条每 2 小时一次的 401（见 `ensureFreshSession`）。
    // 只在首轮做：重试那一趟（retried=true）刚刚续过期，再判一次纯属多余。
    if (!retried) await ensureFreshSession();
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
    if (selectedKey !== undefined) headers['x-api-key'] = selectedKey;
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
        if (refreshed) return httpCall(method, path, body, true, selectedKey);
        // 续期失败 = 会话链真的没了：清掉本地令牌再跳登录页，
        // 否则下次进来还带着死 token，永远是同一轮失败。
        clearPanelAuth();
        window.location.href = '/login';
        // 页面即将整页跳转，返回一个挂起的承诺占位
        return new Promise(function () {});
      }
      var key = selectedKey === undefined ? await askForKey() : null;
      if (key) {
        storeKey(key);
        return httpCall(method, path, body, true, selectedKey);
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

  /**
   * 主动续期的提前量（毫秒）：到期前这么久就把令牌换掉。
   *
   * 两分钟够覆盖「这一轮请求刚好在到期瞬间发出」的抖动，又远小于 2 小时的
   * 寿命 —— 不会因为时钟偏差或一次慢请求就提前换掉刚签发的令牌（那会白白
   * 消耗一次轮换，且让服务端的重放宽限窗频繁生效）。
   */
  var REFRESH_AHEAD_MS = 2 * 60 * 1000;

  /**
   * 到期前主动续期：把「access 过期 → 401 → 续期」这条路径提前到过期之前。
   *
   * ── 为什么要有这一步 ────────────────────────────────────────
   * 只靠 401 被动续期时，**每次** access 过期都必然产生一条 401。面板里频率
   * 最高的轮询是验证码令牌池（2.5 秒一轮，见 ui/zcode-captcha-pool.js），
   * 它永远是第一个撞上的那个 —— 于是每 2 小时准时一条 401、一条日志、徽标 +1。
   * 续期本身是成功的（用户无感），但那行日志按旧口径记成 error，看起来像故障。
   * 服务端已经按「refresh 还活着」把这类 401 降成 verbose 了；这里再从源头
   * 把请求本身消掉：用户挂着面板一整天，日志里不该出现任何一条这种东西。
   *
   * ── 失败怎么办 ──────────────────────────────────────────────
   * 一律**静默放行**，让请求照常发出去：`doRefresh` 返回 false 可能是会话链
   * 真没了（该走 401 → 跳登录页），也可能只是一次网络抖动（下一轮就会重试）。
   * 在这里抛错会把「网关偶尔抖一下」变成一屏报错，而真正的判据（会话有效吗）
   * 只有服务端知道 —— 交回给既有的 401 分支处理。
   *
   * 不记 `expiresIn` 时（旧版服务端、或从没续过期）`panelExpiresAt` 是 0，
   * 这里直接跳过，行为与改造前逐字一致。
   */
  async function ensureFreshSession() {
    // 每次现读：另一个标签页可能刚续过期，按页面加载时那份旧值判断会白换一次
    var expiresAt = currentExpiry();
    if (!expiresAt) return;
    if (Date.now() < expiresAt - REFRESH_AHEAD_MS) return;
    // 续期端点自己也要过这道门 —— 但它的 401 分支会再调 tryRefresh，
    // 递归下去没有意义（而且 `/api/panel/refresh` 是 public，根本不会 401）
    await tryRefresh();
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
      // 到期时刻两种模式都有（服务端恒定下发 expiresIn）：直连 cookie 模式下
      // 响应体里没有令牌，但「什么时候该主动续期」照样要记 —— 否则直连环境
      // 只能靠 401 才发现过期，正是本次要消掉的那条噪音。
      if (data) rememberExpiry(data.expiresIn);
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
  var loginProgressListeners = new Set();
  function loginProgressText(flow) {
    return flow ? [flow.instructions].concat(flow.notices || [], [flow.hint]).filter(function (value) {
      return typeof value === 'string' && value;
    }).join('\n') : '';
  }
  function loginSnapshot() {
    var flow = loginActive && currentFlow;
    return {
      active: loginActive, provider: loginProvider,
      mode: flow ? flow.mode : null, edition: flow ? flow.edition : null,
      instructions: flow ? [flow.instructions].concat(flow.notices || []).filter(Boolean).join('\n') : '',
      hint: flow ? flow.hint || '' : '',
      state: flow && flow.dynamic && flow.state ? flow.state : null,
      manualCallback: !!(flow && flow.dynamic && flow.manualCallback),
      callbackInput: flow && flow.dynamic ? flow.callbackInput : 'url',
    };
  }
  function emitLogin() {
    var payload = loginSnapshot();
    var notify = function (cb) { try { cb(payload); } catch (e) { /* 不记录可能含登录 URL 的异常 */ } };
    loginListeners.forEach(notify);
    loginProgressListeners.forEach(notify);
    // 复用安全粘贴 dialog 的纯文本节点，轮询进度不能作为 HTML 解释。
    if (activeOverlay) {
      var text = activeOverlay.overlay.querySelector('[data-a2a-login-progress]');
      if (text) text.textContent = loginProgressText(loginActive && currentFlow);
    }
  }

  var sleep = function (ms) { return new Promise(function (r) { setTimeout(r, ms); }); };

  // 超时文案必须给出下一步。对走 loopback 回调的几家来说，最常见的原因不是
  // 「忘了授权」，而是浏览器与网关不在同一台机器上（服务器 / Docker 部署）：
  // 回环地址属于用户本机，回调永远到不了网关，只能把地址栏整条 URL 粘回来。
  // 只说「等待超时」，用户不知道该干什么。
  var MANUAL_CALLBACK_TIMEOUT_HINT = '：如果浏览器与网关不在同一台机器上，'
    + '授权完成后的回调会落到你本机而不是网关 —— 请复制浏览器地址栏里的'
    + '完整回调地址，粘回面板重试。';

  async function pollWait(flow) {
    while (Date.now() < flow.deadline) {
      if (currentFlow !== flow || flow.canceled) throw canceledError();
      var wait;
      try {
        wait = await call('GET', '/api/session/login/wait?state=' + encodeURIComponent(flow.state));
      } catch (error) {
        if (currentFlow !== flow || flow.canceled) throw canceledError();
        throw error;
      }
      if (currentFlow !== flow || flow.canceled) throw canceledError();
      if (wait && wait.done === true && !wait.pending) {
        if (wait.error) throw asError(wait.error);
        return wait.session == null ? {} : wait.session;
      }
      if (wait) {
        var hint = typeof wait.hint === 'string' ? wait.hint : wait.pending && wait.pending.hint;
        var instructions = typeof wait.instructions === 'string' ? wait.instructions : flow.instructions;
        var nextHint = typeof hint === 'string' ? hint : flow.hint;
        if (instructions !== flow.instructions || nextHint !== flow.hint) {
          flow.instructions = instructions;
          flow.hint = nextHint;
          emitLogin();
        }
      }
      await sleep(flow.pollIntervalMs);
    }
    var timeoutHint = flow.callbackInput === 'code'
      ? '：请重新发起，并按此方式要求粘贴本人已授权的令牌或一次性 code。'
      : MANUAL_CALLBACK_TIMEOUT_HINT;
    throw new Error('登录等待超时（5 分钟）' + (flow.manualCallback ? timeoutHint : ''));
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

  /** 旧流程保留用户手势占位；动态流程要先确认 URL 非空，绝不空开 browser。 */
  async function runLoginFlow(provider, startRequest, config) {
    if (loginActive || currentFlow) throw new Error('已有登录流程在进行，请先完成或取消当前登录');
    config = config || {};
    var dynamic = config.dynamic === true;
    var popup = null;
    var flow = {
      state: '', canceled: false, dynamic: dynamic, mode: config.mode || 'embedded', edition: config.edition || 'cn',
      instructions: '', hint: '', notices: [], manualCallback: false, callbackInput: 'url', started: null, callbackTargets: [],
      pollIntervalMs: 3000, deadline: 0,
    };
    currentFlow = flow;
    loginActive = true;
    loginProvider = provider;
    emitLogin();
    if (!dynamic) {
      try {
        popup = window.open('about:blank', 'a2a-login');
        if (popup) popup.opener = null;
      } catch (e) { /* 拦截时走兜底 */ }
    }
    try {
      // 工厂延后到全局锁生效后执行，避免第二次调用先 POST 再被锁拒绝。
      var started = await (typeof startRequest === 'function' ? startRequest(flow) : startRequest);
      flow.started = started || {};
      flow.state = String(started && (started.state || started.flow) || '');
      if (!flow.state.trim()) throw new Error('网关未返回登录流程状态');
      if (flow.canceled || currentFlow !== flow) await cancelWebLogin(flow.state);
      var authUrl = started && Object.prototype.hasOwnProperty.call(started, 'url')
        ? started.url : started && started.authUrl;
      if (authUrl == null) authUrl = '';
      if (typeof authUrl !== 'string') throw new Error('网关返回的登录链接格式无效');
      var hasUrl = !!authUrl.trim();
      flow.manualCallback = dynamic && typeof started.manualCallback === 'boolean' ? started.manualCallback
        : dynamic ? !!(flow.method && flow.method.manualCallback) : legacyNeedsManualCallback(provider);
      flow.callbackInput = dynamic && flow.manualCallback
        && (started.callbackInput || flow.method && flow.method.callbackInput) === 'code' ? 'code' : 'url';
      flow.instructions = typeof started.instructions === 'string' && started.instructions
        ? started.instructions : flow.method && flow.method.instructions || '';
      if (flow.method && flow.method.localOnly) {
        flow.notices.push('此方式读取的是网关所在机器的本机客户端账号，不是当前浏览器所在电脑的账号；网页仍可发起。');
      }
      var onlyExternal = started.externalBrowser === true;
      flow.mode = !hasUrl ? 'auto' : 'external';
      if (dynamic && onlyExternal && config.mode === 'embedded' && hasUrl) {
        flow.notices.push('该供应商只支持外部浏览器，授权页将在浏览器标签页中打开，不会伪装成内嵌窗口。');
      }
      var interval = Number(started.pollIntervalMs);
      // 插件 pollIntervalMs=0 是回调型，不代表关闭网关 /wait；不用于猜 manualCallback。
      flow.pollIntervalMs = Number.isFinite(interval) && interval > 0
        ? Math.max(1000, Math.min(30000, interval)) : 3000;
      flow.deadline = Date.now() + 5 * 60 * 1000;
      if (hasUrl) {
        if (!dynamic) authUrl = buildRaccoonWebAuthUrl(provider, flow.state, authUrl);
        flow.callbackTargets = loginCallbackTargets(authUrl);
        // 旧版没有白名单契约，保留原 SSO 行为；动态版严格采用真实 allowedHosts。
        authUrl = checkedLoginUrl(authUrl, dynamic ? started : null, flow.callbackTargets);
        if (popup && !popup.closed) popup.location.href = authUrl;
        else {
          var opened = false;
          try {
            // 先拿同源空页句柄再断开 opener；直接 noopener 会在成功时也返回 null，
            // 不能据此误报「被拦截」。这一步只在已确认 URL 非空且合法后执行。
            var second = window.open('about:blank', '_blank');
            if (second) {
              second.opener = null;
              second.location.replace(authUrl);
              opened = true;
            }
          } catch (e) { /* 显示安全链接兜底 */ }
          if (!opened) showLinkFallback(authUrl);
        }
      } else if (popup && !popup.closed) {
        popup.close();
        popup = null;
      }
      emitLogin();
      var waiting = pollWait(flow);
      if (!flow.manualCallback) return await waiting;
      // 一个 /wait Promise 与安全粘贴 dialog 竞速，accepted 后仍等待同一个终态。
      var outcome = await Promise.race([
        waiting.then(function (session) { return { session: session }; },
          function (error) { return { error: error }; }),
        (async function () {
          await sleep(MANUAL_CALLBACK_GRACE_MS);
          if (currentFlow !== flow || flow.canceled) throw canceledError();
          return waitForManualCallback(provider, flow, authUrl, waiting);
        })().then(function (session) { return { session: session }; },
          function (error) { return { error: error }; }),
      ]);
      if (outcome.error) throw outcome.error;
      return outcome.session;
    } catch (error) {
      if (flow.canceled || (error && error.canceled)) {
        var quiet = flow.externalCancel || (error && error.silent);
        return quiet ? { canceled: true, silent: true } : { canceled: true };
      }
      if (flow.state) {
        call('POST', '/api/session/login/cancel', { state: flow.state }).catch(function () {});
      }
      throw error;
    } finally {
      if (popup && !popup.closed) { try { popup.close(); } catch (e) { /* 无害 */ } }
      if (currentFlow === flow) {
        closeWebOverlay(true);
        currentFlow = null;
        loginActive = false;
        loginProvider = '';
        emitLogin();
      }
    }
  }

  // ── 壳特有命令的网页端降级表 ──────────────────────────────
  var SHELL_UNAVAILABLE = '该操作在网页端不可用（仅桌面端支持）';

  var shellCommands = {
    // 与老 edition/mode/provider 顺序的方法并存，直接 invoke 也收第五 options。
    start_login: function (args) {
      args = args || {};
      return window.workbuddyDesktop.startLogin(args.edition, args.mode, args.provider,
        args.socialRestore, args.options);
    },
    login_state: function () { return window.workbuddyDesktop.getLoginState(); },
    cancel_login: function () { return window.workbuddyDesktop.cancelLogin(); },
    api_request: function (args) {
      var request = (args && args.request) || {};
      return httpCall(request.method || 'GET', request.path || '/', request.body, false,
        request.apiKey === undefined ? undefined : String(request.apiKey));
    },
    check_update: function () {
      // 网页端没有「壳打包的版本号」这个概念：不带 current，服务端按自己的
      // 编译期版本比较（见 api/update 的 check 分支）。返回里带 selfUpdate /
      // serverAsset —— 面板据此决定显示「一键更新」还是手动升级命令。
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
    export_account_files: function (args) {
      return exportAccountFilesViaDownload(args && args.selection);
    },
    export_logs: function (args) {
      return downloadFile('GET', '/api/logs/download' + toQuery(args && args.query), 'agent2api-logs.jsonl');
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

  async function exportAccountFilesViaDownload(selection) {
    var data = await call('POST', '/api/accounts/export-files', selection || {});
    if (!data || !Array.isArray(data.files)) throw new Error('账号导出响应格式无效');
    var failed = Array.isArray(data.failed) ? data.failed.slice() : [];
    var count = 0;
    for (var index = 0; index < data.files.length; index++) {
      var item = data.files[index];
      try {
        var blob = new Blob([JSON.stringify(item.content, null, 2)], { type: 'application/json' });
        saveBlob(blob, item.fileName);
        count++;
      } catch (error) {
        failed.push({ id: item.id, name: item.name, error: asError(error).message });
      }
      if (index + 1 < data.files.length) {
        await new Promise(function (resolve) { setTimeout(resolve, 150); });
      }
    }
    return { total: data.total, count: count, failed: failed, downloadStarted: count > 0 };
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
    // 与 httpCall 同一取向：到期前先续期，别让这条非 JSON 请求去撞 401
    if (!retried) await ensureFreshSession();
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
   * 整库备份（网页端）：POST 密码参数，下载 .a2ab 加密包。
   *
   * 返回形状与桌面壳 `commands.rs::export_database` 对齐
   * （`{ bytes, tables, file }`），设置页的成功提示两端共用一套渲染。
   * 表计数在后端放进 `X-Backup-Tables` 响应头，响应体只承载加密包。
   */
  async function exportDatabase(includeLogs, password) {
    var response = await fetchAuthed('/api/backup/download', function () {
      return {
        method: 'POST',
        cache: 'no-store',
        headers: authHeaders({ 'Accept': '*/*', 'Content-Type': 'application/json' }),
        body: JSON.stringify({ includeLogs: includeLogs !== false, password: password || '' }),
      };
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
    var includedKeys = [];
    try { includedKeys = JSON.parse(response.headers.get('x-backup-keys') || '[]'); } catch (e) { includedKeys = []; }
    var filename = fileNameFromDisposition(response.headers.get('content-disposition'))
      || ('agent2api-backup-' + downloadStamp() + '.a2ab');
    var blob = await response.blob();
    saveBlob(blob, filename);
    return { bytes: blob.size, tables: tables, file: filename, saved: true, encrypted: true, includedKeys: includedKeys };
  }

  /**
   * 整库恢复：选择 .a2ab 或旧 .sql，密码和 File 通过 multipart 流式上传。
   * 不能 file.text()，否则大备份会在界面额外生成全量字符串。
   * 返回形状与桌面壳 `commands.rs::import_database` 一致
   * （`{ tables, safetyBackup, warning? }`）。
   */
  async function importDatabase(password) {
    var picked = await pickFile('.a2ab,.sql');
    if (picked.canceled) return { canceled: true };
    var file = picked.file;
    var form = new FormData();
    form.append('password', password || '');
    form.append('backup', file);
    var response = await fetchAuthed('/api/backup/upload', function () {
      return {
        method: 'POST',
        cache: 'no-store',
        headers: authHeaders({ 'Accept': 'application/json' }),
        body: form,
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
    if (!Array.isArray(accounts) && document_ && typeof document_.id === 'string'
      && document_.id.trim() && !Object.prototype.hasOwnProperty.call(document_, 'accounts')) {
      accounts = [document_];
    }
    if (!Array.isArray(accounts)) {
      throw new Error('文件里没有 accounts 数组，也不是单账号导出文件');
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
    // 平台标识：界面据此裁剪本机功能（导入桌面端登录态等在 web 下隐藏）
    platform: 'web',

    // ── 会话 ──
    getState: function () { return call('GET', '/api/session'); },
    startLogin: function (edition, mode, provider, socialRestore, options) {
      var target = String(provider || 'workbuddy').trim();
      var dynamic = hasMethodIndex(options);
      if (target === 'monkeycode' || target === 'monkeycode-intl') {
        return Promise.reject(new Error('MonkeyCode 自动网页登录仅限桌面端；浏览器面板请登录对应官网后粘贴 session。'));
      }
      // 只有旧小浣熊入口才改 redirect/预检；动态方法不能被 provider 特判覆盖。
      if (!dynamic && target === 'raccoon') {
        var problem = raccoonCallbackOriginProblem();
        if (problem) return reportRaccoonCallbackProblem(problem);
      }
      return runLoginFlow(target, async function (flow) {
        if (dynamic) flow.method = await selectedLoginMethod(target, options);
        if (flow.canceled) throw canceledError();
        var body = { edition: edition || 'cn', provider: target };
        // 不压缩第四参 socialRestore；第五参对象保持原字段/原值（含 proxy:null）。
        if (options !== undefined) body.options = options;
        if (dynamic) {
          body.mode = mode || 'embedded';
          body.socialRestore = socialRestore === true;
          // 兼容当前 API 的平铺读取；options 字段也完整保留给新端点。
          body.methodIndex = options.methodIndex;
          if (Object.prototype.hasOwnProperty.call(options, 'inputs')) body.inputs = options.inputs;
          if (Object.prototype.hasOwnProperty.call(options, 'proxy')) body.proxy = options.proxy;
        } else if (options && Object.prototype.hasOwnProperty.call(options, 'proxy')) {
          // 旧入口（内置表单的网页登录）也能带登录代理：网关的非动态分支读的
          // 就是顶层 `proxy`（见 api::session 的 legacy_proxy）。少了这一句，
          // 无头服务器上选了代理也仍然直连。
          body.proxy = options.proxy;
        }
        return call('POST', '/api/session/login/start', body);
      }, { dynamic: dynamic, mode: mode || 'embedded', edition: edition || 'cn' });
    },
    submitLoginCallback: function (state, callbackUrl) {
      return call('POST', '/api/session/login/callback', {
        state: String(state == null ? '' : state),
        callbackUrl: String(callbackUrl == null ? '' : callbackUrl),
      });
    },
    getLoginState: function () {
      return Promise.resolve(loginSnapshot());
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
      return runLoginFlow('autoclaw-intl', function () { return Promise.resolve({ state: state, authUrl: authUrl }); },
        { mode: mode || 'embedded', edition: 'intl' })
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
    zcodeClaimVerification: function (payload) {
      return call('POST', '/api/zcode/claim-verification', payload);
    },
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
    onLoginProgress: function (callback) {
      loginProgressListeners.add(callback);
      return function () { loginProgressListeners.delete(callback); };
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
    getClientIdentification: function () { return call('GET', '/api/client-identification'); },
    saveClientIdentification: function (payload) { return call('PUT', '/api/client-identification', payload); },
    previewClientIdentification: function (payload) { return call('POST', '/api/client-identification/preview', payload); },
    getCompactRoutes: function () { return call('GET', '/api/models/compact-routes'); },
    saveCompactRoutes: function (payload) { return call('PUT', '/api/models/compact-routes', payload); },
    setModelState: function (payload) { return call('POST', '/api/models/state', payload); },
    // 第 4 / 第 5 个参数（思考等级 / 映射开关）都按「有没有传」决定是否进请求体：
    // 后端按「请求体里有没有这个键」区分三态，undefined 的键不会进 JSON
    addModelMapping: function (alias, target, provider, reasoning, enabled, create) {
      var payload = { alias: alias, target: target, provider: provider };
      if (reasoning !== undefined) payload.reasoning = reasoning;
      if (enabled !== undefined) payload.enabled = enabled;
      if (create !== undefined) payload.create = create;
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
    testModel: function (payload, selectedKey) { return httpCall('POST', '/api/models/test', payload || {}, false, String(selectedKey || '')); },
    getModelApiKeys: function () { return call('GET', '/api/model-access/keys'); },

    // ── 网关 API Key（多把）──
    getKeys: function () { return call('GET', '/api/keys'); },
    createKey: function (payload) { return call('POST', '/api/keys', payload || {}); },
    updateKey: function (id, patch) { return call('PATCH', '/api/keys/' + encodeURIComponent(id), patch); },
    deleteKey: function (id) { return call('DELETE', '/api/keys/' + encodeURIComponent(id)); },

    // ── 多账号 ──
    getAccounts: function () { return call('GET', '/api/accounts'); },
    getAccountSorting: function () { return call('GET', '/api/accounts/sorting'); },
    saveAccountSorting: function (payload) { return call('PUT', '/api/accounts/sorting', payload || {}); },
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
      if (Object.prototype.hasOwnProperty.call(payload, 'usageQuery')) body.usageQuery = payload.usageQuery;
      if (Object.prototype.hasOwnProperty.call(payload, 'limiterGroups')) body.limiterGroups = payload.limiterGroups;
      if (Object.prototype.hasOwnProperty.call(payload, 'limiters')) body.limiters = payload.limiters;
      if (Object.prototype.hasOwnProperty.call(payload, 'limiterMode')) body.limiterMode = payload.limiterMode;
      return call('POST', '/api/accounts/batch', body);
    },

    // ── 账号路由（与 bridge.rs 的同名方法成对，缺一边会报「桥接方法缺失」）──
    getRoutingConfig: function () { return call('GET', '/api/routing'); },
    getRoutingWorkspace: function (model) { return call('GET', '/api/routing/workspace' + toQuery({ model: model })); },
    saveRoutingProfile: function (payload) { return call('PUT', '/api/routing/profile', payload || {}); },
    getRoutingGroups: function () { return call('GET', '/api/routing/groups'); },
    saveRoutingGroup: function (payload) { return call('PUT', '/api/routing/groups', payload); },
    previewRouting: function (payload) { return call('POST', '/api/routing/preview', payload); },
    getRoutingSticky: function (model) { return call('GET', '/api/routing/sticky' + toQuery({ model: model })); },
    clearRoutingSticky: function (payload) { return call('POST', '/api/routing/sticky/clear', payload || {}); },
    getRoutingReplays: function (query) { return call('GET', '/api/routing/replays' + toQuery(query)); },
    getRoutingFlow: function (query) { return call('GET', '/api/routing/flow' + toQuery(query)); },
    getRoutingTrace: function (query) { return call('GET', '/api/routing/trace' + toQuery(query)); },
    getRoutingTraceRequest: function (query) { return call('GET', '/api/routing/trace/request' + toQuery(query)); },
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
    setProxyPoolItemEnabled: function (payload) { return call('POST', '/api/proxies/pool/enable', payload || {}); },
    removeProxyPoolItem: function (id) { return call('POST', '/api/proxies/pool/remove', { id: String(id || '') }); },
    testProxyPoolItem: function (id) { return call('POST', '/api/proxies/pool/test', { id: String(id || '') }); },
    syncClashToProxyPool: function () { return call('POST', '/api/proxies/pool/sync-clash', {}); },
    // 代理设置（「网络代理」页的设置弹窗；与桌面壳 bridge.rs 的同名方法
    // 成对存在，契约见 api::proxies 的模块头 settings 两条）
    getProxySettings: function () { return call('GET', '/api/proxies/settings'); },
    saveProxySettings: function (payload) { return call('POST', '/api/proxies/settings', payload || {}); },
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

    // ── 手机验证码登录（AutoClaw 国内版 / Loomy / Tabbit 国内版）──
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
        : provider === 'tabbit'
          ? '/api/session/login/tabbit/sms/send'
          : '/api/session/login/sms/send';
      return call('POST', path, {
        phone: phone,
        provider: provider || undefined,
        // 与桌面桥同名同义：声明「认识发码前的滑块验证状态」，网关据此回
        // 200 + `{status:"captcha"}` 而不是 409；uuid 用于过完验证重发时
        // 沿用同一轮登录（见 api::session::login_tabbit_sms_send 的说明）。
        supportsCaptcha: isObject && input.supportsCaptcha ? true : undefined,
        uuid: isObject && input.uuid ? String(input.uuid) : undefined,
      });
    },
    // 打开上游给的人机验证页。网页面板没有壳命令可用，直接开新标签：
    // 这一下由用户点按钮触发，不会被弹窗拦截。不用 `noopener` 参数 ——
    // 它会让 window.open 返回 null，看起来像「被拦截」；改成打开后清 opener。
    openVerifyPage: function (url) {
      var opened = window.open(String(url || ''), '_blank');
      // 被拦截时 reject 而不是返回 false：界面据此把地址显示出来让人手动访问，
      // 而不是以为验证页已经开了、干等一个不会出现的窗口。
      if (!opened) return Promise.reject(new Error('浏览器拦截了弹窗'));
      try { opened.opener = null; } catch (error) { /* 跨域时改不了，忽略 */ }
      return Promise.resolve(true);
    },
    verifySmsLogin: function (payload) {
      payload = payload || {};
      var provider = payload.provider ? String(payload.provider) : '';
      var path = provider === 'loomy'
        ? '/api/session/login/loomy/sms/verify'
        : provider === 'tabbit'
          ? '/api/session/login/tabbit/sms/verify'
          : '/api/session/login/sms/verify';
      var body = {
        phone: String(payload.phone || ''),
        code: String(payload.code || ''),
      };
      // deviceId（AutoClaw）/ msgid（Loomy）/ uuid（Tabbit）按「有值才带」整形。
      // 字段名由界面按各自链路给（见 ui/sms-login.js 的 SMS_PROFILES），
      // 发码返回的中间态必须原样回传，不能丢掉，也不替它做归一。
      if (payload.deviceId) body.deviceId = String(payload.deviceId);
      if (payload.msgid) body.msgid = String(payload.msgid);
      if (payload.uuid) body.uuid = String(payload.uuid);
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
    saveCheckinKeepalive: function (models, apiKeyId) { return call('POST', '/api/checkin-keepalive', { models: models, apiKeyId: apiKeyId }); },
    // 猫猫旅行定时排程（国内版；与 bridge.rs 的同名方法成对维护）
    getTravelSchedule: function () { return call('GET', '/api/travel-schedule'); },
    saveTravelSchedule: function (patch) { return call('POST', '/api/travel-schedule', patch); },
    runTravelScheduleNow: function () { return call('POST', '/api/travel-schedule/run', {}); },
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
    getGithubAccess: function () { return call('GET', '/api/update/github'); },
    setGithubAccess: function (payload) { return call('POST', '/api/update/github', payload); },
    // GitHub 令牌：读只报 {filled, origin}，写传 {token: '…' | null}
    getUpdateToken: function () { return call('GET', '/api/update/token'); },
    setUpdateToken: function (payload) { return call('POST', '/api/update/token', payload); },
    // 「加入 beta 版」：读只报 {enabled}，写传 {enabled: true|false}（与桌面 bridge
    // 逐字同形 —— 两侧接口名与签名必须一致，界面代码零改动地跑在两边）
    getUpdateBeta: function () { return call('GET', '/api/update/beta'); },
    setUpdateBeta: function (payload) { return call('POST', '/api/update/beta', payload); },
    getUpdateChannels: function () { return call('GET', '/api/update/channels'); },
    setUpdateChannels: function (payload) { return call('POST', '/api/update/channels', payload); },
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
    installUpdate: function () { return call('POST', '/api/update/install', {}); },
    startSystemdUpdate: function (payload) { return call('POST', '/api/update/systemd', payload); },
    startWindowsServiceUpdate: function (payload) { return call('POST', '/api/update/windows-service', payload); },
    restartService: function () { return call('POST', '/api/update/restart', {}); },

    // ── 出站指纹脱敏 ──
    getSanitize: function () { return call('GET', '/api/sanitize'); },
    saveSanitize: function (patch) {
      return call('PUT', '/api/sanitize', typeof patch === 'boolean'
        ? { sanitizeBlacklistFingerprints: patch } : patch);
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

    // ── ZCode 客户端版本号 ──
    // 与桌面桥（src/bridge.rs 的 getZcodeVersion / saveZcodeVersion）同一映射。
    getZcodeVersion: function () { return call('GET', '/api/zcode/version'); },
    saveZcodeVersion: function (version) {
      return call('PUT', '/api/zcode/version', { version: version == null ? '' : String(version) });
    },

    // ── 网关面跨域访问（/v1/*，默认关）──
    getCors: function () { return call('GET', '/api/cors'); },
    saveCors: function (enabled) {
      return call('PUT', '/api/cors', { corsEnabled: enabled === true });
    },

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
    // 插件页面清单（侧边栏动态菜单用）。与 desktop 壳的 bridge.rs 同名方法
    // **必须成对维护** —— 只改一边会让 headless 面板里插件页静默消失。
    getPlugins: function () { return call('GET', '/api/plugins'); },
    // 侧边栏布局（分组 / 顺序 / 显隐）。与 desktop 壳 bridge.rs 的同名方法
    // **必须成对维护** —— 漏了这一边，网页端就改不到库里的那份布局，
    // 「在 WebUI 里隐藏的菜单客户端也跟着隐藏」这条功能等于不存在。
    // 语义见 api::sidebar_layout：读回 `{layout}`（null = 没存过），写整份。
    getSidebarLayout: function () { return call('GET', '/api/sidebar-layout'); },
    saveSidebarLayout: function (layout) {
      return call('PUT', '/api/sidebar-layout', { layout: layout === undefined ? null : layout });
    },
    clearLogs: function (query) { return call('DELETE', '/api/logs' + toQuery(query)); },
    exportLogs: function (query) { return invokeShell('export_logs', { query: query }); },

    // ── 请求统计报表 / 数据保留 ──
    // dates（`{start, end}`，`YYYY-MM-DD`）只在 range=custom 时给，
    // 与桌面壳 bridge.rs 的同名方法同一口径（toQuery 负责跳过空值）
    getStatsSummary: function (range, dates, options) {
      return call('GET', '/api/stats/summary' + toQuery({
        range: range,
        startDate: dates && dates.start,
        endDate: dates && dates.end,
        apiKey: options && options.apiKey,
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
    // 每次取凭证前复用普通请求的提前续期；refresh=true 只用于明确的面板 401。
    // 只重读 localStorage 不会续期，拿回的仍是同一把失效令牌。
    directCredentials: async function (refresh) {
      if (refresh) {
        if (!(await tryRefresh())) {
          clearPanelAuth();
          window.location.href = '/login';
          return new Promise(function () {});
        }
      } else {
        await ensureFreshSession();
      }
      return authHeaders({});
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
    // ── 会话图片压缩（开关 + 长边 / 质量 / 阈值三项）──
    // 与桌面桥（src/bridge.rs 的 getImageCompress / saveImageCompress）逐字同形。
    getImageCompress: function () { return call('GET', '/api/image-compress'); },
    saveImageCompress: function (patch) { return call('PUT', '/api/image-compress', patch); },
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
    exportAccountFiles: function (selection) { return invokeShell('export_account_files', { selection: selection }); },
    importAccounts: function () { return invokeShell('import_accounts'); },

    // ── 整库备份 / 恢复 ──
    // 桌面壳走系统文件对话框（bridge.rs 的 exportDatabase / importDatabase），
    // 网页端只能「下载附件」与「上传文件」，对应后端新加的一对 HTTP 端点
    // （见 api::backup_api 的模块头）。返回形状与桌面壳逐字段一致，设置页
    // 的提示文案与结果区两端共用。
    exportDatabase: function (includeLogs, password) { return exportDatabase(includeLogs, password); },
    importDatabase: function (password) { return importDatabase(password); },
    // 恢复进度：网页端与桌面端同一条后端端点（见 api::backup_api）
    getBackupProgress: function () { return call('GET', '/api/backup/progress'); },
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
"#
}
