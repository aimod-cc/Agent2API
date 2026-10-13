/* Agent2API · Token 计量单位的展示口径（报表读数 / 图表刻度共用） */
/* global */

/**
 * Token 读数只在这里格式化一次，于是「亿 / 万」与「M / k」两套口径不会在
 * 报表的十来处读数里各写一遍、也必然一起切换。
 *
 * ── 为什么要有两套口径 ───────────────────────────────────────
 * 「1140.05M」与「11.4亿」是同一个数，但前者要读三遍才反应得过来量级。
 * 所以默认按**本地量级词**显示（简体万 / 亿、繁体萬 / 億、日文万 / 億、韩文만 / 억），
 * 需要英文缩写（k / M，与上游文档、日志里的写法一致）时可以在设置页关掉。
 *
 * ── 为什么这个开关存 localStorage 而不是 desktop-settings.json ──
 * 它只改变本机界面怎么显示一串数字：不影响进程行为（那是 closeToTray /
 * autostart 的范畴，所以要放主进程），也不该跟着账号数据迁移到别的机器
 * （那边可能是英文习惯）。这与「主题」「设置页当前分类」属于同一类的纯
 * 前端偏好，所以沿用同一套存储方式与 workbuddy-desktop-* 键名前缀。
 *
 * ── 默认口径跟随界面语言 ─────────────────────────────────────
 * 开关没被用户动过时（localStorage 里没有 'on' / 'off'），默认值 = 当前界面语言**有没有
 * 本地量级词**（UNIT_WORDS：zh-Hans / zh-Hant / ja / ko 有，en / pt-BR 没有）：有就用
 * 万 / 億 / 만 / 억 这套，没有就用 k / M。这四种语言的母语习惯本来就说「1.2億」「1.2억」，
 * 默认给 k / M 反而不自然；英文 / 葡语界面则不该突兀地夹着「亿」。存过值就一律尊重用户的
 * 选择，不再跟着语言变 —— 「用户亲手设过的偏好优先级最高」这条在主题 / 缩放上也是同一套。
 * 语言取自 window.wbI18n（ui/i18n.js，head 里先于本脚本加载）；取不到就按简体算。
 *
 * ── 单位怎么选 ───────────────────────────────────────────────
 * 本地量级：一万以内给精确千分位（这个量级下缩写反而看不出差别），再往上按
 *       「万」「亿」两档，各保留一位小数、整数不带小数点（1.2亿 / 8400万）。
 *       量级词统一从 UNIT_WORDS 取（繁体萬 / 億、日文万 / 億、韩文만 / 억），逻辑一份；
 *       英文：同样的分档逻辑，但用 k / M 两个后缀，且与改造前的写法逐字一致
 *       （1.20M / 8.4k）—— 关掉开关就是回到旧观感。
 *
 * ── 轴刻度与读数的差别 ───────────────────────────────────────
 * 本地量级档下每个刻度各自按自己的量级定单位（0 / 6000万 / 1.2亿 / 1.8亿 / 2.4亿），
 * 因为「万」与「亿」的字面量级一目了然；英文档下按**整条轴**的最大值定单位，
 * 免得同一条轴上混排「3,437.5」与「13.8k」这两种看着像两套坐标的写法。
 * 两种取舍的差别由 `formatAxis` 的 `max` 参数表达：本地量级档不用它。
 */
