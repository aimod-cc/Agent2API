import { t } from '../ui-kit/src/i18n'
import { buildWebSearchApiDoc } from './tools-api-doc'
import type { ApiSection } from './docs-api-content'

/**
 * 工具服务的 REST 接口文档。
 *
 * 「工具服务」页只保留 Skill / MCP 两种可管理协议；网络搜索的 REST API 是固定
 * 接口，不参与服务开关，文档放在这里。正文沿用 `tools-api-doc` 的中英两套
 * （其它语言回落英文）：接口文档长、随上游演进，不适合塞进五语词典。
 */
export function toolApiSections(origin: string): ApiSection[] {
  const locale = (window as unknown as { wbI18n?: { locale(): string } }).wbI18n?.locale() || 'zh-Hans'
  return [{
    id: 'tools-api',
    title: t('工具服务 API'),
    path: 'POST /api/web-search/search · POST /api/web-search/fetch',
    description: t('网络搜索与网页抓取的 REST 接口，使用网关 Key 鉴权；不属于 /v1 对话协议。'),
    paragraphs: [
      t('工具服务 API 是网关对外的固定接口，路径以 /api/web-search 开头；不属于 /v1 对话协议，也不是管理面板的管理接口，鉴权同样使用网关 Key。'),
    ],
    markdown: buildWebSearchApiDoc(locale, origin),
  }]
}
