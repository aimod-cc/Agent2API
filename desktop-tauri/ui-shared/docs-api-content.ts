import { t } from '../ui-kit/src/i18n'
import { apiExampleCode, type ApiExample } from './docs-api-examples'
import { imageApiSections } from './docs-api-images'
import { toolApiSections } from './docs-api-tools'

export type ApiField = { name: string; type: string; description: string }
export type ApiSection = {
  id: string
  title: string
  path?: string
  description: string
  paragraphs?: string[]
  fields?: ApiField[]
  points?: string[]
  warning?: string
  examples?: ApiExample[]
  /** 整份 Markdown 文档（工具服务 API 这类长文用它，不走五语词典） */
  markdown?: string
}

export function apiReferenceSections(origin: string): ApiSection[] {
  const code = apiExampleCode(origin)
  const example = (label: string, value: string, language: ApiExample['language'] = 'bash'): ApiExample => ({ label, code: value, language })
  return [
    {
      id: 'start', title: t('接入与鉴权'),
      description: t('先确定网关地址、Key 和模型，再选择客户端支持的协议。'),
      paragraphs: [
        t('本页说明对外的 /v1 接口，不是管理面板的 /api 接口。三种对话协议共用模型目录、模型映射、账号池和网关 Key，但请求与响应格式不同。'),
        t('OpenAI SDK 的 base_url 填「接口地址」章节中以 /v1 结尾的 Base URL，不要填完整的 /chat/completions。Anthropic 官方 SDK 与 Claude Code 的 ANTHROPIC_BASE_URL 填网关根地址，不带 /v1；它们会自行追加 /v1/messages。其他客户端是否自动追加 /v1，以其地址栏说明为准。'),
        t('地址速查与请求示例使用当前网关地址。从其他设备访问时，127.0.0.1 指向那台设备自身，必须换成实际可达的网关地址。'),
        t('请求示例适用于 Bash / Git Bash。YOUR_GATEWAY_KEY、CHAT_MODEL_ID、IMAGE_MODEL_ID 和图片占位地址都需要替换；返回示例仅展示结构，能力位、Token 数量和时间戳不是当前网关的实测值。'),
      ],
      fields: [
        { name: 'Authorization', type: 'HTTP header', description: t('推荐使用 Authorization: Bearer YOUR_GATEWAY_KEY；Key 来自「网关 Key」页，并且必须处于启用状态。') },
        { name: 'x-api-key', type: 'HTTP header', description: t('也可使用 x-api-key: YOUR_GATEWAY_KEY，常用于 Anthropic 客户端。两种鉴权头任选一种即可，不要填提供商账号令牌或面板登录令牌。') },
        { name: 'Content-Type', type: 'HTTP header', description: t('JSON 请求使用 application/json。图片编辑上传文件时使用 multipart/form-data；curl -F 会自动生成 boundary，不要手动覆盖此请求头。') },
        { name: 'anthropic-version', type: 'HTTP header', description: t('Anthropic SDK 会自动携带版本头；手写 Messages 请求可使用 2023-06-01。/v1/models 收到该头或 x-api-key 时返回 Anthropic 列表形态。') },
      ],
      points: [
        t('模型调用始终需要启用且可用的网关 API Key，即使只在本机使用也不能免鉴权。管理员登录会话只用于管理面板，不能代替模型调用的 Key。'),
        t('GET /health、GET /v1/models 和网关临时图片下载不强制检查 Key。模型清单可在配 Key 前读取，但这不能证明 Key 正确；应以携带该 Key 的实际业务请求为准。'),
        t('每把 Key 的可用模型、可用提供商限制同样适用于生图和编辑图。模型名称使用 /v1/models 中的 id 或已配置的映射名，不要直接使用展示名称。'),
      ],
      examples: [example(t('SDK 示例使用的环境变量'), code.setup), example(t('读取获准使用的模型'), code.models)],
    },
    {
      id: 'models', title: t('模型发现与健康检查'), path: 'GET /v1/models · GET /health',
      description: t('通过能力字段选择正确模型；健康检查只说明网关可访问。'),
      paragraphs: [
        t('模型清单反映当前账号、模型启停与映射配置。不带 Key 时可读取公开清单；携带 Key 时必须有效，返回内容按该 Key 的授权范围过滤，无效 Key 返回 401。'),
        t('不携带 Anthropic 相关头时，模型接口返回 OpenAI 风格的 object=list、data 数组和 meta 摘要；携带 anthropic-version 或 x-api-key 时，返回 data、has_more、first_id、last_id，并保留能力扩展字段。'),
        t('GET /health 返回 status=ok 或 degraded。公网安全模式只返回简化探活信息；HTTP 200 或 status=ok 不代表所有提供商在线、账号有额度或图片模型可调用。'),
      ],
      fields: [
        { name: 'data[].id', type: 'string', description: t('业务请求的 model 值。owned_by 表示目录条目的归属，不保证每次请求都由该提供商承载；同名模型与映射可能有多个路由。') },
        { name: 'kind', type: 'chat | image', description: t('chat 用于三种对话入口；image 用于 /v1/images/generations 和 /v1/images/edits。不要把图片模型提交给对话入口。') },
        { name: 'supports_images', type: 'boolean', description: t('表示支持图片输入，不等于支持生成图片。对话模型可用于看图；编辑模型也可能声明此字段。') },
        { name: 'supports_image_generation', type: 'boolean', description: t('表示目录条目属于生图模型。调用还需要当前插件已接入生成能力，且存在获准使用的可用账号。') },
        { name: 'supports_image_editing', type: 'boolean', description: t('表示当前目录与插件声明支持参考图编辑。编辑前检查此字段，不要仅凭模型名称包含 image 判断。') },
        { name: 'supports_tool_call / supports_reasoning', type: 'boolean', description: t('分别表示工具调用与思考能力。是否生效仍取决于实际承载模型、提供商和相关设置。') },
        { name: 'input_modalities / output_modalities', type: 'string[]', description: t('描述输入与输出模态，例如 text、image、video。字段缺失不表示自动支持其他文件、音频或服务端上传接口。') },
        { name: 'max_input_tokens / max_output_tokens', type: 'number', description: t('目录声明的上下文与最大输出预算，可能省略。预算和余额不是同一概念，也不能替代上游的实际限制。') },
      ],
      examples: [example(t('模型清单结构示例'), code.modelResponse, 'json'), example(t('检查网关是否可访问'), code.health)],
    },
    {
      id: 'chat', title: 'OpenAI Chat Completions', path: 'POST /v1/chat/completions',
      description: t('通用对话入口，适合大多数 OpenAI 兼容客户端。'),
      paragraphs: [t('非流式响应读取 choices[].message；流式响应读取 choices[].delta。每次请求由客户端携带 messages 历史，网关不会根据上一条响应 id 自动恢复对话。')],
      fields: [
        { name: 'model', type: 'string', description: t('推荐显式指定当前目录中的对话模型 id。省略时尝试网关默认模型；默认选择也受该 Key 的授权范围限制。') },
        { name: 'messages', type: 'array', description: t('必填。包含 role 与 content；常见角色为 system、user、assistant，工具结果使用 tool。文本可直接使用字符串，多模态使用内容块数组。') },
        { name: 'stream', type: 'boolean', description: t('默认 false，返回完整 JSON；true 返回 text/event-stream。curl 查看流式输出时添加 -N，客户端应按完整 SSE 事件解析而不是按网络分块解析。') },
        { name: 'stream_options.include_usage', type: 'boolean', description: t('流式用量可请求 include_usage=true；最终是否返回用量取决于上游，不应假设每个增量都有 usage。') },
        { name: 'max_tokens / max_completion_tokens', type: 'integer', description: t('输出 Token 上限，具体支持字段和预算以模型为准。不要同时设置冲突的上限；达到上限时 finish_reason 可能为 length。') },
        { name: 'temperature / top_p', type: 'number', description: t('采样参数，是否支持和有效范围由提供商与模型决定。协议兼容不代表每家上游都支持全部调参项。') },
        { name: 'tools / tool_choice', type: 'array / string | object', description: t('使用支持工具调用的模型。函数声明、调用结果配对与多模态请求示例见「看图与工具调用」章节。') },
        { name: 'reasoning_effort / response_format', type: 'string / object', description: t('分别请求思考档位与结构化输出；能力和可用取值依赖模型。不要因字段被接受就假定上游已经执行。') },
      ],
      points: [
        t('Chat 流式成功通常以 data: [DONE] 收尾。需要同时处理 delta.content、思考字段和 delta.tool_calls；工具参数可能跨多个增量拼接。'),
        t('usage 的 prompt_tokens、completion_tokens、total_tokens 表示用量，不等于统一货币价格。上游未提供时可能缺失或为 0。'),
        t('流式开始后 HTTP 状态码无法再改；即使已经收到 200，也要检查流内错误、结束原因及是否正常收尾，不能把断流视为成功。'),
      ],
      examples: [example(t('非流式对话请求'), code.chat), example(t('流式对话请求'), code.chatStream), example(t('非流式对话响应'), code.chatResponse, 'json')],
    },
    {
      id: 'tools', title: t('看图与工具调用'),
      description: t('图片输入是理解图片，工具调用是由客户端执行动作；二者都需要模型能力支持。'),
      paragraphs: [
        t('看图请求使用对话模型，并确认 supports_images=true。Chat 使用 text 与 image_url 内容块，图片值可为上游支持的 HTTP(S) URL 或 data URL；可用格式与大小取决于目标模型。「参考图编辑」章节的上传限制不应直接套用到所有对话模型。'),
        t('自定义函数工具通过 tools 声明。模型返回 tool_calls 后，客户端校验参数、执行函数，再把 assistant 工具调用和 role=tool 结果一起放入下一次 messages；tool_call_id 必须对应，不能遗漏并行调用的结果。'),
        t('Responses 使用 function_call / function_call_output 配对，Anthropic 使用 tool_use / tool_result 配对。协议转换会处理这些格式，但不会替客户端执行自定义函数。上游原生搜索等服务端工具是否可用取决于提供商，不能当成网关统一能力。'),
      ],
      warning: t('内置 AI 助手已有 generate_images 与 edit_images 工具，可在助手设置中启用并按确认模式执行。外部 API 客户端不会自动获得这些工具；需要由客户端声明并实际调用图片接口。'),
      examples: [example(t('对话模型看图请求'), code.vision), example(t('声明函数工具的请求体'), code.tools, 'json'), example(t('执行工具后回传结果的请求体'), code.toolResult, 'json')],
    },
    {
      id: 'responses', title: 'OpenAI Responses', path: 'POST /v1/responses',
      description: t('兼容 Responses 请求与事件格式，适合 Codex 等客户端；不是服务端会话存储。'),
      fields: [
        { name: 'model', type: 'string', description: t('使用与 Chat 相同的对话模型 id 和 Key 授权规则。') },
        { name: 'input', type: 'string | array', description: t('单轮可以是字符串；多轮使用消息与工具项数组，客户端应携带必要历史。文本块使用 input_text，图片块使用 input_image。') },
        { name: 'instructions', type: 'string', description: t('系统说明，对应 Chat 的 system 消息；需要时每次请求重新携带。') },
        { name: 'max_output_tokens / stream', type: 'integer / boolean', description: t('分别控制输出上限与是否流式。stream 默认 false；true 返回 Responses 命名事件。') },
        { name: 'tools / tool_choice', type: 'array / string | object', description: t('函数工具使用 Responses 的声明格式；工具结果以 function_call_output 携带相同 call_id。实际工具能力仍依赖模型与提供商。') },
        { name: 'reasoning.effort / text.format', type: 'string / object', description: t('映射为思考档位与结构化输出请求；支持范围以目标模型为准。') },
        { name: 'store', type: 'boolean', description: t('网关始终无状态，响应中的 store=false。即使请求 store=true，也不会保存可供后续检索的 Responses 对象。') },
      ],
      points: [
        t('不支持依赖服务端状态的 previous_response_id、conversation、托管提示词 prompt，以及 background=true；请由客户端保存并回传历史，不要调用未实现的 Responses 查询或删除接口。'),
        t('非流式返回 object=response、status、output 数组与 output_text。output 也可能包含 reasoning 或 function_call，不能只读取数组第一项。'),
        t('流式包含 response.created、response.output_text.delta 和最终 response.completed 等事件；错误可能以 response.failed 结束。输出预算不足时检查 incomplete 状态与 incomplete_details。'),
      ],
      examples: [example(t('Responses 请求'), code.responses), example(t('Responses 响应结构'), code.responsesResponse, 'json')],
    },
    {
      id: 'messages', title: 'Anthropic Messages', path: 'POST /v1/messages · POST /v1/messages/count_tokens',
      description: t('兼容 Anthropic 消息块与 SSE 事件，支持本地 Token 预算估算。'),
      fields: [
        { name: 'model', type: 'string', description: t('仍从当前网关目录选择对话模型，不要求模型名称一定以 claude 开头。具体能力以实际承载模型为准。') },
        { name: 'messages', type: 'array', description: t('必填。使用 user / assistant 角色；content 可为字符串或 text、image、tool_use、tool_result 等内容块。') },
        { name: 'system', type: 'string | array', description: t('系统说明是顶层字段，不要放进 messages 的 system 角色；支持文本或 text 块数组。') },
        { name: 'max_tokens', type: 'integer', description: t('Anthropic 标准要求显式填写正整数。网关对缺失或无效值补 8192，但上游模型的实际预算仍可能更低。') },
        { name: 'stream / stop_sequences', type: 'boolean / string[]', description: t('stream 默认 false；true 返回 Anthropic 事件流。stop_sequences 映射为停止序列，是否支持取决于上游。') },
        { name: 'tools / tool_choice', type: 'array / object', description: t('函数工具声明包含 name、description、input_schema；结果在后续 user 消息的 tool_result 块中携带对应 tool_use_id。') },
        { name: 'thinking / output_config', type: 'object', description: t('用于请求思考配置或档位，由网关映射到目标模型；不保证所有 Anthropic 原生特性在跨协议转发时完全等价。') },
      ],
      points: [
        t('非流式返回 type=message、content 块数组、stop_reason 与 usage。content 可能同时包含文本、思考与工具调用，应该按 type 遍历。'),
        t('流式以 message_start 开始，经 content_block_start / content_block_delta / content_block_stop，最终 message_delta 与 message_stop 收尾；不要按 Chat 的 [DONE] 规则解析。'),
        t('count_tokens 在本地估算，不调用上游 tokenizer；返回 input_tokens 和 estimated=true。数值用于上下文预算，不用于精确计费，尤其不应当作图片的真实 Token 用量。'),
      ],
      examples: [example(t('Messages 请求'), code.messages), example(t('Messages 响应结构'), code.messagesResponse, 'json'),
        example(t('估算输入 Token 数量'), code.countTokens), example(t('Token 估算响应'), code.countResponse, 'json')],
    },
    ...imageApiSections(code),
    ...toolApiSections(origin),
    {
      id: 'errors', title: t('错误、限额与兼容边界'),
      description: t('结合 HTTP 状态、错误内容与请求日志判断问题；不要盲目重试收费请求。'),
      paragraphs: [
        t('Chat、Responses 和图片接口通常返回 error 对象，其中 message 是可读说明，type 是错误类别，code 与 upstream_code 按情况附带。Anthropic 业务错误使用 type=error 的信封；鉴权中间件、无效 JSON 或网关层错误也可能返回通用 error 形态。'),
        t('本页只说明实际提供的接口，不等于完整复刻 OpenAI / Anthropic 服务。未列出的 Files、Batch、图片 variations、服务端会话存储等端点不要假定可用。'),
      ],
      fields: [
        { name: '400', type: 'HTTP status', description: t('请求参数、模型能力或协议形态不支持。图片相关 code 可能为 invalid_image_input、image_generation_not_supported 或 image_editing_not_supported；修改请求后再发。') },
        { name: '401', type: 'HTTP status', description: t('Key 缺失、无效、停用，或上游认证失败。网关未收到 Key 时返回 model_api_key_required，无效 Key 返回 invalid_api_key；请检查请求头与「网关 Key」页。') },
        { name: '404', type: 'HTTP status', description: t('模型不存在、已关闭或不在 Key 授权范围，常附 model_not_found；也可能是路径错误，或临时图片下载链接已过期。') },
        { name: '408 / 413', type: 'HTTP status', description: t('分别可能表示请求中断或请求过大。参考图过大时需缩小图片或减少数量，原样重试不会解决；中断的图片任务可能已经消耗额度。') },
        { name: '429', type: 'HTTP status', description: t('限流、额度不足或当前无可用账号窗口。能确定恢复时间时可能返回 Retry-After（秒）、reset_at（Unix 毫秒）与 reset_at_text；这些字段不是每次都有。') },
        { name: '500 / 502 / 503', type: 'HTTP status', description: t('可能是网关内部错误、上游异常、插件未接入、暂无可用账号、图片下载或编码失败。') },
      ],
      warning: t('遇到“图片请求可能已消耗额度，未自动重试”或“上游已生成”等提示，先核对请求日志与上游结果。客户端超时、断线或取消不代表上游撤销任务；自动重发可能再次扣费。'),
      points: [
        t('三种对话协议的流式错误需要检查事件内容与正常结束标记；HTTP 200 本身不足以证明成功。图片接口仅返回非流式 JSON。'),
        t('协议兼容不保证工具、思考、结构化输出、透明背景或每个尺寸都被所有提供商支持。先查看当前模型能力和插件版本，再核对请求日志的承载提供商与错误摘要。'),
        t('网关内置的安全重试与客户端 SDK 的重试是两层独立行为。图片客户端建议关闭自动重试；即使网关已停止重试，SDK 仍可能重新提交整条请求。'),
      ],
      examples: [example(t('通用错误结构示例'), code.error, 'json'), example(t('Anthropic 业务错误结构示例'), code.anthropicError, 'json')],
    },
  ]
}
