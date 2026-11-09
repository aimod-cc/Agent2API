import { createHash } from "node:crypto"

export function pluginHeaders(headers, params, body) {
  const source = new Headers(params.headers || {})
  if (params.session) headers.set("x-magpie-session", params.session)
  const subagent = source.get("x-claude-code-agent-id")
  if (subagent) headers.set("x-magpie-subagent", subagent)
  if (params.fallback === true) headers.set("x-magpie-fallback", "1")
  const parts = value => typeof value === "string" ? value : Array.isArray(value)
    ? value.map(part => typeof part?.text === "string" ? part.text : "").join("\n") : ""
  const contents = [parts(body.system), parts(body.instructions),
    ...(body.messages ?? []).filter(message => message.role === "system" || message.role === "developer").map(message => parts(message.content))].join("\n")
  const work = /(?:Working directory|Current working directory|cwd|工作目录):\s*([^\r\n<]+)/i.exec(contents)?.[1]?.trim()
  if (work && work.length <= 4096) headers.set("x-magpie-work-dir", encodeURIComponent(work))
  const git = /(?:Is a git repository|Is directory a git repo):\s*(true|false|yes|no)/i.exec(contents)?.[1]
  if (git) headers.set("x-magpie-work-git", /true|yes/i.test(git) ? "yes" : "no")
}

export function signatureScope(provider, params) {
  const headers = params.headers || {}
  return createHash("sha256").update(JSON.stringify([provider.id, params.account, params.session || "",
    headers.authorization || headers["x-api-key"] || ""])).digest("hex")
}

export function variantOptions(protocol, model, level, body) {
  const variant = model?.variants?.[level]
  const options = { ...model?.options, ...(variant && typeof variant === "object" ? variant : {}) }
  if (protocol === "gemini") {
    body.generationConfig ??= {}
    if (options.thinkingConfig) body.generationConfig.thinkingConfig = options.thinkingConfig
    for (const key of ["temperature", "topK", "topP", "maxOutputTokens", "responseModalities", "mediaResolution"]) {
      if (options[key] !== undefined) body.generationConfig[key] = options[key]
    }
    if (options.safetySettings) body.safetySettings = options.safetySettings
  } else if (protocol === "anthropic") {
    if (options.thinking) body.thinking = options.thinking
    if (options.effort) body.output_config = { ...body.output_config, effort: options.effort }
    for (const key of ["topK", "topP", "temperature"]) if (options[key] !== undefined) body[{ topK: "top_k", topP: "top_p", temperature: "temperature" }[key]] = options[key]
    if (body.thinking?.type !== "disabled" && body.thinking) { delete body.temperature; delete body.top_p }
  } else if (protocol === "responses") {
    if (options.reasoningEffort) body.reasoning = { ...body.reasoning, effort: options.reasoningEffort }
    if (options.reasoningSummary) body.reasoning = { ...body.reasoning, summary: options.reasoningSummary }
  } else {
    if (options.reasoningEffort) body.reasoning_effort = options.reasoningEffort
    if (options.chatTemplateKwargs) body.chat_template_kwargs = options.chatTemplateKwargs
  }
  if (model.temperature === false || model.capabilities?.temperature === false) {
    delete body.temperature; delete body.top_p
    if (body.generationConfig) { delete body.generationConfig.temperature; delete body.generationConfig.topP }
  }
  return body
}
