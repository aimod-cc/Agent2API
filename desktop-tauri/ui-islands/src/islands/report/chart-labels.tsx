import type { PieLabelRenderProps } from 'recharts'

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
