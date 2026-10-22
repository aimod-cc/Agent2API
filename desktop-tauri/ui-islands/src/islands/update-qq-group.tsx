import * as React from 'react'
import {
  Badge,
  Dialog,
  DialogBody,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from '@ui'
import { COMMUNITY_LINKS, QQ_GROUPS, handleExternalClick, shared } from './update-shared'
import { t } from '../i18n'

/**
 * 「加入交流群」弹窗（更新日志卡片页脚那颗按钮打开）。
 *
 * ── 布局：四张卡片，不是四行文字 ──────────────────────────
 * 平铺成「标签 | 值 | 操作」四行读起来像一张表格，也不告诉人哪个入口更该点。
 * 改成卡片：每张卡左边一枚品牌标（QQ / Telegram / Discord 官方造型，见
 * icons.js 那三枚）、右边主标题 + 副标题，整张卡可点。QQ 两张卡是「复制群号」
 * （群号本身就是要拿走的文本），另两张是「打开链接」（外链交给系统浏览器）。
 *
 * 群 1 的「即将满员」放在卡片右上角而不是标题旁：它是这张卡的**状态**，
 * 挂在标题上会把标题挤成两截，扫视时也看不出这是「整张卡」的属性。
 *
 * ── 为什么不再摆二维码 ────────────────────────────────────
 * 二维码是打进包里的静态图，换群 / 换码必须跟着发一次版（见 update-shared.ts
 * 上 QQ_GROUPS 的注释）。现在群号本来就会变（群 1 满员、以后可能再开群），
 * 只留群号 + 复制就没有这个负担，改一个常量即可。
 */
export function QqGroupDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  /** 取一枚内联 SVG（icons.js 由 index.html 在岛之前加载，正常总是就位） */
  function iconHtml(name: string, size: number): string {
    return shared().wbIcons?.icon?.(name, size) || ''
  }

  return (
    // 受控 open（面板按状态条件渲染本组件）：关窗一律由 onClose 收口，
    // Esc / 点遮罩 / 右上角 ✕ 由 Base UI Dialog 内建（与「更新设置」弹窗同一套）
    <Dialog open={open} onOpenChange={next => { if (!next) onClose() }}>
      <DialogContent className='w-[min(440px,calc(100vw-48px))]'>
        <DialogHeader>
          <DialogTitle>{t('加入交流群')}</DialogTitle>
        </DialogHeader>
        {/* 外链委托挂在 DialogBody 上：卡片是渲染出来的，逐个绑 onClick 既费事又容易漏 */}
        <DialogBody className='gap-2.5' onClick={handleExternalClick}>
          <p className='qq-intro'>{t('遇到问题或有使用心得，欢迎到群里交流。')}</p>

          <div className='qq-cards'>
            {/* QQ 群：两张卡。群 1 挂「即将满员」，并把「优先加群 2」写进副标题 ——
                提示要落在人真要动手的那张卡旁边，而不是只写在顶上那段话里。

                **为什么用原生 <button> 而不是组件库 Button**：Button 的变体类带
                `border-transparent bg-transparent`（ghost）且组件库工具类带 !important，
                会把 .qq-card 的描边与底色整条压掉 —— 两张卡粘成一片、和下面两张
                <a> 卡也对不齐（就是「样式崩了」那个样子）。这里要的是「一张可点的
                卡片」而不是「一个按钮」，所以只借原生元素 + 自己的类。 */}
            {QQ_GROUPS.map((group, index) => (
              <button
                key={group.no}
                type='button'
                className='qq-card'
                data-copy={group.no}
                title={t('复制群号 {group}', { group: group.no })}
              >
                <span
                  className='qq-card-logo'
                  data-brand='qq'
                  dangerouslySetInnerHTML={{ __html: iconHtml('qq', 24) }}
                />
                <span className='qq-card-body'>
                  <span className='qq-card-title'>
                    {index === 0 ? t('QQ 群 1') : t('QQ 群 2')}
                    <code>{group.no}</code>
                  </span>
                  <span className='qq-card-sub'>
                    {group.nearlyFull ? t('即将满员，请优先加群 2') : t('点击复制群号')}
                  </span>
                </span>
                {group.nearlyFull ? (
                  <Badge shape='tag' variant='warning' className='qq-card-flag'>{t('即将满员')}</Badge>
                ) : null}
              </button>
            ))}

            {/* Telegram / Discord：整张卡是外链。标签直接写字面量（不取 link 上的字段
                再包一层）—— 扫描器只认字符串字面量首参，变量形式提不出键，会一直漏翻 */}
            {COMMUNITY_LINKS.map(link => (
              <a
                key={link.id}
                className='qq-card'
                href={link.url}
                data-external={link.url}
                target='_blank'
                rel='noopener'
                title={t('在浏览器中打开')}
              >
                <span
                  className='qq-card-logo'
                  data-brand={link.id}
                  dangerouslySetInnerHTML={{ __html: iconHtml(link.id, 24) }}
                />
                <span className='qq-card-body'>
                  <span className='qq-card-title'>
                    {link.id === 'telegram' ? t('Telegram 群') : t('Discord 群')}
                  </span>
                  <span className='qq-card-sub'>{t('点击在浏览器中打开')}</span>
                </span>
              </a>
            ))}
          </div>

          <p className='qq-foot'>{t('在 QQ 里搜索群号即可加入。')}</p>
        </DialogBody>
      </DialogContent>
    </Dialog>
  )
}
