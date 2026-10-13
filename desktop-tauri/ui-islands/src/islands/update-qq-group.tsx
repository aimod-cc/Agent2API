import * as React from 'react'
import {
  Button,
  Dialog,
  DialogBody,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from '@ui'
import { QQ_GROUP, QQ_GROUP_QR } from './update-shared'
import { t } from '../i18n'

/**
 * 「加入 QQ 群」弹窗（更新日志卡片页脚那颗按钮打开）。
 *
 * ── 为什么是弹窗，而不是点了直接跳转 ──────────────────────────
 * 桌面版 QQ 能接的两条加群路子都坏在 QQ 自己那一层（`mqqapi://` 的群名片 QQ NT 不认；
 * `qm.qq.com/q/…` 那条分享链接只认 QQ 内置浏览器），这两种失败壳这边都看不见、给不出
 * 兜底 —— 详见 update-shared.ts 上 QQ_GROUP_QR 的注释。摆二维码没有这个问题：手机
 * QQ 扫一下就进，不依赖电脑上装了什么、也不依赖对方版本。
 *
 * 二维码是**打进包里**的静态图（`ui/assets/qq-group/<群号>.jpg`），与 provider 图标
 * 同一个口径：裸 <img>、相对路径、不处理加载失败 —— 内置资源拉不到就等于这次构建坏了，
 * 那种情况不该用一句文案糊过去。
 */
export function QqGroupDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  return (
    // 受控 open（面板按状态条件渲染本组件）：关窗一律由 onClose 收口，
    // Esc / 点遮罩 / 右上角 ✕ 由 Base UI Dialog 内建（与「更新设置」弹窗同一套）
    <Dialog open={open} onOpenChange={next => { if (!next) onClose() }}>
      {/* 窄弹窗：里面只有一张方图 + 一行群号，跟正文弹窗同宽会显得空 */}
      <DialogContent className='w-[min(320px,calc(100vw-48px))]'>
        <DialogHeader>
          <DialogTitle>{t('加入 QQ 群')}</DialogTitle>
        </DialogHeader>
        <DialogBody className='items-center gap-3 text-center'>
          <img className='qq-qr' src={QQ_GROUP_QR} alt={t('QQ 群二维码')} />
          <div className='qq-no'>
            <span className='k'>{t('群号')}</span>
            <code>{QQ_GROUP}</code>
            {/* data-copy 是 clipboard.js 的全局委托钩子（点一下复制，成功它自己弹轻提示） */}
            <Button type='button' size='sm' variant='ghost' data-copy={QQ_GROUP}>
              {t('复制')}
            </Button>
          </div>
          <p className='qq-hint'>{t('用手机 QQ 扫上面的二维码，或在 QQ 里搜索群号加入。')}</p>
        </DialogBody>
      </DialogContent>
    </Dialog>
  )
}
