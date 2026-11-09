import * as React from 'react'
import { Button, Checkbox, Input, Label, Switch, Table, TableBody, TableCell, TableHead, TableHeader, TableRow, Textarea } from '@ui'
import { parseProxyList, proxyAddress, type ProxyImportDraft, type ProxyImportIssue, type ProxyImportReport } from './proxy-import'

type Translate = (key: string, values?: Record<string, string | number>) => string
type TestResult = { success?: boolean; error?: string; ip?: string; durationMs?: unknown }
type BatchRow = { key: number; draft: ProxyImportDraft; selected: boolean; testing?: boolean; result?: TestResult; error?: string }
const PAGE_SIZE = 20

export type ProxyBatchHandle = { hasDraft: () => boolean }

export const ProxyBatchEditor = React.forwardRef<ProxyBatchHandle, {
  t: Translate
  locked: boolean
  onBusyChange: (busy: boolean) => void
  save: (items: ProxyImportDraft[]) => Promise<ProxyImportReport>
  test: (item: ProxyImportDraft) => Promise<TestResult>
  onNotice: (message: string, failed?: boolean) => void
}>(({ t, locked, onBusyChange, save, test, onNotice }, ref) => {
  const [input, setInput] = React.useState('')
  const [rows, setRows] = React.useState<BatchRow[]>([])
  const [issues, setIssues] = React.useState<ProxyImportIssue[]>([])
  const [nonDirect, setNonDirect] = React.useState(false)
  const [busy, setBusy] = React.useState<'save' | 'test' | ''>('')
  const [error, setError] = React.useState('')
  const [summary, setSummary] = React.useState('')
  const [page, setPage] = React.useState(1)
  const [progress, setProgress] = React.useState({ done: 0, total: 0 })
  const nextKey = React.useRef(0)
  const busyRef = React.useRef(false)
  const alive = React.useRef(true)
  const current = React.useRef({ rows, input })
  current.current = { rows, input }
  React.useEffect(() => { alive.current = true; return () => { alive.current = false } }, [])
  React.useImperativeHandle(ref, () => ({ hasDraft: () => Boolean(current.current.rows.length || current.current.input.trim()) }), [])
  const disabled = locked || Boolean(busy)
  const selected = rows.filter(row => row.selected)
  const pages = Math.max(1, Math.ceil(rows.length / PAGE_SIZE))
  const currentPage = Math.min(page, pages)
  const pageRows = rows.slice((currentPage - 1) * PAGE_SIZE, currentPage * PAGE_SIZE)

  function parse(): void {
    if (disabled) return
    const result = parseProxyList(input, nonDirect, rows.map(row => row.draft), t)
    setRows(previous => [...previous, ...result.items.map(draft => ({ key: ++nextKey.current, draft, selected: true }))])
    setInput(result.remaining)
    setIssues(result.issues)
    setError('')
    setPage(1)
    onNotice(t('已识别 {n} 条代理，跳过 {duplicates} 条重复地址', { n: result.items.length, duplicates: result.duplicates }), result.issues.length > 0)
  }

  function update(key: number, change: Partial<BatchRow>): void {
    setRows(previous => previous.map(row => row.key === key ? { ...row, ...change } : row))
  }

  async function act(mode: 'save' | 'test'): Promise<void> {
    if (disabled || busyRef.current || !selected.length) return
    if (mode === 'save' && selected.some(row => !row.draft.name.trim())) { setError(t('请填写代理名称')); return }
    busyRef.current = true
    setBusy(mode)
    onBusyChange(true)
    setError('')
    const targets = selected.map(row => ({ ...row, draft: { ...row.draft, name: row.draft.name.trim() } }))
    setProgress({ done: 0, total: targets.length })
    try {
      if (mode === 'save') {
        const result = await save(targets.map(row => row.draft))
        if (!alive.current) return
        const finished = new Set([...result.added.map(item => item.index), ...result.duplicates].map(index => targets[index]?.key))
        const errors = new Map(result.errors.map(item => [targets[item.index]?.key, item.error]))
        setRows(previous => previous.filter(row => !finished.has(row.key)).map(row => ({ ...row, error: errors.get(row.key) })))
        const message = t('批量添加完成：新增 {added} 条，重复 {duplicates} 条，失败 {failed} 条', {
          added: result.added.length, duplicates: result.duplicates.length, failed: result.errors.length,
        })
        setSummary(message)
        onNotice(message, result.errors.length > 0)
      } else {
        let ok = 0
        for (const [index, row] of targets.entries()) {
          if (!alive.current) break
          update(row.key, { testing: true, result: undefined, error: undefined })
          let result: TestResult
          try { result = await test(row.draft) }
          catch (failure) { result = { success: false, error: failure instanceof Error ? failure.message : String(failure) } }
          if (!alive.current) break
          if (result.success) ok += 1
          update(row.key, { testing: false, result })
          setProgress({ done: index + 1, total: targets.length })
        }
        if (alive.current) onNotice(t('批量测试完成：{ok} 个可用、{failed} 个失败（共 {total} 个）', {
          ok, failed: targets.length - ok, total: targets.length,
        }), ok !== targets.length)
      }
    } catch (failure) {
      if (alive.current) setError(failure instanceof Error ? failure.message : String(failure))
    } finally {
      busyRef.current = false
      if (alive.current) { setBusy(''); onBusyChange(false) }
    }
  }

  return <section className='flex min-w-0 flex-col gap-3' aria-label={t('批量添加')}>
    <div>
      <h3 className='text-sm font-semibold'>{t('批量添加')}</h3>
      <p className='mt-1 text-xs leading-relaxed text-subtle'>{t('每行一条，解析后可修改名称、设置非直连并测试，最后保存勾选的代理。')}</p>
    </div>
    <Textarea value={input} rows={5} disabled={disabled} spellCheck={false} autoComplete='off'
      className='resize-y font-mono text-xs' aria-label={t('代理列表')}
      onChange={event => { setInput(event.currentTarget.value); setIssues([]) }}
      placeholder={'192.0.2.1:8080\n192.0.2.1:8080:user:password\nsocks5://user:password@proxy.example:1080\nhttp://[2001:db8::1]:8080{' + t('名称') + '}'} />
    <Label className='flex items-center gap-2 text-xs font-normal'>
      <Checkbox checked={nonDirect} disabled={disabled} onCheckedChange={setNonDirect} />{t('新解析的代理默认使用非直连')}
    </Label>
    <p className='text-xs leading-relaxed text-subtle'>{t('非直连会先经过网关所在设备的系统代理，再连接到此代理；系统代理不可用时不会改走直连。')}</p>
    <div className='flex flex-wrap items-center gap-2'>
      <Button size='sm' variant='outline' disabled={disabled || !input.trim()} onClick={parse}>{t('解析')}</Button>
      <Button size='sm' variant='ghost' disabled={disabled || !rows.length} onClick={() => { setRows([]); setPage(1); setError('') }}>{t('清空已识别')}</Button>
      <span className='ml-auto text-xs text-subtle'>{t('已识别 {n} 条 · 已勾选 {selected} 条', { n: rows.length, selected: selected.length })}</span>
    </div>
    {issues.length ? <div className='max-h-24 overflow-auto text-xs text-destructive' role='alert'>
      <p>{t('以下行未能识别，已留在输入框中；修正后可再次解析。')}</p>
      {issues.map(issue => <p key={issue.line}>{t('第 {line} 行：{error}', { line: issue.line, error: issue.error })}</p>)}
    </div> : null}
    <Table containerClassName='max-h-[320px] rounded-md border border-border' className='min-w-[560px]'>
      <TableHeader><TableRow>
        <TableHead className='w-8'><Checkbox checked={rows.length > 0 && selected.length === rows.length}
          indeterminate={selected.length > 0 && selected.length < rows.length} disabled={disabled || !rows.length}
          aria-label={t('全选所有代理')} onCheckedChange={value => setRows(previous => previous.map(row => ({ ...row, selected: value })))} /></TableHead>
        <TableHead>{t('代理名称')}</TableHead><TableHead>{t('地址')}</TableHead>
        <TableHead className='text-center'>{t('非直连')}</TableHead><TableHead>{t('测试')}</TableHead><TableHead>{t('操作')}</TableHead>
      </TableRow></TableHeader>
      <TableBody>{pageRows.length ? pageRows.map(row => <TableRow key={row.key}>
        <TableCell><Checkbox checked={row.selected} disabled={disabled} aria-label={t('勾选「{name}」', { name: row.draft.name })}
          onCheckedChange={value => update(row.key, { selected: value })} /></TableCell>
        <TableCell><Input maxLength={60} value={row.draft.name} disabled={disabled} className='h-7 min-w-24 text-xs'
          aria-label={t('代理名称')} onChange={event => update(row.key, { draft: { ...row.draft, name: event.currentTarget.value }, error: undefined })} /></TableCell>
        <TableCell><span className='block max-w-44 truncate font-mono text-[11px]' title={proxyAddress(row.draft)}>{proxyAddress(row.draft)}</span>
          {row.draft.username || row.draft.password ? <span className='text-[10px] text-subtle'>{t('已配置认证')}</span> : null}</TableCell>
        <TableCell className='text-center'><Switch checked={row.draft.nonDirect} disabled={disabled} aria-label={t('非直连代理')}
          onCheckedChange={value => update(row.key, { draft: { ...row.draft, nonDirect: value }, result: undefined, error: undefined })} /></TableCell>
        <TableCell><span className={`block max-w-36 truncate text-xs ${row.error || row.result?.success === false ? 'text-destructive' : 'text-subtle'}`}
          title={row.error || row.result?.error || row.result?.ip}>
          {row.error || (row.testing ? t('测试中…') : row.result ? row.result.success ? row.result.ip || t('可用') : row.result.error || t('失败') : t('未测试'))}
        </span></TableCell>
        <TableCell><Button size='xs' variant='ghost' disabled={disabled} onClick={() => setRows(previous => previous.filter(item => item.key !== row.key))}>{t('移除')}</Button></TableCell>
      </TableRow>) : <TableRow><TableCell colSpan={6} className='py-8 text-center text-xs text-subtle'>{t('粘贴代理列表后点击「解析」')}</TableCell></TableRow>}</TableBody>
    </Table>
    {pages > 1 ? <div className='flex items-center justify-end gap-2 text-xs'>
      <Button size='xs' variant='outline' disabled={currentPage === 1} onClick={() => setPage(currentPage - 1)}>{t('上一页')}</Button>
      <span>{currentPage} / {pages}</span>
      <Button size='xs' variant='outline' disabled={currentPage === pages} onClick={() => setPage(currentPage + 1)}>{t('下一页')}</Button>
    </div> : null}
    {summary ? <p className='text-xs text-subtle' role='status'>{summary}</p> : null}
    {error ? <p className='text-xs text-destructive' role='alert'>{error}</p> : null}
    <div className='mt-auto flex flex-wrap justify-end gap-2 pt-1'>
      <Button variant='outline' disabled={disabled || !selected.length} onClick={() => void act('test')}>
        {busy === 'test' ? t('测试中 {done}/{total}', progress) : t('批量测试（{n}）', { n: selected.length })}
      </Button>
      <Button disabled={disabled || !selected.length} onClick={() => void act('save')}>
        {busy === 'save' ? t('保存中…') : t('批量添加（{n}）', { n: selected.length })}
      </Button>
    </div>
  </section>
})
