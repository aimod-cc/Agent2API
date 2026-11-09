const kinds = ["text", "image", "audio", "video", "pdf"]
const modalities = values => Object.fromEntries(kinds.map(kind => [kind, (values ?? ["text"]).includes(kind)]))

export function normalizeModels(provider, models) {
  const original = provider.originalID || provider.id
  return Object.fromEntries(Object.entries(models ?? {}).filter(([id, model]) => id && model && typeof model === "object"
    && !Array.isArray(model) && model.disabled !== true && model.status !== "deprecated").map(([id, model]) => {
    const base = provider.seedModels?.[id] ?? {}
    const api = { id: model.id || base.api?.id || id,
      url: model.provider?.api || model.api?.url || base.api?.url || provider.config.api || "",
      npm: model.provider?.npm || model.npm || model.api?.npm || base.api?.npm || provider.config.npm || "@ai-sdk/openai-compatible",
      ...model.api }
    const capabilities = { temperature: true, toolcall: true, input: { text: true }, output: { text: true }, ...base.capabilities,
      ...(model.temperature !== undefined ? { temperature: model.temperature } : {}),
      ...(model.tool_call !== undefined ? { toolcall: model.tool_call } : {}),
      ...(model.reasoning !== undefined ? { reasoning: model.reasoning } : {}),
      ...(model.modalities?.input ? { input: modalities(model.modalities.input) } : {}),
      ...(model.modalities?.output ? { output: modalities(model.modalities.output) } : {}), ...model.capabilities }
    return [id, { ...base, ...model, id, providerID: original, name: model.name || base.name || id, api, capabilities,
      limit: { context: 0, output: 0, ...base.limit, ...model.limit }, options: { ...base.options, ...model.options },
      headers: { ...base.headers, ...model.headers }, variants: model.variants ?? base.variants ?? {} }]
  }))
}

export function modelRecords(models, loads, privateRecord = false) {
  return Object.entries(models ?? {}).map(([id, model]) => {
    const caps = model.capabilities ?? {}
    const input = model.modalities?.input ?? []
    const levels = Object.keys(model.variants ?? {})
    const load = loads?.get(id)
    return {
      id, name: model.name || id,
      maxOutputTokens: model.limit?.output ?? 0, maxInputTokens: model.limit?.context ?? 0,
      supportsToolCall: caps.toolcall ?? model.tool_call ?? true,
      supportsImages: caps.input?.image ?? input.includes("image"),
      supportsVideo: caps.input?.video ?? input.includes("video"),
      supportsReasoning: caps.reasoning ?? model.reasoning ?? false,
      input_modalities: kinds.filter(kind => caps.input?.[kind] === true || input.includes(kind)),
      output_modalities: kinds.filter(kind => caps.output?.[kind] === true || model.modalities?.output?.includes(kind)),
      reasoningLevels: levels, reasoningBindingSupported: levels.length > 0, reasoningSource: "plugin",
      scriptApi: { npm: model.api?.npm },
      ...(privateRecord ? { scriptModel: model, variants: model.variants ?? {} } : {}),
      cost: model.cost, free: model.free, rate: model.rate, rateWas: model.rateWas,
      temperature: model.temperature, serviceTiers: model.serviceTiers, fast: model.fast,
      searches: model.searches, decides: model.decides,
      ...(load && Date.now() - load.at < 300_000 ? { load: load.value } : {}),
    }
  })
}

export function modelsFromRecords(records, provider) {
  return normalizeModels(provider, Object.fromEntries((records ?? []).filter(row => typeof row.id === "string").map(row => [row.id,
    row.scriptModel ?? { ...(provider.seedModels?.[row.id] ?? {}), name: row.name || row.id,
      ...(row.scriptApi ? { api: row.scriptApi } : {}), ...(row.scriptOptions ? { options: row.scriptOptions } : {}),
      ...(row.scriptProvider ? { provider: row.scriptProvider } : {}), cost: row.cost,
      limit: { context: row.maxInputTokens, output: row.maxOutputTokens }, tool_call: row.supportsToolCall,
      reasoning: row.supportsReasoning, modalities: { input: row.supportsImages ? ["text", "image"] : ["text"], output: ["text"] },
      variants: row.variants ?? {} } ])))
}
