import { t } from '../ui-kit/src/i18n'
import { apiExampleCode, type ApiExample } from './docs-api-examples'
import type { ApiSection } from './docs-api-content'

export function imageApiSections(code: ReturnType<typeof apiExampleCode>): ApiSection[] {
  const example = (label: string, value: string, language: ApiExample['language'] = 'bash'): ApiExample => ({ label, code: value, language })
  return [
    {
      id: 'image-generation', title: t('图片生成与共用参数'), path: 'POST /v1/images/generations',
      description: t('根据文字提示生成图片。请求使用 JSON，生成与编辑均只支持非流式响应。'),
      paragraphs: [
        t('目前小浣熊、Loomy、Antigravity、ModelScope、WorkBuddy 已接入图片生成；Loomy、Antigravity、ModelScope 与 WorkBuddy 同时接入参考图编辑，小浣熊暂不支持编辑。需要当前提供商插件包含对应能力，并且账号、模型和 Key 授权可用。'),
        t('小浣熊使用 raccoon-image；Loomy 使用目录中的 kind=image 模型；Antigravity 使用目录中的 gemini-*-image 模型；ModelScope 使用已确认支持的 Qwen-Image、Z-Image 或 Qwen-Image-Edit 型号；WorkBuddy 使用目录中带 text-to-image 或 image-to-image 标签的模型，例如 hunyuan-image-alpha 与 hunyuan-image-alpha-edit。实际 model 值以当前 /v1/models 为准，不要直接复制官方 API 的默认模型名。'),
        t('下表是网关接受的参数范围，不是所有提供商都支持的范围。生成与编辑共用这些字段，具体上游可能进一步拒绝数量、质量、风格、背景或尺寸。'),
      ],
      fields: [
        { name: 'prompt', type: 'string', description: t('必填且不能为空，最多 32000 个字符。描述主体、构图、风格和修改目标；当前不支持 mask，编辑范围也应在提示词中说明。') },
        { name: 'model', type: 'string', description: t('建议显式填写图片模型 id。省略时从当前 Key 可用目录中选择支持本次操作的图片模型，不使用对话默认模型；没有对应能力模型时返回 503。') },
        { name: 'n', type: 'integer', description: t('网关接受 1–10，默认 1；Antigravity 与 ModelScope 仅支持 n=1。上游返回数量不足时网关报错，不会为了凑数再次调用图片生成。') },
        { name: 'size', type: 'string', description: t('可省略，或使用 auto、WIDTHxHEIGHT，例如 1024x1024。网关校验边长 1–8192，提供商可能进一步限制宽高比与分辨率，不保证精确像素输出。') },
        { name: 'response_format', type: 'b64_json | url', description: t('默认 b64_json，返回可直接解码的图片 base64；url 返回上游签名链接或网关临时下载链接。不是图片文件编码。') },
        { name: 'output_format', type: 'png | jpeg | webp', description: t('网关可将实际图片转换为指定文件编码。省略时保留上游图片格式；需要固定扩展名时建议显式填写。JPEG 无法保留透明通道。') },
        { name: 'output_compression', type: 'integer · 0–100', description: t('仅在显式指定 output_format=jpeg 时支持，网关按 JPEG 质量参数编码。PNG、WebP 当前不支持此参数。') },
        { name: 'quality', type: 'string', description: t('可用值为 auto、standard、hd、low、medium、high、xhigh、max。小浣熊与 Antigravity 仅接受省略、auto 或 standard；其他值是否有效取决于 Loomy 的具体模型。') },
        { name: 'style', type: 'natural | vivid', description: t('小浣熊与 Antigravity 不支持此字段，请在 prompt 中描述风格；Loomy 转交上游，由模型决定是否支持。') },
        { name: 'background', type: 'auto | opaque | transparent', description: t('小浣熊与 Antigravity 仅接受省略、auto 或 opaque；不保证所有模型都能生成透明背景。文件转码不会自动抠图。') },
        { name: 'moderation', type: 'auto | low', description: t('小浣熊与 Antigravity 仅接受省略或 auto。该字段不保证上游降低审核强度，内容政策拒绝不会自动重试。') },
        { name: 'user', type: 'string', description: t('可选的调用方用户标识，不是网关鉴权 Key，也不能用于指定账号或绕过 Key 的授权范围。是否传递与处理取决于提供商。') },
        { name: 'stream / partial_images', type: 'false / 0', description: t('仅可省略或设为 stream=false、partial_images=0。当前不支持图片事件流、渐进图片或流式部分结果。未知参数会被拒绝，不会静默忽略。') },
      ],
      points: [
        t('ModelScope 使用官方异步任务接口，创建后只查询同一任务，不会为了等待结果重新提交。单次 n=1；size 可省略、设为 auto，或指定边长 64–2048 的 WIDTHxHEIGHT，具体型号可能进一步限制。'),
        t('ModelScope 不支持 style、mask 与 input_fidelity；quality 仅接受省略、auto 或 standard，background 仅接受省略、auto 或 opaque，moderation 仅接受省略或 auto。风格与修改目标请写入 prompt。'),
        t('Antigravity 支持宽高比 1:1、2:3、3:2、3:4、4:3、4:5、5:4、9:16、16:9、21:9，尺寸会映射到原生 1K / 2K / 4K 档。请求最长边 ≤1792 为 1K，1793–3072 为 2K，3073–4096 为 4K；超过 4096 会被该提供商拒绝。'),
        t('Antigravity 的 size=auto 生成使用 1:1、1K；参考图编辑使用 1K 且不强制正方形。模型版本可能有更严格的输出限制；宽高比需接近支持值，不能任意指定像素尺寸。'),
        t('WorkBuddy 走官方图片接口：size 省略或设为 auto 时按 1024x1024 请求；该通道不接受 quality、style、background 与 moderation，请把风格与质量要求写进 prompt。参考图以 data URL 内联在 JSON 请求里发送（不是 multipart），模型 id 由上游目录下发，换版本时以 /v1/models 为准。'),
      ],
      warning: t('生图可能消耗账号额度。先使用 n=1 和较小尺寸确认能力，内容政策拒绝或结果不确定时不要自动重复提交。'),
      examples: [example(t('JSON 图片生成请求'), code.generation)],
    },
    {
      id: 'image-editing', title: t('参考图编辑'), path: 'POST /v1/images/edits',
      description: t('上传一张或多张参考图，并用提示词描述保留内容与修改目标。'),
      paragraphs: [
        t('使用 supports_image_editing=true 的图片模型。该接口是参考图编辑、重绘或合成，不是精确遮罩修补；当前不支持 mask 与 input_fidelity，也不支持 file_id。'),
        t('上传本地文件推荐 multipart/form-data；已经有图片 URL 或 base64 时可发送 application/json。除参考图字段外，参数与图片生成相同。'),
      ],
      fields: [
        { name: 'image / image[]', type: 'multipart file', description: t('单图可用 image=@reference.png，多图重复提交 image[]。也兼容 images、images[] 与 image[0] 这类数字索引字段；普通参数不能重复提交。') },
        { name: 'images[].image_url', type: 'JSON string', description: t('JSON 的 images 是对象数组，每项只能包含 image_url。接受完整 HTTP(S) URL、data:image/...;base64,...，或先前响应返回的完整网关临时图片链接；不接受裸 base64、相对路径、file_id 或本地文件路径。') },
        { name: 'PNG / JPEG / WebP', type: 'image', description: t('只支持这三种可解码图片格式，校验实际文件内容而不是仅看扩展名；单张最多 10 MiB，边长最多 8192。') },
        { name: 'images', type: 'array · 1–16', description: t('网关最多接受 16 张参考图。Antigravity 的 gemini-2.5-* 模型最多 3 张，其他当前接入的 Antigravity 图片模型最多 14 张；ModelScope 接受 1–3 张参考图；Loomy 还可能受上游模型限制。') },
        { name: 'multipart / JSON', type: '16 MiB / 14 MiB', description: t('编辑请求上传总大小最多 16 MiB。参考图转为 data URL 后，内联图片与最终 JSON 请求还受 14 MiB 上限；base64 约增加三分之一体积，因此多张各小于 10 MiB 也可能超限。') },
      ],
      points: [
        t('HTTP(S) 参考图由网关服务器下载，不携带网关 Key 或提供商凭证。外链必须能被网关无额外请求头访问；本机、内网、带用户名密码或不安全重定向地址会被拒绝。受保护图片建议直接上传文件。'),
        t('先前网关返回的 /v1/images/files/{id} 完整链接可在未过期时作为 JSON 参考图。不要把 AI 助手内部的 assistant-image:ID 当成对外接口图片 URL。'),
        t('多图有顺序意义：在 prompt 中明确“第 1 张是主体，第 2 张是风格参考”等关系。编辑不保证逐像素保真，要求保留的主体、文字或布局应明确描述。'),
        t('curl -F 自动生成 multipart boundary。不要额外设置 Content-Type: application/json 或手写无 boundary 的 multipart/form-data；prompt 使用 --form-string 可避免特殊字符被当作文件上传。'),
      ],
      examples: [example(t('上传一张本地参考图'), code.editMultipart), example(t('上传多张参考图'), code.editMultiple), example(t('JSON 图片引用编辑请求'), code.editJson)],
    },
    {
      id: 'image-results', title: t('图片响应与下载'), path: 'GET /v1/images/files/{id}',
      description: t('生成与编辑返回同一结构；base64 适合直接保存，URL 适合短期获取。'),
      fields: [
        { name: 'created', type: 'integer', description: t('图片响应创建时间，Unix 秒。不要与错误限额恢复字段 reset_at 的毫秒单位混用。') },
        { name: 'data[].b64_json', type: 'string', description: t('response_format=b64_json 时返回不带 data URL 前缀的标准 base64。解码为二进制后保存；网页展示时自行加对应 MIME 类型前缀。') },
        { name: 'data[].url', type: 'string', description: t('response_format=url 时返回。可能是提供商签名链接，也可能是网关临时下载链接；应直接使用返回值，不要自行拼下载 id。') },
        { name: 'data[].revised_prompt', type: 'string', description: t('仅在上游返回修改后的提示词时保留，不保证每个模型都有。') },
        { name: 'output_format / usage / points_consumed', type: 'optional', description: t('可能附带实际输出编码、用量或点数。usage 与 points_consumed 取决于上游，缺失不代表免费；若未要求固定编码，不要默认所有图片都是 PNG。') },
      ],
      points: [
        t('网关临时图片仅保存在内存，生成后 1 小时过期，网关重启后立即失效。缓存总容量 128 MiB；缓存满时会返回错误并建议改用 b64_json。上游签名链接的有效期由提供商决定。'),
        t('网关临时下载不要求 Authorization，链接中的随机令牌就是访问凭据，任何拿到链接的人都能下载。请将链接视为敏感信息，重要结果及时保存，不要当作长期文件存储。'),
        t('下载返回原始图片二进制及 image/png、image/jpeg 或 image/webp Content-Type，不是 JSON。不存在或过期返回 404，响应使用 private, no-store 与 nosniff。'),
        t('网关需要下载或转换输出时，单张图片最多 32 MiB、边长最多 8192，并使用有界解码。请求 output_format 会按实际图片转码，而不是只修改文件扩展名。'),
      ],
      warning: t('response_format=url 不保证永久地址；网关临时链接与 AI 助手会话中的图片存储是两套机制，不能混用接口或有效期。'),
      examples: [example(t('base64 图片响应结构'), code.imageBase64, 'json'), example(t('网关临时 URL 响应结构'), code.imageUrl, 'json'), example(t('下载并保存网关临时图片'), code.download)],
    },
    {
      id: 'image-sdk', title: t('OpenAI Python SDK 图片示例'),
      description: t('使用同一客户端生成与编辑，并将返回的 base64 保存为图片文件。'),
      paragraphs: [
        t('安装并使用支持 Images 参数的 OpenAI Python SDK。先按接入章节设置 GATEWAY_KEY 与 IMAGE_MODEL_ID，编辑前确认该模型支持参考图，并准备本地 reference.png。'),
        t('示例显式设置 max_retries=0，避免 SDK 对超时或 5xx 自动重新提交收费任务；timeout=300.0 仅是客户端等待示例，不会改变网关或上游的超时配置。'),
      ],
      warning: t('不要把 base64 图片塞回对话文本或日志，也不要将“请求超时”理解为“没有扣费”。如需连续编辑，保存结果文件后再作为下一次 image 上传。'),
      examples: [example(t('Python：生成、编辑并保存图片'), code.imageSdk, 'python')],
    },
  ]
}
