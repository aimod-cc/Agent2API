import {
  Badge, Button, Collapsible, CollapsibleContent, CollapsibleTrigger,
  Dialog, DialogBody, DialogClose, DialogContent, DialogDescription,
  DialogFooter, DialogHeader, DialogTitle, DialogTrigger,
  Sheet, SheetBody, SheetClose, SheetContent, SheetDescription,
  SheetFooter, SheetHeader, SheetTitle, SheetTrigger,
} from '../ui-kit/src'
import { licenseNotices, sources } from '../../licenses/ICONS_SOURCE.json'

type Translate = (key: string) => string

type LicenseEntry = {
  id: string
  name: string
  repository: string
  sourceRevision?: string
  licenseRevision?: string
  license: string
  copyright: string
  text: string
}

const LICENSES: LicenseEntry[] = [
  {
    id: 'magpie',
    name: 'Magpie',
    repository: sources.magpie.repository,
    sourceRevision: sources.magpie.revision,
    ...licenseNotices.magpie,
  },
  {
    id: 'lobe-icons',
    name: 'Lobe Icons',
    repository: sources.lobeIcons.repository,
    licenseRevision: sources.lobeIcons.licenseRevisionVerified,
    ...licenseNotices.lobeIcons,
  },
  {
    id: 'opendesign',
    name: 'OpenDesign',
    repository: sources.openDesign.repository,
    sourceRevision: sources.openDesign.revision,
    ...licenseNotices.openDesign,
  },
]

type LicenseViewerProps = {
  t: Translate
  openExternal: (url: string) => void | Promise<void>
  mobile?: boolean
}

function LicenseList({ t, openExternal }: LicenseViewerProps) {
  return (
    <>
      <p className='text-xs leading-relaxed text-subtle'>
        {t('目前展示仓库已登记的图标来源许可，不代表完整的第三方依赖清单。')}
      </p>
      <div className='flex min-w-0 flex-col gap-3'>
        {LICENSES.map(entry => (
          <Collapsible key={entry.id} className='overflow-hidden rounded-md border border-border bg-surface'>
            <h3 className='m-0'>
              <CollapsibleTrigger className='group h-auto w-full justify-start gap-3 rounded-none px-4 py-3 text-left whitespace-normal'>
                <span className='min-w-0 flex-1'>
                  <span className='flex flex-wrap items-center gap-2'>
                    <span className='text-[13px] font-semibold'>{entry.name}</span>
                    <Badge variant='outline' shape='tag'>{entry.license}</Badge>
                  </span>
                  <span className='mt-1 block text-[11px] font-normal break-words text-subtle'>{entry.copyright}</span>
                </span>
                <svg viewBox='0 0 24 24' className='size-4 shrink-0 text-subtle transition-transform group-data-panel-open:rotate-180' aria-hidden='true'>
                  <path d='m6 9 6 6 6-6' fill='none' stroke='currentColor' strokeWidth='1.5' strokeLinecap='round' strokeLinejoin='round' />
                </svg>
              </CollapsibleTrigger>
            </h3>
            <CollapsibleContent className='border-t border-hairline p-4'>
              <dl className='space-y-2 text-xs leading-relaxed'>
                <div>
                  <dt className='text-subtle'>{t('项目来源')}</dt>
                  <dd>
                    <a
                      href={entry.repository}
                      target='_blank'
                      rel='noopener noreferrer'
                      className='break-all text-primary underline underline-offset-2'
                      onClick={event => { event.preventDefault(); void openExternal(entry.repository) }}
                    >
                      {entry.repository}
                    </a>
                  </dd>
                </div>
                <div>
                  <dt className='text-subtle'>{entry.licenseRevision ? t('许可核对版本') : t('来源版本')}</dt>
                  <dd className='font-mono break-all'>{entry.licenseRevision || entry.sourceRevision}</dd>
                </div>
              </dl>
              {entry.id === 'lobe-icons' ? (
                <p className='mt-2 text-xs leading-relaxed text-subtle'>
                  {t('图标经 Magpie 引入，未单独记录 Lobe Icons 的图标版本；以上版本仅用于核对许可。')}
                </p>
              ) : null}
              <h4 className='mt-4 text-xs font-semibold'>{t('许可原文')}</h4>
              <pre
                tabIndex={0}
                role='region'
                aria-label={`${entry.name} ${t('许可原文')}`}
                lang='en'
                className='mt-2 max-h-[40vh] overflow-auto rounded-md border border-border bg-surface-inset p-3 font-mono text-[11px] leading-relaxed whitespace-pre-wrap break-words select-text'
              >{entry.text}</pre>
            </CollapsibleContent>
          </Collapsible>
        ))}
      </div>
      <p className='text-xs leading-relaxed text-subtle'>
        {t('本项目的 AGPL-3.0 许可不替代这些第三方许可。名称与标识的商标权归各自权利人所有。')}
      </p>
    </>
  )
}

export function ThirdPartyLicensesButton({ t, openExternal, mobile = false }: LicenseViewerProps) {
  const title = t('第三方开源许可')
  const description = t('许可原文已随软件内置，无需联网即可查看；打开项目来源链接需要联网。')

  if (mobile) {
    return (
      <Sheet>
        <SheetTrigger render={<Button variant='outline' size='touch' className='w-full' />}>
          {title}
        </SheetTrigger>
        <SheetContent showHandle={false}>
          <SheetHeader><SheetTitle>{title}</SheetTitle></SheetHeader>
          <SheetBody>
            <SheetDescription className='text-xs leading-relaxed text-subtle'>{description}</SheetDescription>
            <LicenseList t={t} openExternal={openExternal} />
          </SheetBody>
          <SheetFooter>
            <SheetClose render={<Button variant='outline' size='touch' className='w-full' />}>
              {t('关闭')}
            </SheetClose>
          </SheetFooter>
        </SheetContent>
      </Sheet>
    )
  }

  return (
    <Dialog>
      <DialogTrigger render={<Button variant='outline' size='xs' />}>
        {title}
      </DialogTrigger>
      <DialogContent className='w-[min(800px,calc(100vw-48px))]'>
        <DialogHeader className='shrink-0'><DialogTitle>{title}</DialogTitle></DialogHeader>
        <DialogBody className='min-h-0 [&>*]:shrink-0'>
          <DialogDescription className='text-xs leading-relaxed text-subtle'>{description}</DialogDescription>
          <LicenseList t={t} openExternal={openExternal} />
        </DialogBody>
        <DialogFooter className='shrink-0'>
          <DialogClose render={<Button variant='outline' />}>{t('关闭')}</DialogClose>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
