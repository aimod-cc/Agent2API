import { rpc } from "./wire.mjs"

const dataURL = value => {
  const match = /^data:([^;,]+)(?:;[^,]*)?;base64,(.*)$/s.exec(value || "")
  return match ? { inlineData: { mimeType: match[1], data: match[2] } } : null
}

function parts(content) {
  if (typeof content === "string") return content ? [{ text: content }] : []
  return (Array.isArray(content) ? content : []).flatMap(part => {
    if (typeof part?.text === "string") return [{ text: part.text }]
    const image = part?.image_url?.url || (typeof part?.image_url === "string" ? part.image_url : "")
    const file = part?.file?.file_data || part?.file_data || part?.source?.data
    if (image) return [dataURL(image) || { fileData: { mimeType: "image/jpeg", fileUri: image } }]
    if (part?.input_audio) return [{ inlineData: { mimeType: `audio/${part.input_audio.format || "wav"}`, data: part.input_audio.data } }]
    if (file) {
      const encoded = dataURL(file)
      if (encoded) return [encoded]
      if (part.source?.media_type) return [{ inlineData: { mimeType: part.source.media_type, data: file } }]
    }
    return []
  })
}

function json(value) {
  if (typeof value !== "string") return value ?? {}
  try { return JSON.parse(value) } catch { return { output: value } }
}

function schema(value) {
  if (Array.isArray(value)) return value.map(schema)
  if (!value || typeof value !== "object") return value
  return Object.fromEntries(Object.entries(value).filter(([key]) => !["$schema", "$id", "$comment"].includes(key))
    .map(([key, value]) => [key, schema(value)]))
}

export async function geminiRequest(chat, session) {
  const contents = [], system = [], toolsByID = new Map()
  const append = (role, content) => {
    if (!content.length) return
    const previous = contents.at(-1)
    if (previous?.role === role) previous.parts.push(...content)
    else contents.push({ role, parts: content })
  }
  for (const message of chat.messages ?? []) {
    if (message.role === "system" || message.role === "developer") { system.push(...parts(message.content)); continue }
    if (message.role === "tool" || message.role === "function") {
      const name = toolsByID.get(message.tool_call_id) || message.name || message.tool_call_id
      if (!name) throw new Error("Tool result has no matching name")
      const response = json(message.content)
      append("user", [{ functionResponse: { name, ...(message.tool_call_id ? { id: message.tool_call_id } : {}),
        response: response && typeof response === "object" && !Array.isArray(response) ? response : { output: response } } }])
      continue
    }
    const content = parts(message.content)
    for (const call of message.tool_calls ?? []) {
      const name = call.function?.name
      if (!name) continue
      toolsByID.set(call.id, name)
      const explicit = call.extra_content?.google?.thought_signature || call.thoughtSignature || call.thought_signature
      const signature = explicit || (await rpc("script.signature", { session, call: call.id }).catch(() => null))
      content.push({ functionCall: { name, args: json(call.function.arguments), ...(call.id ? { id: call.id } : {}) },
        ...(signature ? { thoughtSignature: signature } : {}) })
    }
    append(message.role === "assistant" ? "model" : "user", content)
  }
  const generationConfig = {}
  for (const [source, target] of Object.entries({ temperature: "temperature", top_p: "topP", top_k: "topK", max_tokens: "maxOutputTokens", max_completion_tokens: "maxOutputTokens", seed: "seed", n: "candidateCount", presence_penalty: "presencePenalty", frequency_penalty: "frequencyPenalty" })) {
    if (chat[source] !== undefined) generationConfig[target] = chat[source]
  }
  if (chat.stop) generationConfig.stopSequences = Array.isArray(chat.stop) ? chat.stop : [chat.stop]
  if (chat.response_format?.type === "json_object" || chat.response_format?.type === "json_schema") {
    generationConfig.responseMimeType = "application/json"
    if (chat.response_format.json_schema?.schema) generationConfig.responseJsonSchema = schema(chat.response_format.json_schema.schema)
  }
  const declarations = [], native = []
  for (const tool of chat.tools ?? []) {
    if (tool.type === "function" || tool.function) {
      const value = tool.function || tool
      declarations.push({ name: value.name, description: value.description || "", parametersJsonSchema: schema(value.parameters || { type: "object", properties: {} }) })
    } else if (tool.type === "web_search" || tool.type === "web_search_preview") native.push({ googleSearch: {} })
    else if (tool.googleSearch || tool.codeExecution || tool.urlContext) native.push(tool)
  }
  const result = { contents, generationConfig }
  if (system.length) result.systemInstruction = { parts: system }
  if (declarations.length || native.length) result.tools = [...(declarations.length ? [{ functionDeclarations: declarations }] : []), ...native]
  const choice = chat.tool_choice
  if (choice === "none") result.toolConfig = { functionCallingConfig: { mode: "NONE" } }
  else if (choice === "required" || choice?.function?.name) result.toolConfig = {
    functionCallingConfig: { mode: "ANY", ...(choice?.function?.name ? { allowedFunctionNames: [choice.function.name] } : {}) },
  }
  for (const key of ["safetySettings", "cachedContent"]) if (chat[key] !== undefined) result[key] = chat[key]
  return result
}
