import * as React from 'react'
import { Alert, Badge, Button, Card, CardContent, CardHeader, CardTitle, Empty, Spinner, confirmDialog } from '@ui'
import { t } from '../ui-kit/src/i18n'
import type { ImageToolApi, ToolImageFile, ToolImageJob, ToolImageJobs } from './tools-image-types'

const STATUS: Record<ToolImageJob['status'], string> = {
  running: t('生成中'), succeeded: t('已完成'), failed: t('失败'), uncertain: t('结果不确定'), expired: t('已过期或清理'),
}

export function ToolImageHistory({ api }: { api: ImageToolApi }) {
  const [data, setData] = React.useState<ToolImageJobs | null>(null)
  const [error, setError] = React.useState('')
  const [busy, setBusy] = React.useState('')
  const [loading, setLoading] = React.useState(true)
  const load = React.useCallback(async () => {
    try { setData(await api.jobs()); setError('') }
    catch (failure) { setError(t(failure instanceof Error ? failure.message : String(failure))) }
    finally { setLoading(false) }
  }, [api])
  React.useEffect(() => {
    void load()
    const timer = setInterval(() => { if (document.visibilityState === 'visible') void load() }, 10_000)
    return () => clearInterval(timer)
  }, [load])

  const remove = async (job: ToolImageJob) => {
    if (!await confirmDialog(t('清理这次生成的临时图片？已保存到本机的文件不会被删除，原提交编号也不会重新生图。'), { destructive: true })) return
    setBusy(job.job_id)
    try { await api.remove(job.job_id); await load() }
    catch (failure) { setError(t(failure instanceof Error ? failure.message : String(failure))) }
    finally { setBusy('') }
  }
  const download = async (file: ToolImageFile) => {
    setBusy(file.id)
    try { await api.download(file) }
    catch (failure) { setError(t(failure instanceof Error ? failure.message : String(failure))) }
    finally { setBusy('') }
  }
  const mib = (value: number) => (value / (1024 * 1024)).toFixed(1)
  return <Card>
    <CardHeader>
      <CardTitle>{t('最近生成记录')}</CardTitle>
      <Button className='ml-auto' size='sm' variant='outline' onClick={() => void load()}>{t('刷新')}</Button>
    </CardHeader>
    <CardContent className='flex flex-col gap-3'>
      {data ? <p className='text-xs text-subtle'>{t('图片占用 {used} MiB，生成中预留 {reserved} MiB。', { used: mib(data.storage.used_bytes), reserved: mib(data.storage.reserved_bytes) })}</p> : null}
      {error ? <Alert variant='destructive'>{error}</Alert> : null}
      {loading ? <Spinner /> : null}
      {!loading && !data?.jobs.length ? <Empty>{t('暂无生图记录，连接 MCP 后即可让 Agent 生成图片。')}</Empty> : null}
      {data?.jobs.map(job => <div key={job.job_id} className='flex flex-col gap-2 rounded-md border border-border p-3'>
        <div className='flex flex-wrap items-center gap-2'>
          <Badge variant={job.status === 'succeeded' ? 'success' : 'outline'}>{STATUS[job.status] || job.status}</Badge>
          <span className='min-w-0 break-all text-sm font-medium'>{job.model}</span>
          <span className='text-xs text-subtle'>{new Date(job.created_at).toLocaleString()}</span>
          {job.status !== 'running' && job.status !== 'expired' ? <Button className='ml-auto' size='sm' variant='ghost' disabled={Boolean(busy)} onClick={() => void remove(job)}>{t('清理')}</Button> : null}
        </div>
        <div className='break-all text-xs text-subtle'>{job.provider} · {job.request_id}{job.actual_account_name ? ` · ${job.actual_account_name}` : ''}</div>
        {job.status === 'succeeded' ? <p className='text-xs text-subtle'>{t('积分')}: {job.credit ?? job.points_consumed ?? '—'}</p> : null}
        {job.error ? <p className='whitespace-pre-wrap text-xs text-warning'>{t(job.error)}</p> : null}
        {job.images?.length ? <div className='flex flex-wrap gap-2'>{job.images.map((file, index) => <Button key={file.id} size='sm' variant='outline' disabled={Boolean(busy)} onClick={() => void download(file)}>
          {busy === file.id ? <Spinner /> : null}{t('下载图片 {index}', { index: index + 1 })} · {file.width} × {file.height}
        </Button>)}</div> : null}
        {job.status === 'succeeded' ? <p className='text-xs text-subtle'>{t('保留至 {time}', { time: new Date(job.expires_at).toLocaleString() })}</p> : null}
      </div>)}
    </CardContent>
  </Card>
}

export function downloadToolImageBlob(file: ToolImageFile, blob: Blob): void {
  const extension = file.mime === 'image/jpeg' ? 'jpg' : file.mime === 'image/webp' ? 'webp' : 'png'
  const url = URL.createObjectURL(blob)
  const anchor = document.createElement('a')
  anchor.href = url
  anchor.download = `image-${file.id.slice(0, 12)}.${extension}`
  document.body.append(anchor)
  anchor.click()
  anchor.remove()
  setTimeout(() => URL.revokeObjectURL(url), 60_000)
}
