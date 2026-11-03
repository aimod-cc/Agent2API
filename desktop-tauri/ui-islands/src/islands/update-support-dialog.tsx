import * as React from 'react'
import {
  Card,
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  SegmentedControl,
  useMediaQuery,
} from '@ui'
import { t } from '../i18n'

type SupportMethod = 'wechat' | 'alipay'

export function SupportDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const narrow = useMediaQuery('(max-width: 639px)')
  const [method, setMethod] = React.useState<SupportMethod>('wechat')
  const methods = [
    {
      value: 'wechat' as const,
      label: t('微信赞赏'),
      src: 'assets/support/wechat-reward.png',
      alt: t('微信赞赏码'),
      pixels: 710,
    },
    {
      value: 'alipay' as const,
      label: t('支付宝'),
      src: 'assets/support/alipay-receive.png',
      alt: t('支付宝收款码'),
      pixels: 1620,
    },
  ]

  return (
    <Dialog open={open} onOpenChange={next => { if (!next) onClose() }}>
      <DialogContent className='w-[min(680px,calc(100vw-48px))]'>
        <DialogHeader>
          <DialogTitle>{t('支持作者')}</DialogTitle>
        </DialogHeader>
        <DialogBody>
          <DialogDescription className='support-intro'>
            {t('Agent2API 一直在更新，是因为有人在背后持续投入。如果它帮到了你，欢迎请作者喝杯咖啡。')}
          </DialogDescription>
          {narrow ? (
            <SegmentedControl
              className='self-center'
              aria-label={t('赞赏方式')}
              options={methods}
              value={method}
              onValueChange={setMethod}
            />
          ) : null}
          <div className='support-codes' data-narrow={narrow}>
            {methods.filter(item => !narrow || item.value === method).map(item => (
              <Card key={item.value} render={<figure />} size='sm' className='support-code-card'>
                <figcaption className='support-code-title'>{item.label}</figcaption>
                <img
                  className='support-code-image'
                  src={item.src}
                  alt={item.alt}
                  width={item.pixels}
                  height={item.pixels}
                />
              </Card>
            ))}
          </div>
          <p className='support-note'>
            {t('赞赏完全自愿，不解锁任何功能，也不影响正常使用。')}
          </p>
        </DialogBody>
      </DialogContent>
    </Dialog>
  )
}
