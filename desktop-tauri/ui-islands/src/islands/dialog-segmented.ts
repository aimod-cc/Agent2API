/**
 * 弹窗档位的分段控件外观：项高 28px、左右各 12px、字号 12.5px —— 比页面里那一档
 * （24px 项高）大一号，因为弹窗里的控件整体都大一号。
 *
 * 添加账号弹窗与账号设置弹窗共用这一份：两处都是「弹窗里的档位」，尺寸必须一样。
 * 谁要再放一个分段控件进弹窗，也引这里，别再自己写一串任意变体。
 *
 * 用任意变体而不是给组件库加一档尺寸：组件库的 SegmentedControl 只定义页面档，
 * 弹窗这一档属于排版约定，放在调用方比扩组件库的 API 更轻。
 */
export const DIALOG_SEG_CLASS =
  'max-w-full flex-wrap [&_[data-slot=segmented-item]]:h-7 [&_[data-slot=segmented-item]]:px-3 [&_[data-slot=segmented-item]]:text-[12.5px]'
