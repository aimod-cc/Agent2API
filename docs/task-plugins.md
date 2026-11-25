# 定时任务类插件制作指南

Agent2API 的 **定时任务类插件（Scheduled Task Plugin）** 是一种**内置安全沙箱的轻量脚本插件**。插件只需用声明式配置描述自己需要哪些表单输入项、操作按钮和状态卡片，Agent2API 就会在**桌面端、网页端和手机端**自动用原生组件库渲染出统一美观的操作界面，不再需要把账号密码写死在脚本代码里。

安装一次插件后，你可以在「定时任务」页面为同一个插件创建任意多个独立任务实例（例如「主号每日签到」「小号每日签到」），每个实例拥有独立的定时计划、网络代理、加密参数和持久化登录态。

---

## 1. 快速上手：可视化制作（推荐）

无需搭建开发环境或安装任何额外运行时，直接在 Agent2API 客户端即可制作：

1. 打开侧边栏 **「定时任务」**，点击右上角 **「编写脚本任务」**（或进入 **「任务插件 → 新建定时任务插件」**）。
2. 在弹窗左侧的 **「可视化界面」** 页签中：
   - 可先点击 **「套用：账密登录签到模板」** 或 **「套用：网页监控通知模板」** 快速起步；
   - 添加、排序、编辑表单输入项、自定义按钮（支持放在输入框旁或表单底栏）和状态展示项；
   - 在 **「信息与权限」** 页签填写插件名称、标识、默认 Cron 计划、允许访问的域名白名单，以及是否声明系统通知权限。
3. 右侧会**实时渲染真实组件预览**，你可以直接填测试数据、点自定义按钮或点 **「试跑定时入口」**，在内置沙箱里真实验证效果并查看日志。
4. 切换到 **「脚本代码」** 页签编写业务逻辑（`onAction` / `onSchedule`）；可视化界面与单文件代码支持**双向同步**。
5. 点击 **「保存插件」**，勾选「保存后同步创建默认定时任务」即可立刻投入使用。

---

## 2. 单文件插件格式规范

一个定时任务插件可以保存为单个 `.js` 文件（UTF-8 编码，上限 256 KB），由顶部的 `@agent2api-task-plugin` JSON 声明块与下方的 JavaScript 脚本正文组成：

```javascript
/* @agent2api-task-plugin
{
  "id": "demo-site-checkin",
  "name": "示例站点自动签到",
  "version": "1.0.0",
  "description": "支持账号密码或短信验证码登录，并每日自动签到领取积分",
  "defaultCron": "0 9 * * *",
  "defaultTimezone": "Asia/Shanghai",
  "defaultTimeout": 60,
  "permissions": {
    "domains": ["api.example.com"],
    "notify": true
  },
  "fields": [
    {
      "key": "loginMode",
      "label": "登录方式",
      "type": "segmented",
      "defaultValue": "password",
      "options": [
        { "value": "password", "label": "账号密码" },
        { "value": "sms", "label": "短信验证码" }
      ]
    },
    {
      "key": "username",
      "label": "账号 / 手机号",
      "type": "text",
      "required": true,
      "placeholder": "请输入账号或手机号"
    },
    {
      "key": "password",
      "label": "登录密码",
      "type": "password",
      "secret": true,
      "when": { "key": "loginMode", "equals": "password" }
    },
    {
      "key": "smsCode",
      "label": "短信验证码",
      "type": "text",
      "placeholder": "6 位数字验证码",
      "when": { "key": "loginMode", "equals": "sms" }
    },
    {
      "key": "notifyOnFailOnly",
      "label": "仅在失败时推送通知",
      "type": "switch",
      "group": "advanced",
      "defaultValue": false
    }
  ],
  "actions": [
    {
      "id": "send_sms",
      "label": "发送验证码",
      "variant": "outline",
      "placement": "inline",
      "attachToField": "smsCode",
      "cooldownSeconds": 60
    },
    {
      "id": "login",
      "label": "测试登录",
      "variant": "default",
      "placement": "footer"
    },
    {
      "id": "logout",
      "label": "退出登录",
      "variant": "destructive",
      "placement": "footer",
      "confirm": "确定清除当前任务的登录态缓存吗？"
    }
  ],
  "statusFields": [
    { "key": "account", "label": "登录状态" },
    { "key": "points", "label": "当前积分" }
  ]
}
*/

export async function onAction(action, ctx) {
  // 响应用户在界面点击的按钮
}

export async function onSchedule(ctx) {
  // 定时触发或手动点「运行」时执行
}
```

