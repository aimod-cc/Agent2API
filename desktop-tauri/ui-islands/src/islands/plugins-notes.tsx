import * as React from 'react'
import { Badge, Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle } from '@ui'
import { handleExternalClick, markdownHtml, openExternal } from './update-shared'
import { updatePageUrl } from '../../../ui-shared/updates'
import { t } from '../i18n'
import { HtmlContent } from '@ui'

export type PluginNotes = {
  name: string
  version: string
  notes?: string | null
  page?: string | null
  prerelease?: boolean
}

export function PluginNotesDialog({ value, onClose }: { value: PluginNotes | null; onClose: () => void }) {
  const notes = value?.notes?.trim() || ''
  const html = notes ? markdownHtml(notes) : ''
  return <Dialog open={value !== null} onOpenChange={next => { if (!next) onClose() }}>
    <DialogContent>
      <DialogHeader><DialogTitle>{t('更新日志')}</DialogTitle></DialogHeader>
      <DialogBody>
        <div className='flex flex-wrap items-center gap-2 text-[13px]'>
          <strong>{value?.name}</strong><span className='text-subtle'>v{value?.version}</span>
          {value?.prerelease ? <Badge variant='warning'>{t('beta 版')}</Badge> : null}
        </div>
        <div className='max-h-[46vh] overflow-y-auto overscroll-contain pr-1.5' onClick={handleExternalClick}>
          {html ? <HtmlContent className='md-body' html={html} />
            : notes ? <div className='whitespace-pre-wrap text-[12.5px] leading-relaxed text-subtle'>{notes}</div>
              : <p className='text-[12px] text-muted-foreground'>{t('这个版本没有填写发布说明。')}</p>}
        </div>
      </DialogBody>
      {value?.page ? <DialogFooter>
        <Button variant='outline' size='sm' onClick={() => void openExternal(updatePageUrl(value.page))}>{t('查看发布页面')}</Button>
      </DialogFooter> : null}
    </DialogContent>
  </Dialog>
}
