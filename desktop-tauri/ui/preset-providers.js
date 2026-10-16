/* Agent2API · 自定义提供商的预置目录（preset） */

/**
 * 「添加账号 → 自定义」分段下的预置提供商清单：名称 / 图标 / 默认协议 / Base URL
 * 抄自 9Router（Acankao/9router）的 open-sse/providers/registry 注册表 —— 那边每家
 * 存的是完整端点 URL（如 https://api.openai.com/v1/chat/completions），这里按本网关
 * 自定义提供商的 Base URL 语义折算（chat_completions 拼 {baseUrl}/chat/completions，
 * anthropic 拼 {baseUrl}/v1/messages）。
 *
 * 预置只是**新建自定义提供商表单的预填模板**：选中某一家时把名称 / 协议 / Base URL
 * 填进表单（都可改），提交仍走 POST /api/custom-providers，落库后就是一条普通的
 * 自定义提供商（custom- 前缀），与手动新建的没有任何区别。因此这里不需要知道
 * 「哪家已建过」—— 那是账号页 / 弹窗第 1 步按 customList 现算的事。
 *
 * ── 默认协议怎么取 ─────────────────────────────────────────
 * 按该家在 9Router 注册表里的主力端点格式：主打 Claude Code 的 coding 套餐
 * （GLM Coding、Minimax 两家）官方主推 Anthropic 端点，默认 anthropic；其余家
 * 默认 chat_completions。协议下拉在表单里随时可改，hint 里注明另一端的地址。
 *
 * Command Code 有意不预置：它的端点（/alpha/generate）是私有协议，三种协议都
 * 对不上，预置了也转发不通（见 CLAUDE 会话记录）。
 */