### 2.1 顶层元数据字段

| 字段 | 必填 | 类型 | 说明 |
| --- | --- | --- | --- |
| `id` | 是 | string | 插件唯一标识，仅允许小写字母开头，由 `a-z`、`0-9`、`-` 组成，最长 64 字符 |
| `name` | 是 | string | 展示给用户的插件名称 |
| `version` | 否 | string | 语义化版本号（如 `1.0.0`），默认 `1.0.0` |
| `description` | 否 | string | 插件简介 |
| `defaultCron` | 否 | string | 新建任务时的默认 5 段 Cron 表达式（分 时 日 月 周），默认 `0 9 * * *` |
| `defaultTimezone` | 否 | string | 默认 IANA 时区（如 `Asia/Shanghai`、`UTC`），默认 `Asia/Shanghai` |
| `defaultTimeout` | 否 | number | 单次执行超时上限（秒，范围 `1`–`300`），默认 `60` |
| `permissions` | 否 | object | 权限声明，详见 2.2 |
| `fields` | 否 | array | 表单输入项声明（最多 64 项），详见 2.3 |
| `actions` | 否 | array | 自定义操作按钮声明（最多 16 项），详见 2.4 |
| `statusFields` | 否 | array | 只读状态摘要指标声明（最多 16 项），详见 2.5 |

### 2.2 权限声明 (`permissions`)

```json
"permissions": {
  "domains": ["api.example.com", "*.example.org"],
  "notify": true
}
```

- `domains`（数组，最多 32 项）：允许脚本访问的域名白名单。
  - 支持精确域名（如 `api.example.com`）、通配子域（如 `*.example.com`），以及在「站点地址由用户在表单自由填写」的通用插件里声明 `"*"`（匹配任意公网域名）。
  - 留空表示「仅本地纯计算、不联网」，脚本发起的任何网络请求都会被沙箱拒绝。
  - 即使声明了域名或 `"*"`，沙箱也**始终禁止**访问内网/环回/链路本地 IP（`127.0.0.0/8`、`10.0.0.0/8`、`172.16.0.0/12`、`192.168.0.0/16`、`169.254.0.0/16`、`::1` 等），且遇到 HTTP 3xx 重定向时每一跳都会重新校验白名单与公网地址。
- `notify`（布尔值，默认 `false`）：声明插件是否需要调用系统通知渠道（用户在「设置 → 通知」中配置的 Webhook / Telegram / Bark）。
  - 插件声明 `"notify": true` 后，安装时系统会为该插件创建一个**显式的通知权限开关（默认开启）**；用户可随时在插件设置或任务配置中关闭，关闭后 `ctx.notify.send()` 会被静默拦截。

### 2.3 表单输入项 (`fields`)

支持 **8 种原生控件类型**：

| `type` | 展示形态 | 值类型 | 适用场景 |
| --- | --- | --- | --- |
| `text` | 单行文本框 | string | 账号、手机号、URL、验证码等 |
| `password` | 密码输入框（带显隐按钮） | string | 密码、Cookie、API Token（自动加密保存） |
| `textarea` | 多行文本框 | string | 多行 Cookie、备注、JSON 片段 |
| `number` | 数字输入框（支持 `min`/`max`/`unit`） | number | 重试次数、阈值、延迟毫秒数 |
| `switch` | 开关 | boolean | 是否启用某项功能 |
| `select` | 下拉单选框 | string | 从多个选项中单选一项 |
| `segmented` | 分段切换器 | string | 2–4 个互斥模式切换（如登录方式） |
| `multiselect` | 多选下拉 / 芯片组 | string[] | 勾选多个签到项目或频道 |

