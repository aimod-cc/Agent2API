const languageKeys = {
  "zh-CN": ["zh-CN", "zh-Hans", "zh"],
  "zh-Hans": ["zh-Hans", "zh-CN", "zh"],
  "zh-Hant": ["zh-Hant", "zh-TW", "zh-HK"],
  "pt-BR": ["pt-BR", "pt"],
}

export function localizedText(value, translations, locale = "zh-CN") {
  const keys = languageKeys[locale] || [locale, locale.split("-")[0]]
  if (translations && typeof translations === "object" && !Array.isArray(translations)) {
    for (const key of keys) {
      const text = translations[key]
      if (typeof text === "string" && text.trim()) return text
    }
  }
  return typeof value === "string" ? value : ""
}
