import { formatTokens } from '../report-charts'

export function CompactValue({ value }: { value: unknown }) {
  const text = formatTokens(value)
  const parts = text.match(/^(.*?)([万萬亿億만억kMB])$/u)
  return parts ? <>{parts[1]}<small>{parts[2]}</small></> : <>{text}</>
}