字段通用属性：

| 属性 | 说明 |
| --- | --- |
| `key` | 字段标识，脚本里通过 `ctx.form[key]` 读取 |
| `label` | 界面展示名称 |
| `type` | 上述 8 种类型之一 |
| `placeholder` | 输入框占位提示语 |
| `description` | 字段下方的简要辅助说明 |
| `required` | 是否必填（布尔值） |
| `secret` | 是否加密保存（`type: "password"` 默认为 `true`；`text`/`textarea` 也可手动设为 `true`）。加密字段使用 AES-256-GCM 加密入库，前端界面永远只显示「已加密保存」而不会回传明文 |
| `group` | `"basic"`（默认）或 `"advanced"`（归入折叠的「高级选项」面板） |
| `defaultValue` | 默认值 |
| `min` / `max` / `unit` | 仅 `number` 类型使用，如 `min: 1, max: 100, unit: "秒"` |
| `options` | `select` / `segmented` / `multiselect` 的选项数组：`[{ "value": "a", "label": "选项 A" }]` |
| `when` | 条件显隐：`{ "key": "loginMode", "equals": "sms" }`，仅当目标字段值等于 `equals` 时显示 |

### 2.4 自定义操作按钮 (`actions`)

| 属性 | 说明 |
| --- | --- |
| `id` | 动作标识，用户点击时传入 `onAction(action, ctx)` 的 `action` 参数 |
| `label` | 按钮文字 |
| `variant` | 按钮样式：`"default"`（主按钮）、`"outline"`（普通按钮，默认）、`"destructive"`（警示红按钮） |
| `placement` | 摆放位置：`"footer"`（表单底栏，默认）或 `"inline"`（紧贴某个输入框右侧，如「发送验证码」） |
| `attachToField` | 当 `placement: "inline"` 时，指定附着到哪个字段的 `key` 右侧 |
| `confirm` | 可选的二次确认提示语；填写后点击按钮会先弹出确认框 |
| `cooldownSeconds` | 可选的冷却倒计时秒数（如 `60`），点击后自动进入倒计时防连点 |

### 2.5 状态摘要指标 (`statusFields`)

```json
"statusFields": [
  { "key": "account", "label": "登录状态" },
  { "key": "points", "label": "当前积分" }
]
```

当脚本在返回值里给出 `status: { account: "已登录 (Alice)", points: "1,280" }` 时，Agent2API 会自动把最新状态持久化并渲染在**任务列表卡片**与**任务详情顶部**。

---

## 3. 脚本生命周期与内置 API

### 3.1 两个入口函数

脚本正文支持写 `export async function ...` 或普通 `async function ...`：

1. **`onAction(action, ctx)`**：用户在任务配置界面点击任意自定义按钮时触发，`action` 为该按钮的 `id`。
2. **`onSchedule(ctx)`**：到达 Cron 计划时间、或用户在任务列表点击「运行」时触发。

> **兼容纯脚本模式**：如果你只是迁移一段没有写 `onSchedule` 的顶层异步脚本（例如来自 OmniProxy 的旧定时脚本），脚本可以直接写顶层代码并调用全局 `fetch()`、`sleep()`、`console.log()`，无需改造也能在定时触发时正常运行。

### 3.2 返回值协议

`onAction` 和 `onSchedule` 均可返回一个结果对象（所有字段均可选）：

```javascript
return {
  message: '登录成功，已缓存会话令牌',   // 展示给用户的提示文案（最多 1000 字符）
  level: 'success',                     // 'success' | 'info' | 'warning' | 'error'
  status: {                             // 局部更新任务状态摘要卡片
    account: '已登录',
    points: '1,280',
  },
  qrText: 'https://example.com/qr?id=xxx', // 自动在后端生成可扫描的 SVG 二维码展示在界面
  imageDataUrl: 'data:image/png;base64,...', // 展示验证码图片等（支持 png/jpeg/webp/gif，上限 2 MB）
  patchForm: {                          // 可选：回填表单字段（例如自动填回获取到的设备号）
    deviceId: 'dev-12345',
  },
}
```

