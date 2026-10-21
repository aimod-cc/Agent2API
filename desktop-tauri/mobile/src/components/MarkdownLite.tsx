import type { ReactNode } from 'react'

/**
 * 极简 Markdown 渲染（只服务助手的回答气泡）。
 *
 * ── 为什么需要它 ──────────────────────────────────────────────
 * 模型输出天然带 Markdown：`**账号名**`、`` `模型ID` ``、`- 列表项`。直接当
 * 纯文本铺在气泡里，用户看到的是满屏星号和反引号 —— 尤其是账号名 / 模型名
 * 这类本来就该用等宽字显示的内容，反而被符号糊住了。
 *
 * ── 为什么不上一个 Markdown 库 ────────────────────────────────
 * 手机上只需要三样：粗体、行内代码、列表。引 markdown-it / react-markdown
 * 会给包体加几十 KB，还要处理 HTML 白名单（模型输出是不可信内容，直接
 * `dangerouslySetInnerHTML` 等于开一个 XSS 口子）。这里用 React 元素拼，
 * 天然转义，没有注入面。
 *
 * 代码块（```）只按普通段落显示 —— 手机上长代码块本来就该去桌面端看，
 * 为它做一套横向滚动 + 语法高亮不值。
 */
export function MarkdownLite({ text }: { text: string }) {
  const lines = String(text || '').split('\n')
  const blocks: ReactNode[] = []
  let list: string[] = []

  const flushList = (key: string) => {
    if (!list.length) return
    blocks.push(
      <ul key={key} style={{ margin: '4px 0', paddingLeft: 18 }}>
        {list.map((item, index) => (
          <li key={index} style={{ margin: '2px 0' }}>
            {inline(item)}
          </li>
        ))}
      </ul>,
    )
    list = []
  }

  lines.forEach((line, index) => {
    const trimmed = line.trim()
    // 代码围栏：整行丢掉（内容按普通行渲染，见文件头）
    if (trimmed.startsWith('```')) return
    const bullet = /^[-*+]\s+(.*)$/.exec(trimmed)
    const numbered = /^\d+[.)]\s+(.*)$/.exec(trimmed)
    if (bullet) {
      list.push(bullet[1])
      return
    }
    if (numbered) {
      list.push(numbered[1])
      return
    }
    flushList(`list-${index}`)
    if (!trimmed) {
      blocks.push(<div key={`gap-${index}`} style={{ height: 6 }} />)
      return
    }
    // 标题（## / ###）当成加粗段落：手机气泡里字号层级够多了
    const heading = /^#{1,6}\s+(.*)$/.exec(trimmed)
    blocks.push(
      <div key={`line-${index}`} style={{ margin: '1px 0', fontWeight: heading ? 600 : undefined }}>
        {inline(heading ? heading[1] : trimmed)}
      </div>,
    )
  })
  flushList('list-tail')

  return <>{blocks}</>
}

/** 行内：**粗体** 与 `代码`，其余原样 */
function inline(text: string): ReactNode[] {
  const parts = text.split(/(\*\*[^*]+\*\*|`[^`]+`)/g)
  return parts.filter(Boolean).map((part, index) => {
    if (part.startsWith('**') && part.endsWith('**') && part.length > 4) {
      return <strong key={index}>{part.slice(2, -2)}</strong>
    }
    if (part.startsWith('`') && part.endsWith('`') && part.length > 2) {
      return (
        <code
          key={index}
          style={{
            padding: '1px 5px',
            borderRadius: 5,
            background: 'var(--m-surface-2)',
            fontFamily: 'var(--m-mono)',
            fontSize: '0.94em',
            wordBreak: 'break-all',
          }}
        >
          {part.slice(1, -1)}
        </code>
      )
    }
    return <span key={index}>{part}</span>
  })
}