(() => {
  /** 协议值与后端 custom_providers::PROTOCOLS 逐字一致（勿改拼写） */
  const P = { openai: 'chat_completions', anthropic: 'anthropic' };

  /**
   * 预置清单。字段：
   *   key      卡片的稳定标记（data-provider="preset:<key>"），也是图标文件名
   *   name     展示名（预填进「名称」输入框，可改）。**不包 t()**：品牌名，且
   *            同时被 add-provider-pick 的「同名家是否已建」判重与 models-page
   *            的图标回match当字面量用，包了会跨语言失配（见任务报告）
   *   icon     assets/providers/ 下的图标文件；没有收录的用首字母徽章
   *   protocol 预填协议（下拉仍可改）
   *   baseUrl  预填基址（按本网关的拼接语义，见文件头）
   *   hint     表单 Base URL 栏下的备注：该家另一端协议的地址 / 站点差异说明
   *            （纯展示文案，走 wbI18n.t）
   *   quirks   该家的上游特判（随创建写进提供商记录，转发时生效，见下）
   *   account  该家账号的默认取值（表单里的初始勾选态，用户可改）：
   *            · noAuth —— 「该上游无需鉴权」预勾选（无需 API Key 的家里预勾上；
   *              用户一填 Key 就自动取消，见 add-custom-provider.tsx）
   *   placeholder  Base URL 输入框的占位提示（自建面板没有固定地址时给一句
   *            「填你自己的站点」；缺省按协议算的那句，见 add-custom-provider）
   *   balance  该家的**余额查询方案**（`{ scheme }`，取值与后端
   *            `custom_providers::BALANCE_SCHEMES` 逐字一致）：创建时随记录写入
   *            `balanceScheme`，后端据此选查询链路；界面按方案渲染要补的凭证
   *            输入框（方案与字段说明见 ui-islands 的 balance-query.tsx）。
   *            没有这个键的家 = 不查余额（手工新建的家也一样）。
   *
   * 六种方案分别对应：sub2api / new_api / deepseek / opencode（OpenCode Go）/
   * sensenova（商汤日日新）/ zhipu（智谱）。**方案不是用户选的**，是这张卡
   * 「这一家是谁」的派生事实 —— 所以它写在这里，不在表单里给下拉。
   *
   * ── quirks 的取值（抄自 9Router 各家适配器，后端 customProviders 记录的字段）──
   *   urlSuffix          原样追加到出站 URL 的查询串（GLM / MiniMax 的
   *                      Claude 兼容端点要求 ?beta=true，缺了部分 beta 特性不可用）
   *   headers            合并到默认头上的静态额外头（同名覆盖默认头）：
   *                      · Anthropic-Beta —— 启用 interleaved-thinking 等 beta
   *                        特性，Claude 兼容的 coding 端点按 9Router 同款下发；
   *                      · OpenRouter 的 HTTP-Referer / X-Title —— 它的推荐
   *                        标识头（缺了不影响转发，照抄保持同款行为）；
   *                      · Cline Pass 的一组 Cline 客户端伪装头 —— 它的上游按
   *                        客户端形态校验（`X-CLIENT-TYPE: cline-sdk` 是硬门槛，
   *                        缺了一律 403），取值抄自后端 Cline 适配器的
   *                        DEFAULT_HEADERS（providers/cline/headers.rs，与 Agent
   *                        内置家发出去的是同一套）。**不要抄别的网关的客户端值**
   *                        （如 9Router 的 9router 头）—— 上游认的是 Cline 客户端。
   *   anthropicToolType  "custom" = 发 anthropic 上游时给每个工具补 type
   *                      （MiniMax 的 Claude 兼容端点拒绝无 type 的工具；
   *                      注意 DeepSeek 恰好相反、拒绝 type:"custom"，而本网关
   *                      翻译器默认输出无 type 形态，两家各按各的特判走）。
   *   clientEmulation    "opencode" = 按 **OpenCode 官方 CLI 的形状**补齐出站请求
   *                      （无 Key 时用匿名凭证 `public`、补 `ses_…` 会话头、
   *                      请求体补 `bash` / `read` 两个桩工具 —— 上游免费档的
   *                      三道校验，2026-09 实测 403 FreeTierError 就是这个）。
   *                      **它会改写请求体**，所以只有 OpenCode Zen 默认开着，
   *                      别的家不要加。
   *
   * 其余在 9Router 注册表里出现的特判**有意不搬**（本网关的行为已天然覆盖）：
   * preserveCacheControl（chat 透传原样发请求体）、dropOutputConfig（翻译器
   * 不产出 output_config）、anthropicVersion（恒发 anthropic-version）、
   * reasoningInject / forceStream（思考等级注入与恒 stream 是本网关的既有机制）。
   */
  const ANTHROPIC_BETA_HEADERS = {
    'Anthropic-Beta': 'claude-code-20250219,interleaved-thinking-2025-05-14',
  };

  const PRESET_PROVIDERS = [
    {
      key: 'openai', name: 'OpenAI', icon: 'openai.png',
      protocol: P.openai, baseUrl: 'https://api.openai.com/v1',
    },
    {
      key: 'anthropic', name: 'Anthropic', icon: 'anthropic.png',
      protocol: P.anthropic, baseUrl: 'https://api.anthropic.com',
      quirks: { headers: { ...ANTHROPIC_BETA_HEADERS } },
    },
    {
      key: 'openrouter', name: 'OpenRouter', icon: 'openrouter.png',
      protocol: P.openai, baseUrl: 'https://openrouter.ai/api/v1',
      hint: wbI18n.t('一个 Key 用遍多家模型，模型名用「厂商/模型」全称（如 deepseek/deepseek-chat）'),
      quirks: {
        headers: { 'HTTP-Referer': 'https://endpoint-proxy.local', 'X-Title': 'Endpoint Proxy' },
      },
    },
    {
      key: 'huggingface', name: 'HuggingFace', icon: 'huggingface.png',
      protocol: P.openai, baseUrl: 'https://router.huggingface.co/v1',
      hint: wbI18n.t('走 Inference Providers 路由，Key 在 hf.co/settings/tokens 创建'),
    },
    {
      key: 'nvidia', name: 'NVIDIA', icon: 'nvidia.png',
      protocol: P.openai, baseUrl: 'https://integrate.api.nvidia.com/v1',
      hint: wbI18n.t('NVIDIA NIM 托管端点，Key 在 integrate.api.nvidia.com 申请'),
    },
    {
      key: 'deepseek', name: 'DeepSeek', icon: 'deepseek.png',
      protocol: P.openai, baseUrl: 'https://api.deepseek.com',
      hint: wbI18n.t('Anthropic 兼容端点在 https://api.deepseek.com/anthropic（协议换 anthropic 时填它）'),
      // 余额查询方案（后端 custom_providers::BALANCE_SCHEME_DEEPSEEK）：
      // 用该账号的 API Key 打 {base}/user/balance，**不需要额外配置** ——
      // 表单里因此不会多出任何输入框（见 ui-islands 的 balance-query）
      balance: { scheme: 'deepseek' },
    },
    {
      key: 'glm', name: 'GLM Coding', icon: 'glm.png',
      protocol: P.anthropic, baseUrl: 'https://api.z.ai/api/anthropic',
      hint: wbI18n.t('国际版 z.ai 的 Coding 套餐；OpenAI 兼容地址为 https://api.z.ai/api/coding/paas/v4'),
      quirks: { urlSuffix: '?beta=true', headers: { ...ANTHROPIC_BETA_HEADERS } },
      // 智谱国际站余额查询（后端 BALANCE_SCHEME_ZHIPU_ZAI）：用该账号的 API Key
      // 现场铸 SIGN 签名打 api.z.ai 的账户报告接口（美元），**不需要额外配置**
      //（国内站 glm-cn 走的是另一条控制台令牌链路，两个方案不是一回事）
      balance: { scheme: 'zhipu_zai' },
    },
    {
      key: 'glm-cn', name: 'GLM 中国', icon: 'glm-cn.png',
      protocol: P.openai, baseUrl: 'https://open.bigmodel.cn/api/coding/paas/v4',
      hint: wbI18n.t('智谱 bigmodel.cn 的 Coding 套餐，Key 在 open.bigmodel.cn/usercenter/apikeys'),
      // 智谱余额查询（后端 BALANCE_SCHEME_ZHIPU）：打 bigmodel.cn **控制台**
      // 账户余额（人民币元），用的是控制台令牌而不是这里的 API Key ——
      // 表单里会出现一个「访问令牌」输入框（见 balance-query 的方案说明）
      balance: { scheme: 'zhipu' },
    },
    {
      key: 'minimax', name: 'Minimax Coding', icon: 'minimax.png',
      protocol: P.anthropic, baseUrl: 'https://api.minimax.io/anthropic',
      hint: wbI18n.t('国际站 Coding 套餐；OpenAI 兼容地址为 https://api.minimax.io/v1'),
      quirks: {
        urlSuffix: '?beta=true',
        headers: { ...ANTHROPIC_BETA_HEADERS },
        anthropicToolType: 'custom',
      },
    },
    {
      key: 'minimax-cn', name: 'Minimax 中国', icon: 'minimax-cn.png',
      protocol: P.anthropic, baseUrl: 'https://api.minimaxi.com/anthropic',
      hint: wbI18n.t('国内站（minimaxi.com）；OpenAI 兼容地址为 https://api.minimaxi.com/v1'),
      quirks: {
        urlSuffix: '?beta=true',
        headers: { ...ANTHROPIC_BETA_HEADERS },
        anthropicToolType: 'custom',
      },
    },
    {
      key: 'siliconflow', name: 'SiliconFlow', icon: 'siliconflow.png',
      protocol: P.openai, baseUrl: 'https://api.siliconflow.com/v1',
      hint: wbI18n.t('国内站为 https://api.siliconflow.cn/v1'),
    },
    {
      key: 'volcengine-ark', name: '火山方舟', icon: 'volcengine-ark.png',
      protocol: P.openai, baseUrl: 'https://ark.cn-beijing.volces.com/api/coding/v3',
      hint: wbI18n.t('火山引擎 Coding 套餐；通用推理接入点为 https://ark.cn-beijing.volces.com/api/v3'),
    },
    {
      key: 'alicode', name: 'Alibaba', icon: 'alicode.png',
      protocol: P.openai, baseUrl: 'https://coding.dashscope.aliyuncs.com/v1',
      hint: wbI18n.t('阿里云百炼 Coding 套餐（国内站）'),
    },
    {
      key: 'alicode-intl', name: 'Alibaba Coding', icon: 'alicode-intl.png',
      protocol: P.openai, baseUrl: 'https://coding-intl.dashscope.aliyuncs.com/v1',
      hint: wbI18n.t('阿里云百炼 Coding 套餐（国际站）'),
    },
    {
      key: 'alims-intl', name: 'Alibaba Studio', icon: 'alims-intl.png',
      protocol: P.openai, baseUrl: 'https://dashscope-intl.aliyuncs.com/compatible-mode/v1',
      hint: wbI18n.t('阿里云百炼模型服务（国际站，OpenAI 兼容模式）'),
    },
    {
      key: 'alitp-intl', name: 'Alibaba Token Plan', icon: 'alitp-intl.png',
      protocol: P.openai, baseUrl: 'https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1',
      hint: wbI18n.t('阿里云 Token Plan 套餐（国际站）'),
    },
    {
      key: 'xiaomi-tokenplan', name: '小米 MiMo (Token Plan)', icon: 'xiaomi-tokenplan.png',
      protocol: P.openai, baseUrl: 'https://token-plan-sgp.xiaomimimo.com/v1',
      hint: wbI18n.t('小米 MiMo Token Plan 套餐（新加坡站）'),
    },
    {
      key: 'opencode-go', name: 'OpenCode Go', icon: 'opencode-go.png',
      // Go 面与免费档的要求不同：网关按端点自动补上游要求的会话头
      // （x-opencode-session）与产品 UA，**不需要开伪装开关**（那是 Zen
      // 免费档的，Go 面拒绝被塞桩工具）。见后端 emulation 的「Go 面」一节。
      protocol: P.openai, baseUrl: 'https://opencode.ai/zen/go/v1',
      hint: wbI18n.t('同地址换 anthropic 协议即走它的 Claude 兼容端点。转发必须填 OpenCode Go 的 API Key（订阅制；获取模型清单不需要 Key，但发请求上游会回 401「Missing API key」）'),
      // OpenCode Go 余额查询（后端 BALANCE_SCHEME_OPENCODE）：用该账号的
      // API Key 打 {base}/v1/usage，读滚动 / 每周 / 每月三个窗口的剩余百分比，
      // **不需要额外配置**（免费档 OpenCode Zen 不在这一列：它没有余额接口）
      balance: { scheme: 'opencode' },
    },
    {
      key: 'opencode-zen', name: 'OpenCode Zen', icon: 'opencode-zen.png',
      // 端点 2026-10 换过：旧的 /zen/v1 已废弃（打它一律 403 FreeTierError），
      // 当前是 /inference/v1。存量记录里写着的旧值由后端读侧自动迁移
      // （custom_providers::migrate_base_url），不必手改。
      protocol: P.openai, baseUrl: 'https://opencode.ai/inference/v1',
      hint: wbI18n.t('免费模型不用填 Key（默认勾上「该上游无需鉴权」，网关自动用匿名凭证）；付费模型则填 Zen 的 API Key。免费档上游只认官方客户端形态，已默认开启「伪装 OpenCode 官方客户端」'),
      // 免费档的四道校验（匿名凭证 / 四个会话头 / 不带 request·project 头 /
      // 请求体形态）由该开关统一补齐，见后端 providers::custom::emulation
      // 的模块头（含 2026-10-14 对着官方客户端 v2.0.27 的实测记录）
      clientEmulation: 'opencode',
      account: { noAuth: true },
    },
    {
      // Cline Pass 是订阅制 coding 套餐，与上面两张 OpenCode 卡同类，按语义插在
      // 它们后面（末尾那三家是「后到居后」追加的，见那一段的说明）。key 写
      // `clinepass` 而不是 Agent 内置家的 `cline-pass`：预置卡的 key 只是模板
      // 标记（卡片上是 `preset:clinepass`），与内置家的 provider id 是两个命名
      // 空间，别混用 —— 那家的 OAuth 添加入口已从 Agent 段收起，只剩这张卡。
      // 图标：复用 Agent 段那张 Cline 品牌图 cline.png（与内置家 cline-free /
      // cline-pass 同一张图，卡片视觉一致）。
      key: 'clinepass', name: 'Cline Pass', icon: 'cline.png',
      protocol: P.openai, baseUrl: 'https://api.cline.bot/api/v1',
      hint: wbI18n.t('Cline Pass 的 API Key 在 app.cline.bot 获取（订阅制）；端点为 OpenAI 兼容，模型清单以 Cline 控制台为准，模型名在模型管理里按裸名添加（如 glm-5.2 / kimi-k2.7-code / deepseek-v4-pro）；上游按客户端形态校验，所需的客户端请求头已随卡预置'),
      // 请求头抄自本网关 Cline 适配器的官方客户端形态（后端 providers/cline/
      // headers.rs 的 DEFAULT_HEADERS，与 Agent 内置家 cline-pass 发出去的是同
      // 一套）：上游按客户端形态校验，X-CLIENT-TYPE: cline-sdk 是硬门槛（缺了
      // 免费池一律 403），其余版本 / 平台头让请求更贴近官方 CLI。**不要抄别的
      // 网关（如 9Router）的 9router 客户端值** —— 那家的伪装头只适用它自己。
      quirks: {
        headers: {
          'User-Agent': 'Cline/3.0.62',
          'HTTP-Referer': 'https://cline.bot',
          'X-Title': 'Cline',
          'X-IS-MULTIROOT': 'false',
          'X-CLIENT-TYPE': 'cline-sdk',
          'X-CLIENT-VERSION': '3.0.62',
          'X-PLATFORM': 'terminal',
          'X-PLATFORM-VERSION': '3.0.62',
          'X-CORE-VERSION': '0.0.70',
        },
      },
    },
    {
      key: 'ollama-local', name: 'Ollama（本地）', icon: 'ollama-local.png',
      protocol: P.openai, baseUrl: 'http://localhost:11434/v1',
      hint: wbI18n.t('本机 Ollama 的 OpenAI 兼容端点，无需 API Key（默认已勾上「该上游无需鉴权」，鉴权头一个都不发）'),
      account: { noAuth: true },
    },

    // ── 自建聚合面板与商汤日日新（随余额查询一起接入）─────────────
    // 这三家排在末尾：既有的卡片顺序不动（用户对第 1 步的网格已有肌肉记忆），
    // 新卡按「后到居后」追加。sub2api / NewAPI 是**自建**面板，没有固定
    // baseUrl —— 预置卡只预填名称 / 协议，地址由用户自己填（`placeholder`
    // 是给那个输入框的提示，见 add-custom-provider 的预填逻辑）。
    // 三家的 `balance.scheme` 见文件头的字段说明。
    // 图标出处：sub2api 取官方仓库（Wei-Shaw/sub2api）的 assets/logo.svg 栅格化；
    // NewAPI 取官方仓库（QuantumNous/new-api）的 web/public/logo.png；
    // 商汤日日新取官网 www.sensenova.cn 的 icon.png（商汤品牌方格标）。
    {
      key: 'sub2api', name: 'Sub2API', icon: 'sub2api.png',
      protocol: P.openai, baseUrl: '',
      placeholder: 'https://你的 Sub2API 站点地址（OpenAI 兼容填到 /v1）',
      hint: wbI18n.t('自建聚合面板：填你自己的站点地址。余额查询用该账号的 API Key 打 {base}/v1/usage，不需要额外配置'),
      balance: { scheme: 'sub2api' },
    },
    {
      key: 'newapi', name: 'NewAPI', icon: 'newapi.png',
      protocol: P.openai, baseUrl: '',
      placeholder: 'https://你的 NewAPI 站点地址（OpenAI 兼容填到 /v1）',
      hint: wbI18n.t('自建聚合面板：填你自己的站点地址；余额查询要另填 NewAPI 的个人访问令牌（见下方「余额查询」一段）'),
      balance: { scheme: 'new_api' },
    },
    {
      key: 'sensenova', name: '商汤日日新', icon: 'sensenova.png',
      protocol: P.openai, baseUrl: 'https://api.sensenova.cn/compatible-mode/v1',
      hint: wbI18n.t('商汤日日新的 OpenAI 兼容端点；余额查询走控制台账号密码（见下方「余额查询」一段）'),
      balance: { scheme: 'sensenova' },
    },
  ];

  /** 按 key 取一家（找不到给 null，调用方自行回落到手动新建） */
  function presetOf(key) {
    return PRESET_PROVIDERS.find(item => item.key === key) || null;
  }

  /** 图标路径：收录过的家出 assets/providers/<file>，没有返回空串（首字母徽章兜底） */
  function iconOf(key) {
    const preset = presetOf(key);
    return preset?.icon ? `assets/providers/${preset.icon}` : '';
  }

  window.wbPresetProviders = { list: PRESET_PROVIDERS, presetOf, iconOf };
})();