如果脚本执行出错，直接 `throw new Error('失败原因')`，Agent2API 会记录失败状态与日志并在界面清晰提示。

### 3.3 `ctx` 上下文对象

| 接口 | 说明 |
| --- | --- |
| `ctx.form` | 只读对象，包含当前任务保存的全部表单值（加密字段已在后端安全解密注入；在点交互按钮时还会合并用户刚刚在表单里输入但尚未点「保存」的临时值） |
| `ctx.task` | 只读元数据：`{ id, name, pluginId, trigger, action, triggerTime }` |
| `ctx.http` | 沙箱 HTTP 客户端（自动走该任务配置的代理，并受域名白名单约束）：<br>• `await ctx.http.request(url, { method, headers, body })`<br>• `await ctx.http.get(url, init)` / `post(url, body, init)` / `put` / `patch` / `delete`<br>• `await ctx.http.getText(url, init)`<br>• `await ctx.http.getJson(url, init)`<br>• `await ctx.http.postJson(url, body, init)`<br>响应对象包含 `{ ok, status, statusText, url, headers, setCookies, body, text(), json() }`（其中 `setCookies` 为逐条原始 `Set-Cookie` 响应头数组，方便在多条 Cookie 响应中准确提取会话 Cookie），单次响应体上限 2 MB |
| `ctx.storage` | **按任务隔离的加密持久化键值存储**（适合保存 Cookie、Token、上次检查的哈希等）：<br>• `await ctx.storage.get(key)` → 返回字符串或 `null`<br>• `await ctx.storage.set(key, value)`（对象会自动序列化为 JSON）<br>• `await ctx.storage.delete(key)`<br>• `await ctx.storage.keys()` → 返回所有键名数组<br>注：单任务最多 256 个键、总容量上限 256 KB，落盘前自动 AES-256-GCM 加密 |
| `ctx.notify.send(title, body, data?)` | 调用系统已启用的通知渠道发送通知。仅当插件声明了 `permissions.notify: true` 且用户未关闭该插件的通知权限时实际发送；在编辑器试跑模式下不会真实外发 |
| `ctx.crypto` | 常用摘要与编解码工具：<br>• `ctx.crypto.md5(str)` / `sha1(str)` / `sha256(str)`<br>• `ctx.crypto.hmacSha256(key, msg)`<br>• `ctx.crypto.base64Encode(str)` / `base64Decode(str)`<br>• `ctx.crypto.randomHex(bytes)` |
| `await ctx.sleep(ms)` | 可中断延时等待（单次不超过任务剩余超时时间，用户点「停止」会立刻唤醒终止） |
| `ctx.log(...args)` / `console.log(...)` | 写入本次执行日志，可在任务的「运行记录」中查看（单次日志上限 64 KB） |

---

## 4. 安全与资源边界

所有定时任务插件都运行在内置的纯沙箱 JavaScript 引擎（QuickJS-NG）隔离线程中：

- **无文件系统、无子进程、无环境变量访问**：脚本看不到主机文件，也无法读取网关账号表、网关 Key 或其他定时任务的数据。
- **内存与栈保护**：单次执行内存上限 **32 MB**、调用栈上限 **512 KB**。
- **硬超时与即时停止**：超过任务配置的超时秒数（或用户点击「停止」）时，引擎会在字节码中断点与网络/等待点**立即强行中止**，即使脚本里写了 `while (true) {}` 死循环也不会卡死服务。
- **加密隔离**：每个任务的敏感表单字段和 `ctx.storage` 登录态都使用独立密钥作用域（`task-plugins.key`）加密存储，且会随 Agent2API 整库备份/恢复流程一起安全重加密迁移。
- **脱敏导出分享**：在插件页面点击 **「导出分享」** 生成的单文件 `.js` 只包含界面声明与代码，**绝不会携带任何已填密码、表单参数或登录 Cookie**。

