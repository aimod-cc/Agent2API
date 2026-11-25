/* 网页语言只持久化在 localStorage；原生托盘与系统对话框需在每次建窗后同步一次。 */
(() => {
  let sent = '';

  function syncNativeLocale() {
    const locale = window.wbI18n?.locale?.() || document.documentElement.lang;
    const setUiLocale = window.workbuddyDesktop?.setUiLocale;
    if (!locale || locale === sent || typeof setUiLocale !== 'function') return;
    sent = locale;
    try {
      Promise.resolve(setUiLocale(locale)).catch(() => {
        if (sent === locale) sent = '';
      });
    } catch {
      sent = '';
    }
  }

  syncNativeLocale();
  window.addEventListener('wb:app-ready', syncNativeLocale, { once: true });
})();