(() => {
  /** 持久化键：沿用项目既有的 workbuddy-desktop-* 前缀 */
  const STORAGE_KEY = 'workbuddy-desktop-chinese-units';

  /** 数字分组的地区格式：拼音/繁体的数字写法一致，都走 zh-CN；其余按各自语言 */
  const NUMBER_LOCALES = {
    'zh-Hans': 'zh-CN',
    'zh-Hant': 'zh-CN',
    ja: 'ja-JP',
    ko: 'ko-KR',
    en: 'en-US',
    'pt-BR': 'pt-BR',
  };

  /**
   * 各语言的「万 / 亿」两个量级词。en / pt-BR 不在这张表里 —— 它们的本地方案就是
   * k / M，直接走 englishTokens；这也让「本地量级」这四个字不必把「万」「亿」字面量
   * 散在函数里。
   */
  const UNIT_WORDS = {
    'zh-Hans': { wan: '万', yi: '亿' },
    'zh-Hant': { wan: '萬', yi: '億' },
    ja: { wan: '万', yi: '億' },
    ko: { wan: '만', yi: '억' },
  };

  /** 当前界面语言（ui/i18n.js 注入）；没就位时按简体处理，维持改造前的中文观感 */
  function currentLocale() {
    try {
      const api = window.wbI18n;
      if (api && typeof api.locale === 'function') return api.locale();
    } catch { /* 忽略：取不到就用默认语言 */ }
    return 'zh-Hans';
  }

  /**
   * 读开关：明确存过 'on' / 'off' 就尊重用户选择；**没存过**时默认跟随界面语言
   * —— 该语言有本地量级词（zh-Hans / zh-Hant / ja / ko）就默认开，en / pt-BR 默认关。
   * 存储抛错（隐私模式）时按同一个「跟随语言」的默认处理。
   */
  function readChinese() {
    let stored = null;
    try {
      stored = localStorage.getItem(STORAGE_KEY);
    } catch {
      stored = null;
    }
    if (stored === 'off') return false;
    if (stored === 'on') return true;
    return Object.prototype.hasOwnProperty.call(UNIT_WORDS, currentLocale());
  }

  /** 默认口径：跟着界面语言走（有本地量级词的语言用万 / 億 / 만，其余用 k / M） */
  let chinese = readChinese();

  // ─── 单位换算 ──────────────────────────────

  const formatInt = value => (Number(value) || 0).toLocaleString(NUMBER_LOCALES[currentLocale()] || 'en-US');

  /** 四舍五入到 `digits` 位，并顺手去掉尾随的零（2.40 → 2.4、1.00 → 1）。
   *  走 Number 而不是字符串 trim：`(1.00).toFixed(1)` 是 "1.0"，
   *  而 `Number("1.0")` 再拼进模板就是 "1"，不必自己写一个去零正则。 */
  const fix = (value, digits) => Number(value.toFixed(digits));

  /** 本地量级：一万以内给精确千分位，再往上按万 / 亿两档缩写（量级词取自 UNIT_WORDS） */
  function localTokens(value, words) {
    const num = Number(value) || 0;
    if (num < 10_000) return formatInt(num);
    if (num < 100_000_000) {
      const wan = fix(num / 10_000, 1);
      // 9999.95 万往上会被四舍五入成「10000万」，进位成「1亿」才符合读数直觉
      return wan >= 10_000 ? `${fix(num / 100_000_000, 1)}${words.yi}` : `${wan}${words.wan}`;
    }
    return `${fix(num / 100_000_000, 1)}${words.yi}`;
  }

  /** 英文量级：k / M 两档，与改造前 report.js 的写法保持一致（en / pt-BR 的本地方案） */
  function englishTokens(value) {
    const num = Number(value) || 0;
    if (num < 10_000) return formatInt(num);
    if (num < 1_000_000) return `${fix(num / 1000, 1)}k`;
    return `${fix(num / 1_000_000, 2)}M`;
  }

  /** 读数：概览小格、图表标注、tooltip 都走这一个函数 */
  const formatTokens = value => {
    if (!chinese) return englishTokens(value);
    const words = UNIT_WORDS[currentLocale()];
    return words ? localTokens(value, words) : englishTokens(value);
  };

  /**
   * 纵轴刻度文案。`max` 只在英文档下用到（整条轴统一单位，见文件头）。
   * 小量级下刻度多是整数；上限只有个位数时可能出现 .5，保留一位即可。
   */
  function formatAxis(value, max) {
    if (chinese) {
      const words = UNIT_WORDS[currentLocale()];
      if (words) return localTokens(value, words);
    }
    if (max >= 1_000_000) return `${fix(value / 1_000_000, 2)}M`;
    if (max >= 10_000) return `${fix(value / 1000, 1)}k`;
    return Number.isInteger(value) ? formatInt(value) : value.toFixed(1);
  }

  /** 切换口径。值没变就什么都不做：设置页回填开关时也会调到这里 */
  function setChinese(on) {
    const next = on === true;
    if (next === chinese) return;
    chinese = next;
    try {
      localStorage.setItem(STORAGE_KEY, next ? 'on' : 'off');
    } catch { /* 存储不可用只影响下次启动，本次会话照常 */ }
    // 用事件而不是直接回调：订阅方（报表页）与设置页互不认识，谁关心谁注册，
    // 以后再加订阅方也不必回来改这里
    window.dispatchEvent(new CustomEvent('wb-units-changed'));
  }

  window.wbUnits = {
    formatInt,
    formatTokens,
    formatAxis,
    isChinese: () => chinese,
    setChinese,
  };
})();