---

## 5. 实用示例

### 示例 A：网页内容变更监控 + 系统通知

```javascript
/* @agent2api-task-plugin
{
  "id": "webpage-change-watch",
  "name": "网页变更监控",
  "version": "1.0.0",
  "description": "定时抓取指定页面摘要，检测到内容变化时发送系统通知",
  "defaultCron": "*/30 * * * *",
  "defaultTimezone": "Asia/Shanghai",
  "defaultTimeout": 30,
  "permissions": {
    "domains": ["status.example.com"],
    "notify": true
  },
  "fields": [
    {
      "key": "targetUrl",
      "label": "监控地址",
      "type": "text",
      "required": true,
      "placeholder": "https://status.example.com/api/summary"
    },
    {
      "key": "keyword",
      "label": "关注关键词（可选）",
      "type": "text",
      "placeholder": "留空则监控整个响应内容变化"
    }
  ],
  "actions": [
    { "id": "check_now", "label": "立即检测一次", "variant": "default", "placement": "footer" },
    { "id": "reset_hash", "label": "重置基线", "variant": "outline", "placement": "footer" }
  ],
  "statusFields": [
    { "key": "state", "label": "监控状态" },
    { "key": "checkedAt", "label": "最近检测" }
  ]
}
*/

async function checkOnce(ctx, notifyOnChange) {
  const url = String(ctx.form.targetUrl || '').trim();
  if (!url) throw new Error('请先填写监控地址');

  const res = await ctx.http.get(url);
  if (!res.ok) throw new Error(`请求失败（HTTP ${res.status}）`);

  const kw = String(ctx.form.keyword || '').trim();
  const digestSource = kw ? String(res.body.includes(kw)) : res.body;
  const currentHash = ctx.crypto.sha256(digestSource);
  const prevHash = await ctx.storage.get('last_hash');
  const nowLabel = new Date().toLocaleTimeString();

  await ctx.storage.set('last_hash', currentHash);

  if (prevHash && prevHash !== currentHash) {
    ctx.log('检测到内容变化:', prevHash.slice(0, 8), '->', currentHash.slice(0, 8));
    if (notifyOnChange) {
      await ctx.notify.send('网页监控提醒', `监控目标内容已发生变化：${url}`);
    }
    return {
      level: 'warning',
      message: '检测到目标内容已发生变化！',
      status: { state: '已变化', checkedAt: nowLabel },
    };
  }

  return {
    level: 'success',
    message: prevHash ? '内容无变化' : '已记录首次检测基线',
    status: { state: '正常', checkedAt: nowLabel },
  };
}

export async function onAction(action, ctx) {
  if (action === 'reset_hash') {
    await ctx.storage.delete('last_hash');
    return { level: 'info', message: '已重置检测基线', status: { state: '待检测' } };
  }
  return await checkOnce(ctx, false);
}

export async function onSchedule(ctx) {
  return await checkOnce(ctx, true);
}
```

### 示例 B：扫码登录 + 定时保活

当目标站点需要手机扫码登录时，只需在 `onAction` 里返回 `qrText`，界面会自动渲染可扫描二维码：

