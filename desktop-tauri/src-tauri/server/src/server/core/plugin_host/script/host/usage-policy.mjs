const JOYCODE_EMPTY_ERROR = "JoyCode 暂未返回额度数据"

export function observeUsageResponse(response, url, context) {
  if (context.provider !== "joycode" || context.action !== "usage.query"
    || url.origin !== "https://api-ai.jd.com" || url.pathname !== "/api"
    || url.searchParams.get("functionId") !== "get_newpoint_ide") return response

  const parse = response.json.bind(response)
  // 复用插件本次读取的 JSON，不额外查询或复制响应流；只记录明确的空额度结果。
  response.json = async () => {
    context.emptyUsage = null
    const value = await parse()
    const data = value?.data
    if (response.ok && [0, "0"].includes(value?.code) && !value.error && value.success !== false
      && !data?.error && Array.isArray(data?.usageItems) && data.usageItems.length === 0
      && typeof data.emptyText === "string" && data.emptyText.trim()) {
      context.emptyUsage = data.emptyText.trim().slice(0, 2000)
    }
    return value
  }
  return response
}

export function compatibleUsageResult(value, context) {
  if (context?.provider !== "joycode" || context.action !== "usage.query" || !context.emptyUsage
    || value?.error !== JOYCODE_EMPTY_ERROR || value.signIn !== "kept"
    || !Array.isArray(value.windows) || value.windows.length > 0
    || value.balance != null || value.plan != null) return value
  return { windows: [], empty: true, emptyText: context.emptyUsage }
}
