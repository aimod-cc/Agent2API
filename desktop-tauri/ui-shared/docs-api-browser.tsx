import { useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { Button, Empty, EmptyDescription, EmptyTitle, HtmlContent, InputGroup, InputGroupAddon, InputGroupButton,
  InputGroupInput, NavItem, NavList, SidebarGroupLabel } from '@ui'
import { t } from '../ui-kit/src/i18n'
import { apiReferenceSections, type ApiSection } from './docs-api-content'
import { ApiReference } from './docs-api-reference'
import { documentationGroups, documentationMatches, type DocumentationEntry } from './docs-api-navigation'
import { ProviderDocsTable } from './provider-docs-view'
import type { PluginProviderDocumentation } from './provider-docs'

type DocumentationBrowserProps = {
  origin: string
  packs: PluginProviderDocumentation[]
  addresses: ReactNode
  notice?: ReactNode
  onCopy: (text: string, label: string) => void | Promise<void>
}

/** 整份 Markdown 文档（工具服务 API）。正文不走五语词典，复制按钮与示例同一套。 */
function MarkdownArticle({ section, onCopy }: { section: ApiSection; onCopy: DocumentationBrowserProps['onCopy'] }) {
  const markdown = section.markdown || ''
  const html = (window as unknown as { wbMarkdown?: { render?: (text: string) => string } }).wbMarkdown?.render?.(markdown) || ''
  return <div className='docs-reference'>
    {section.paragraphs?.length ? <div className='docs-prose'>
      {section.paragraphs.map(text => <p key={text}>{text}</p>)}
    </div> : null}
    <div className='docs-block'>
      <div className='mb-3 flex justify-end'>
        <Button variant='outline' size='sm' onClick={() => void onCopy(markdown, t('工具服务 API 文档'))}>{t('复制文档')}</Button>
      </div>
      {html ? <HtmlContent className='tool-api-doc' html={html} />
        : <pre className='tool-api-doc max-h-[640px] overflow-auto whitespace-pre-wrap rounded-md border border-border p-4 text-xs'>{markdown}</pre>}
    </div>
  </div>
}

export function DocumentationBrowser({ origin, packs, addresses, notice, onCopy }: DocumentationBrowserProps) {
  const sections = useMemo(() => apiReferenceSections(origin), [origin])
  const groups = useMemo(() => documentationGroups(sections, packs), [sections, packs])
  const entries = groups.flatMap(group => group.entries)
  const [selected, setSelected] = useState('start')
  const [query, setQuery] = useState('')
  const [menuOpen, setMenuOpen] = useState(false)
  const articleRef = useRef<HTMLElement>(null)
  const headingRef = useRef<HTMLHeadingElement>(null)
  const searchRef = useRef<HTMLInputElement>(null)
  const focusArticle = useRef(false)
  const active = entries.find(entry => entry.id === selected) || entries.find(entry => entry.id === 'start') || entries[0]
  const section = sections.find(item => item.id === active.id)
  const currentIndex = entries.findIndex(entry => entry.id === active.id)
  const previous = entries[currentIndex - 1]
  const next = entries[currentIndex + 1]
  const searching = query.trim().length > 0
  const filteredGroups = groups.map(group => ({
    ...group, entries: group.entries.filter(entry => documentationMatches(entry, query)),
  })).filter(group => group.entries.length > 0)
  const results = filteredGroups.flatMap(group => group.entries)

  useLayoutEffect(() => {
    if (selected !== active.id) setSelected(active.id)
    if (articleRef.current) articleRef.current.scrollTop = 0
    if (focusArticle.current) {
      focusArticle.current = false
      headingRef.current?.focus({ preventScroll: true })
    }
  }, [active.id, selected])

  function navigate(id: string) {
    focusArticle.current = true
    setSelected(id)
    setMenuOpen(false)
    setQuery('')
    if (id === active.id) {
      if (articleRef.current) articleRef.current.scrollTop = 0
      headingRef.current?.focus({ preventScroll: true })
      focusArticle.current = false
    }
  }

  function clearSearch() {
    setQuery('')
    searchRef.current?.focus()
  }

  function pageLink(entry: DocumentationEntry, direction: 'previous' | 'next') {
    return <Button variant='ghost' className={`docs-page-link docs-page-link-${direction}`} onClick={() => navigate(entry.id)}>
      <span className='docs-page-link-label'>
        {direction === 'previous' ? t('上一章') : t('下一章')}
      </span>
      <span className='docs-page-link-title'>{entry.title}</span>
      <span className='docs-page-link-arrow' aria-hidden='true'>{direction === 'previous' ? '←' : '→'}</span>
    </Button>
  }

  return <div className='docs-browser' data-menu-open={menuOpen ? 'true' : undefined}>
    <div className='docs-layout'>
      <aside className='docs-sidebar' aria-label={t('文档目录')}>
        <div className='docs-sidebar-head'>
          <div className='docs-sidebar-title'>{t('接口文档')}</div>
          <div className='docs-sidebar-caption'>{t('接入指南与接口参考')}</div>
          <InputGroup className='docs-search'>
            <InputGroupInput ref={searchRef} type='search' value={query} autoComplete='off'
              placeholder={t('搜索文档、参数或接口…')} aria-label={t('搜索文档')}
              onChange={event => { setQuery(event.currentTarget.value); setMenuOpen(true) }}
              onKeyDown={event => {
                if (event.key === 'Escape') {
                  event.preventDefault()
                  clearSearch()
                } else if (event.key === 'Enter' && !event.nativeEvent.isComposing && searching && results[0]) {
                  event.preventDefault()
                  navigate(results[0].id)
                }
              }} />
            <InputGroupAddon aria-hidden='true'>⌕</InputGroupAddon>
            {query ? <InputGroupAddon align='inline-end'>
              <InputGroupButton size='icon-xs' title={t('清除搜索')} aria-label={t('清除搜索')} onClick={clearSearch}>×</InputGroupButton>
            </InputGroupAddon> : null}
          </InputGroup>
          <Button variant='ghost' className='docs-menu-toggle' aria-expanded={menuOpen || searching}
            aria-controls='docs-navigation' onClick={() => { if (searching) setQuery(''); setMenuOpen(!menuOpen) }}>
            <span className='docs-menu-current'>{active.title}</span>
            <span className='docs-menu-action'>{menuOpen || searching ? t('收起目录') : t('展开目录')}</span>
            <span aria-hidden='true'>{menuOpen || searching ? '−' : '+'}</span>
          </Button>
        </div>
        <nav id='docs-navigation' className='docs-navigation' aria-label={t('接口参考目录')}>
          {searching ? <div className='docs-search-summary' role='status'>
            {t('找到 {count} 个章节', { count: results.length })}
          </div> : null}
          {filteredGroups.map(group => <section key={group.id} className='docs-nav-group' aria-labelledby={`docs-group-${group.id}`}>
            <SidebarGroupLabel id={`docs-group-${group.id}`} className='docs-group-head'>
              <svg className='docs-group-icon' viewBox='0 0 24 24' fill='none' stroke='currentColor'
                strokeWidth={1.7} strokeLinecap='round' strokeLinejoin='round' aria-hidden='true' focusable='false'>
                <path d='M3 7V6a2 2 0 0 1 2-2h4l3 3h7a2 2 0 0 1 2 2v1M3 7v11a2 2 0 0 0 2 2h14l3-10H6L3 20' />
              </svg>
              <span>{group.title}</span>
            </SidebarGroupLabel>
            <NavList className='docs-nav-chapters'>
              {group.entries.map(entry => <NavItem key={entry.id} active={active.id === entry.id} title={entry.title}
                className='docs-nav-item' aria-controls='docs-article' onClick={() => navigate(entry.id)}
                icon={<svg className='docs-chapter-icon' viewBox='0 0 24 24' fill='none' stroke='currentColor'
                  strokeWidth={1.7} strokeLinecap='round' strokeLinejoin='round' aria-hidden='true' focusable='false'>
                  <path d='M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9l-6-6Z' />
                  <path d='M14 3v6h6M8 13h8M8 17h5' />
                </svg>}>
                {entry.title}
              </NavItem>)}
            </NavList>
          </section>)}
          {!results.length ? <Empty className='docs-search-empty'>
            <EmptyTitle>{t('没有找到匹配的文档')}</EmptyTitle>
            <EmptyDescription>{t('试试接口路径、参数名或其他关键词。')}</EmptyDescription>
            <Button variant='outline' size='sm' onClick={clearSearch}>{t('清除搜索')}</Button>
          </Empty> : null}
        </nav>
      </aside>
      <article id='docs-article' ref={articleRef} className='docs-article' aria-labelledby='docs-article-title'>
        <div className='docs-article-inner'>
          {notice}
          <header className='docs-article-head'>
            <div className='docs-breadcrumb'>
              <span>{t('接口文档')}</span><span aria-hidden='true'>/</span><span>{active.group}</span>
            </div>
            <h2 id='docs-article-title' ref={headingRef} tabIndex={-1}>{active.title}</h2>
            <p>{active.description}</p>
            {active.path ? <code className='docs-article-path'>{active.path}</code> : null}
          </header>
          {/* 地址节点由 port-panel 更新，切换章节时必须保留，不能随正文卸载。 */}
          <div hidden={active.id !== 'addresses'}>{addresses}</div>
          {section?.markdown
            ? <MarkdownArticle section={section} onCopy={onCopy} />
            : section ? <ApiReference key={section.id} section={section} onCopy={onCopy} /> : null}
          {active.id === 'providers' ? <ProviderDocsTable packs={packs} /> : null}
          <nav className='docs-pagination' aria-label={t('章节导航')}>
            <div>{previous ? pageLink(previous, 'previous') : null}</div>
            <div>{next ? pageLink(next, 'next') : null}</div>
          </nav>
        </div>
      </article>
    </div>
  </div>
}