```javascript
export async function onAction(action, ctx) {
  if (action === 'get_qr') {
    const data = await ctx.http.postJson('https://api.example.com/auth/qr/create', {});
    await ctx.storage.set('qr_ticket', data.ticket);
    return {
      level: 'info',
      message: '请使用手机客户端扫描下方二维码，完成后点击「确认扫码状态」',
      qrText: data.qrUrl,
      status: { account: '等待扫码' },
    };
  }
  if (action === 'confirm_qr') {
    const ticket = await ctx.storage.get('qr_ticket');
    if (!ticket) throw new Error('请先点击获取二维码');
    const data = await ctx.http.getJson(`https://api.example.com/auth/qr/poll?ticket=${encodeURIComponent(ticket)}`);
    if (!data.token) {
      return { level: 'warning', message: '尚未完成扫码确认，请扫码后重试' };
    }
    await ctx.storage.set('session_token', data.token);
    return {
      level: 'success',
      message: `登录成功：${data.nickname}`,
      status: { account: `已登录 (${data.nickname})` },
    };
  }
}
```

---

## 6. 分享、导入与上架官方商店（ZIP 打包规范）

### 6.1 个人分享 / 社区分发

- **单文件分享**：在「定时任务 → 任务插件」（或「插件 → 定时任务」）卡片上点击 **「权限与设置 → 导出分享文件」**，拿到干净的 `.js` 文件，发给其他人直接点击 **「导入脚本/包」** 即可导入。
- **ZIP 包导入**：支持在「定时任务 → 任务插件 → 导入脚本/包」中导入，也支持直接拖入「插件 → 更多操作 → 从文件安装（批量安装插件包）」自动识别安装。ZIP 包（上限 2 MB）包含以下文件：
  ```text
  my-plugin-1.0.0-task-script.zip
  ├── script.js       # 必填：包含 @agent2api-task-plugin 头部的完整插件脚本（也可命名为 plugin.js）
  ├── icon.png        # 可选：自定义图标（支持 icon.png / icon.webp / icon.svg，上限 256 KB）
  └── pack.json       # 可选：包清单（上架官方商店时必填，含签名信息）
  ```

### 6.2 上架官方商店与管理后台配置

官方商店对定时任务插件复用现有的安全分发与数字签名链路：

1. **包文件命名**：`{id}-{version}-task-script.zip`（与提供商插件的 `wasm32-unknown-unknown` 区分开，目标名为 `task-script`）。
2. **`pack.json` 签名格式**：
   ```json
   {
     "id": "demo-site-checkin",
     "version": "1.0.0",
     "category": "scheduled_task",
     "target": "task-script",
     "signature": {
       "algorithm": "ed25519",
       "key_id": "official-1",
       "value": "<base64(64字节 ed25519 签名)>"
     }
   }
   ```
   签名原文为 UTF-8 字符串：`task-script-v1\n{id}\n{version}\n{sha256_hex(script.js)}`。
3. **管理后台录入**：在管理后台「插件管理」中新增插件，将类别选为 **「定时任务」**（`scheduled_task`），配置发布仓库与标签模板后同步即可出现在官网插件目录与客户端商店中，并支持自动检查更新。

---

## 7. 附：让 AI 帮你写定时任务插件的提示词模板

你可以直接把下面这段提示词连同你的需求发给任意 AI 助手，生成的代码可直接粘贴进 Agent2API 的「导入脚本/包」：

```text
请帮我编写一个 Agent2API 定时任务类插件单文件脚本（.js）。
要求严格遵守以下格式：
1. 文件顶部必须是 /* @agent2api-task-plugin { ...合法 JSON... } */ 声明块，包含 id、name、version、description、defaultCron、defaultTimezone、defaultTimeout、permissions（domains 白名单与 notify 布尔值）、fields、actions、statusFields。
2. fields 的 type 只能从 text / password / textarea / number / switch / select / segmented / multiselect 中选择；密码/Token 字段必须设置 "secret": true；需要联动的字段用 when: { key, equals }。
3. 脚本中导出 async function onAction(action, ctx) 处理按钮点击，导出 async function onSchedule(ctx) 处理定时执行。
4. 只使用沙箱内置 API：ctx.form、ctx.http（get/post/getJson/postJson/request）、ctx.storage（get/set/delete/keys）、ctx.notify.send(title, body)、ctx.crypto、ctx.sleep(ms)、ctx.log(...)，不得引入 Node.js 模块或浏览器 DOM API。
5. 返回对象支持 { message, level, status, qrText, imageDataUrl, patchForm }。

我的具体需求是：
【在这里描述你要自动化的网站、登录方式、需要定时做的事情和通知条件】
```
