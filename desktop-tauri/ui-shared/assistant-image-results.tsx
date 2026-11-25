import { memo, useEffect, useState } from 'react'
import { Alert, Button, Dialog, DialogContent, DialogHeader, DialogTitle, DialogBody, Spinner } from '@ui'
import type { ImageAsset } from './assistant-image-tools'
import type { Translate } from './assistant-read-types'
import { saveAssistantImageFile, type ImageFileSaver } from './assistant-image-save'

type Props = { images: ImageAsset[]; load: (id: string, signal?: AbortSignal) => Promise<Blob>; save?: ImageFileSaver; t: Translate }

const ImageResult = memo(function ImageResult({ image, load, saveFile, t }: { image: ImageAsset; load: Props['load']; saveFile: ImageFileSaver; t: Translate }) {
  const [url, setUrl] = useState('')
  const [error, setError] = useState('')
  const [open, setOpen] = useState(false)
  const [blob, setBlob] = useState<Blob | null>(null)
  const [saving, setSaving] = useState(false)
  useEffect(() => {
    const controller = new AbortController()
    let source = ''
    setError('')
    setUrl('')
    setBlob(null)
    void load(image.id, controller.signal).then(blob => {
      if (controller.signal.aborted) return
      source = URL.createObjectURL(blob)
      setUrl(source)
      setBlob(blob)
    }).catch(failure => {
      if (!controller.signal.aborted) setError(failure instanceof Error ? failure.message : String(failure))
    })
    return () => { controller.abort(); if (source) URL.revokeObjectURL(source) }
  }, [image.id, load])
  const save = async () => {
    if (!blob || saving) return
    setSaving(true)
    try { setError(''); await saveFile(image, blob) } catch (failure) {
      setError(t('保存图片失败：{message}', { message: failure instanceof Error ? failure.message : String(failure) }))
    } finally { setSaving(false) }
  }
  return (
    <figure style={{ margin: 0, minWidth: 0 }}>
      {error ? <Alert variant='destructive'>{error}</Alert> : url ? (
        <Button variant='ghost' onClick={() => setOpen(true)} aria-label={t('查看图片')}
          style={{ padding: 0, width: '100%', height: 'auto', overflow: 'hidden' }}>
          <img src={url} alt={t('生成的图片')} loading='lazy' decoding='async'
            style={{ display: 'block', width: '100%', maxHeight: 320, objectFit: 'contain' }} />
        </Button>
      ) : <Spinner aria-label={t('正在读取图片…')} />}
      <figcaption style={{ display: 'flex', gap: 8, alignItems: 'center', fontSize: 12, marginTop: 6 }}>
        <span>{image.width} × {image.height}</span>
        {url ? <Button variant='ghost' size='sm' disabled={saving} onClick={() => void save()}>{t('下载图片')}</Button> : null}
      </figcaption>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent>
          <DialogHeader><DialogTitle>{t('生成的图片')}</DialogTitle></DialogHeader>
          <DialogBody><img src={url} alt={t('生成的图片')} decoding='async' style={{ maxWidth: '100%', maxHeight: '75vh', objectFit: 'contain' }} /></DialogBody>
        </DialogContent>
      </Dialog>
    </figure>
  )
})

export const AssistantImageResults = memo(function AssistantImageResults({ images, load, save = saveAssistantImageFile, t }: Props) {
  return <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(min(220px, 100%), 1fr))', gap: 12, padding: 12 }}>
    {images.map(image => <ImageResult key={image.id} image={image} load={load} saveFile={save} t={t} />)}
  </div>
})
