import type { PieLabelRenderProps } from 'recharts'

/** 折线数据点标签之间至少要留出的横向距离。实际步长由调用方按容器宽度算，
 *  一行读数（「97.1%」「2.8 亿」）约 34px 宽，46px 留出可辨认的间隙。 */
export const LINE_LABEL_GAP = 46

const LABEL_OFFSET = 10

export function renderDonutLabel({ cx, cy, midAngle, innerRadius, outerRadius, percent, fill }: PieLabelRenderProps) {
  if (percent == null || percent < 0.03 || midAngle == null) return null
  const radius = innerRadius + (outerRadius - innerRadius) / 2
  const angle = -midAngle * Math.PI / 180
  return <text x={cx + radius * Math.cos(angle)} y={cy + radius * Math.sin(angle)}
    className='donut-label' data-other={fill === 'var(--chart-other)' ? '' : undefined}
    textAnchor='middle' dominantBaseline='central'>
    {`${Math.round(percent * 100)}%`}
  </text>
}

/** 折线点上的读数标签（口径与 OmniProxy 的报表一致）：
 *  · 命中率标在点下方、Token 标在点上方，两条线的读数分开摆放，不叠在一起；
 *  · 首末两个点向内锚定，末点标签不会探进右侧纵轴的刻度区；
 *  · 文字带描边（描边取卡片底色），压到折线上时把线「挖开」，密集处也读得清。
 *
 *  `stride` 是抽稀步长：一屏 24 个小时点，窄窗口逐点标注会连成一片数字，
 *  所以按容器宽度取步长，只有首末两个有数据的点无论如何都标。
 *  `flipUnder` 是「贴底改标上方」的阈值：命中率掉到 0 时点贴在绘图区底边，
 *  标签画在下方会落进横轴刻度带里，这时改标到点上方。
 */
export function renderPointLabel(props: PointLabelProps, options: PointLabelOptions) {
  const index = props.index ?? 0
  const value = props.value
  if (value == null || !(index === options.first || index === options.last || index % options.stride === 0)) return null
  const box = (props.viewBox ?? {}) as { x?: unknown; y?: unknown }
  const x = Number(box.x ?? props.x)
  const y = Number(box.y ?? props.y)
  const numeric = Number(value)
  if (!Number.isFinite(numeric) || !Number.isFinite(x) || !Number.isFinite(y)) return null
  const above = options.place === 'above' || (options.flipUnder != null && numeric <= options.flipUnder)
  return <text x={x} y={above ? y - LABEL_OFFSET : y + LABEL_OFFSET}
    className='report-line-label' style={{ fill: options.fill }}
    textAnchor={index === options.first ? 'start' : index === options.last ? 'end' : 'middle'}
    dominantBaseline={above ? 'auto' : 'hanging'}>
    {options.format(numeric)}
  </text>
}

/** recharts 的 LabelList 逐点回调入参：viewBox 与根上的 x/y 都是这个点的坐标。 */
type PointLabelProps = { viewBox?: unknown; x?: unknown; y?: unknown; value?: unknown; index?: number }

type PointLabelOptions = {
  first: number
  last: number
  stride: number
  fill: string
  place: 'above' | 'below'
  flipUnder?: number
  format: (value: number) => string
}
