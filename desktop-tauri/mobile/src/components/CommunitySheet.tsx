import { Sheet, SheetContent, SheetHeader, SheetBody, SheetTitle, SheetFooter , Button } from '@ui'
import { Icon } from './Icon'
import { Badge, copyWithToast } from './ui'
import { COMMUNITY_LINKS, QQ_GROUPS, openExternal } from '../lib/community'
import { t } from '../i18n'

/**
 * 「社区与支持」面板（设置页底部那一行点开）。
 *
 * ── 为什么是抽屉而不是二级页 ──────────────────────────────────
 * 里面只有社区入口卡片（QQ 群 + Telegram），没有需要记住的状态、
 * 也没有下一步。做成二级页会多一层返回栏、多一条浏览器历史，用户点完就退。
 *
 * ── QQ 卡片是「复制群号」，另一张是「打开链接」───────────────
 * 群号本身就是用户要拿走的东西（去 QQ 里搜），所以点一下复制；Telegram
 * 是 https 链接，交给系统浏览器。两类卡片的副标题把这个差别写出来，
 * 不用用户猜。
 *
 * ── 为什么不再摆二维码 ────────────────────────────────────────
 * 与桌面端同一理由：二维码是打进包里的静态图，换群 / 换码得跟着发版。
 */
export function CommunitySheet({
  open,
  onClose,
}: {
  open: boolean
  onClose: () => void
}) {
  return (
    <Sheet open={open} onOpenChange={next => { if (!next) onClose() }}>
      <SheetContent>
        <SheetHeader>
          <SheetTitle>{t('社区与支持')}</SheetTitle>
        </SheetHeader>
        <SheetBody>
          <p style={{ margin: 0, fontSize: 12.5, lineHeight: 1.65, color: 'var(--m-text-2)' }}>
            {t('遇到问题或有使用心得，欢迎到群里交流。')}
          </p>

          {QQ_GROUPS.map((group, index) => (
            <Button size='default' variant='ghost'
              key={group.no}
              type='button'
              className='m-row h-auto min-h-[52px] w-full justify-start rounded-none border-0 px-3.5 py-3 text-left'
              onClick={() => void copyWithToast(group.no, t('QQ 群号'))}
              style={{
                borderRadius: 'var(--m-r-md)',
                border: '1px solid var(--m-line)',
                background: 'var(--m-surface)',
                minHeight: 58,
              }}
            >
              <span
                className='m-logo'
                style={{ background: '#1EBAFC', borderColor: 'transparent', color: '#fff' }}
              >
                <BrandGlyph id='qq' />
              </span>
              <div className='m-grow'>
                <div className='m-t1' style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                  {t('QQ 群 {n}', { n: index + 1 })}
                  <span className='m-mono' style={{ fontSize: 11.5, color: 'var(--m-text-3)' }}>
                    {group.no}
                  </span>
                </div>
                <div className='m-t2 whitespace-normal'>
                  {group.nearlyFull
                    ? t('即将满员，请优先加3群、4群')
                    : t('点击复制群号')}
                </div>
              </div>
              {group.nearlyFull ? (
                <Badge tone='warn'>{t('即将满员')}</Badge>
              ) : (
                <span style={{ color: 'var(--m-primary)', display: 'flex', flex: 'none' }}>
                  <Icon name='copy' style={{ width: 16, height: 16 }} />
                </span>
              )}
            </Button>
          ))}

          {COMMUNITY_LINKS.map(link => (
            <Button size='default' variant='ghost'
              key={link.id}
              type='button'
              className='m-row h-auto min-h-[52px] w-full justify-start rounded-none border-0 px-3.5 py-3 text-left'
              onClick={() => openExternal(link.url)}
              style={{
                borderRadius: 'var(--m-r-md)',
                border: '1px solid var(--m-line)',
                background: 'var(--m-surface)',
                minHeight: 58,
              }}
            >
              <span
                className='m-logo'
                style={{
                  background: '#26A5E4',
                  borderColor: 'transparent',
                  color: '#fff',
                }}
              >
                <BrandGlyph id={link.id} />
              </span>
              <div className='m-grow'>
                <div className='m-t1'>{t('Telegram 群')}</div>
                <div className='m-t2'>{t('点击在浏览器中打开')}</div>
              </div>
              <span className='m-chev'>
                <Icon name='link' />
              </span>
            </Button>
          ))}

          <p style={{ margin: '2px 0 0', fontSize: 11.5, lineHeight: 1.7, color: 'var(--m-text-3)' }}>
            {t('在 QQ 里搜索群号即可加入。')}
          </p>
        </SheetBody>
        <SheetFooter>
          <Button size='touch' variant='outline' type='button' className='m-btn m-ghost w-full' style={{ flex: 1 }} onClick={onClose}>
            {t('关闭')}
          </Button>
        </SheetFooter>
      </SheetContent>
    </Sheet>
  )
}

/**
 * 品牌标（QQ / Telegram）。
 *
 * 路径取自 simple-icons（与桌面端 `ui/icons.js` 的 brand 集同一来源、同一份
 * 路径），填充色写在 path 上 —— 这两个品牌有官方色，跟着文字色走反而不像。
 */
function BrandGlyph({ id }: { id: 'qq' | 'telegram' }) {
  const path =
    id === 'qq'
      ? 'M21.395 15.035a40 40 0 0 0-.803-2.264l-1.079-2.695c.001-.032.014-.562.014-.836C19.526 4.632 17.351 0 12 0S4.474 4.632 4.474 9.241c0 .274.013.804.014.836l-1.08 2.695a39 39 0 0 0-.802 2.264c-1.021 3.283-.69 4.643-.438 4.673.54.065 2.103-2.472 2.103-2.472 0 1.469.756 3.387 2.394 4.771-.612.188-1.363.479-1.845.835-.434.32-.379.646-.301.778.343.578 5.883.369 7.482.189 1.6.18 7.14.389 7.483-.189.078-.132.132-.458-.301-.778-.483-.356-1.233-.646-1.846-.836 1.637-1.384 2.393-3.302 2.393-4.771 0 0 1.563 2.537 2.103 2.472.251-.03.581-1.39-.438-4.673'
      : 'M11.944 0A12 12 0 0 0 0 12a12 12 0 0 0 12 12 12 12 0 0 0 12-12A12 12 0 0 0 12 0a12 12 0 0 0-.056 0zm4.962 7.224c.1-.002.321.023.465.14a.506.506 0 0 1 .171.325c.016.093.036.306.02.472-.18 1.898-.962 6.502-1.36 8.627-.168.9-.499 1.201-.82 1.23-.696.065-1.225-.46-1.9-.902-1.056-.693-1.653-1.124-2.678-1.8-1.185-.78-.417-1.21.258-1.91.177-.184 3.247-2.977 3.307-3.23.007-.032.014-.15-.056-.212s-.174-.041-.249-.024c-.106.024-1.793 1.14-5.061 3.345-.48.33-.913.49-1.302.48-.428-.008-1.252-.241-1.865-.44-.752-.245-1.349-.374-1.297-.789.027-.216.325-.437.893-.663 3.498-1.524 5.83-2.529 6.998-3.014 3.332-1.386 4.025-1.627 4.476-1.635z'
  return (
    <svg viewBox='0 0 24 24' width={18} height={18} aria-hidden='true'>
      <path d={path} fill='currentColor' />
    </svg>
  )
}
