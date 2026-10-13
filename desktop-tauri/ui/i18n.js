/* Agent2API · 前端多语言运行时（经典脚本，零依赖） */
/* global */

/**
 * 界面文案的运行时查表与静态替换器。
 *
 * ── 「中文即键」── 为什么不做 key → 文案的间接层 ────────────────
 * t('账号') 的键就是简体原文本身。好处是：源码里读到的就是用户看到的那句话，
 * 不必对着 'accounts.title' 这类符号名去猜内容，词典缺一条也不会出现「界面上
 * 冒出一串英文键名」的尴尬 —— 查不到就原样显示键（也就是正确的简体中文）。
 * 代价是改文案等于改键，得同步改词典；这对一个以中文为母语、翻译是后补的项目
 * 是划算的取舍。
 *
 * ── 词典从哪来、什么时候到 ────────────────────────────────────
 * 词典**不在**本文件里。index.html / login.html 的 head 引导脚本按
 * `workbuddy-desktop-locale` 解析出的语言，只 document.write 注入 i18n/manifest.js
 * —— 那是域清单（window.__WB_I18N_DOMAINS）；各域词典文件由本脚本在执行时补写
 * （此刻清单已就位、页面仍在解析期，写出的 <script> 插在本脚本之后、业务脚本之前）。
 * 这些词典脚本（构建产物，见 desktop-tauri/i18n/）会往 window.wbI18nDict 上
 * Object.assign，且都在任何业务脚本之前执行完毕；即便某条缺失或某域文件 404，
 * t() 也只是回落简体，页面不会报错。
 *
 * ── 回落策略只有一条 ──────────────────────────────────────────
 * 非空字符串才算译到位，其余（undefined / 空串 / 非字符串）一律回落键本身。
 * 空串绝不是「有效译文」—— 那会把界面变白，比显示中文糟得多。
 *
 * ── 静态文本扫描器（sweeper）─────────────────────────────────
 * 经典脚本与各页面里还留着写死的静态文案。它们不该为了翻译全部改成 JS 拼接，
 * 而是就地打上 data-i18n 系列属性：DOMContentLoaded 时本脚本扫一遍、就地替换。
 * 属性族各管一处目标（见下方 ATTR_TARGETS）。React 岛（islands/ui.js）里的
 * 文案走 i18n.ts 的 t()，不依赖这个扫描器。
 *
 * ── 切换语言 = 整页刷新 ───────────────────────────────────────
 * 本地页面重载的代价极小，而运行时热切换要处理「已渲染的岛 / 已绑定的事件 /
 * 已生成的 DOM」三处一致性，得不偿失。所以 setLocale 只负责写 localStorage
 * 再 location.reload()，让 head 引导脚本按新语言重新注入词典、页面从零重建。
 */
(() => {
  /** 语言偏好持久化键，值 'auto' 或某个 locale 代码 */
  const STORAGE_KEY = 'workbuddy-desktop-locale'
  /** 具体语言代码（不含 'auto'）：白名单，拼 src 前也靠它校验 */
  const CODES = ['zh-Hans', 'zh-Hant', 'en', 'ja', 'ko', 'pt-BR']

  /**
   * 界面语言下拉的选项：label 恒用「该语言自己的写法」，不翻译（选了也看得懂）；
   * nameKey 是这门外语的中文名，供设置页用 t() 译成**当前界面语言**，拼出
   * 「English（英语）」式双语选项（两者相同则只显示一次）—— 见 settings-page 的
   * LanguageRow。中文名同样以「中文即键」进词典：en/ja/ko/pt-BR 有人工译文，
   * zh-Hant 由 opencc 生成。
   */
  const LOCALES = [
    { code: 'zh-Hans', label: '简体中文', nameKey: '简体中文' },
    { code: 'zh-Hant', label: '繁體中文', nameKey: '繁体中文' },
    { code: 'en', label: 'English', nameKey: '英语' },
    { code: 'ja', label: '日本語', nameKey: '日语' },
    { code: 'ko', label: '한국어', nameKey: '韩语' },
    { code: 'pt-BR', label: 'Português (Brasil)', nameKey: '巴西葡萄牙语' },
  ]

  /** 属性族 → 就地替换的目标属性（data-i18n-tip 写回 data-tip，供 tooltip.js 用） */
  const ATTR_TARGETS = {
    'data-i18n-title': 'title',
    'data-i18n-placeholder': 'placeholder',
    'data-i18n-tip': 'data-tip',
    'data-i18n-aria-label': 'aria-label',
  }

  /* ─── 查表 ─────────────────────────────── */

  /**
   * 取键的译文：非空字符串才算译到位，否则回落键本身。
   * params 给出时把 {name} 形参按值替换 —— 取不到的形参替换成空串（不留占位符）。
   */
  function t(key, params) {
    const value = (window.wbI18nDict || {})[key]
    let text = (typeof value === 'string' && value) ? value : key
    if (params) {
      text = text.replace(/\{(\w+)\}/g, (_match, name) => (
        Object.prototype.hasOwnProperty.call(params, name) ? String(params[name]) : ''
      ))
    }
    return text
  }

  /* ─── 语言解析与切换 ───────────────────── */

  /**
   * 'auto' 或非法值 → 按 navigator.languages / navigator.language 逐个匹配：
   * zh-TW / zh-HK / zh-MO / 含 hant → zh-Hant；其余 zh* → zh-Hans；
   * ja* → ja；ko* → ko；pt* → pt-BR；都不中 → en。
   * 规则与 index.html / login.html head 引导脚本里那份逐字一致（那份要在 i18n.js
   * 加载前先定语言去注入词典，没法共用本函数）—— 改这里必须同步改那两处。
   */
  function resolveLocale(raw) {
    if (raw && raw !== 'auto' && CODES.indexOf(raw) >= 0) return raw
    const tags = (navigator.languages && navigator.languages.length)
      ? navigator.languages
      : [navigator.language]
    for (const rawTag of tags) {
      const tag = String(rawTag || '').toLowerCase()
      if (!tag) continue
      if (tag === 'zh-tw' || tag === 'zh-hk' || tag === 'zh-mo' || tag.indexOf('hant') >= 0) return 'zh-Hant'
      if (tag.indexOf('zh') === 0) return 'zh-Hans'
      if (tag.indexOf('ja') === 0) return 'ja'
      if (tag.indexOf('ko') === 0) return 'ko'
      if (tag.indexOf('pt') === 0) return 'pt-BR'
    }
    return 'en'
  }

  /** 存储里存的原始值：'auto'（默认）或具体语言代码 */
  function rawLocale() {
    try {
      const value = localStorage.getItem(STORAGE_KEY)
      return value || 'auto'
    } catch {
      return 'auto'
    }
  }

  /** 已解析的具体语言（永不返回 'auto'） */
  function locale() {
    return resolveLocale(rawLocale())
  }

  /** 切换语言：写存储后整页刷新；存储被禁（隐私模式）时也照样刷新 */
  function setLocale(value) {
    const next = (value === 'auto' || CODES.indexOf(value) >= 0) ? value : 'auto'
    try {
      localStorage.setItem(STORAGE_KEY, next)
    } catch { /* 存储不可用：本次切换至少还能刷新一下当前语言 */ }
    location.reload()
  }

  /* ─── 静态文本扫描器 ───────────────────── */

  /**
   * 就地替换静态文案。可重复执行：data-i18n 的文本换成译文后，键仍在 dataset 里，
   * 再跑一次得到同样的结果；裸 data-i18n 用 textContent 当键，第一次替换后键变成
   * 译文且查不到 → 回落译文本身，同样稳定，不会越套越乱。
   */
  function sweep(root) {
    const scope = root || document
    for (const el of scope.querySelectorAll('[data-i18n]')) {
      const key = el.dataset.i18n || (el.textContent || '').trim()
      if (key) el.textContent = t(key)
    }
    for (const [attr, target] of Object.entries(ATTR_TARGETS)) {
      for (const el of scope.querySelectorAll('[' + attr + ']')) {
        const key = el.getAttribute(attr)
        if (key) el.setAttribute(target, t(key))
      }
    }
  }

  /* ─── 启动 ─────────────────────────────── */

  const current = locale()
  document.documentElement.lang = current

  // 各域词典注入：head 引导脚本只写 manifest.js（域清单）—— 它在 document.write
  // 之后同步读不到 __WB_I18N_DOMAINS（manifest 要等引导脚本块结束才执行），所以
  // 各域文件挪到这里补写。本脚本静态排在 manifest.js 之后执行，此刻清单已就位；
  // 且页面仍在解析期，写出的 <script> 会插在本脚本之后、所有业务脚本之前。
  // 简体没有词典目录（查不到即显示键本身），跳过。
  if (current !== 'zh-Hans' && document.readyState === 'loading') {
    const domains = window.__WB_I18N_DOMAINS || []
    for (const domain of domains) {
      document.write('<script src="i18n/' + current + '/' + domain + '.js"><\/script>')
    }
  }

  window.wbI18n = {
    t,
    locale,
    rawLocale,
    setLocale,
    sweep,
    LOCALES,
  }

  // 页面解析完再扫一次：此时各业务脚本已执行、静态 DOM 已就位
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', () => sweep(), { once: true })
  } else {
    sweep()
  }
})()
