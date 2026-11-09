import { Card, Progress } from '@ui'
import type { ScriptOperation, ScriptPackage, ScriptRecommendation } from './plugins'
import { PluginArtwork, type PluginTranslate } from './plugins-store'
import { reinstallSource } from './plugins-script-display'

export function scriptOperationMatches(operation: ScriptOperation, item: ScriptPackage | ScriptRecommendation): boolean {
  const target = operation.target
  if (!operation.active || !target) return false
  if (target.id && 'id' in item) return target.id === item.id
  const source = 'id' in item ? reinstallSource(item) : item.source || item.name
  if (target.source) return target.source.toLowerCase() === source.toLowerCase()
  return target.name === item.name
}

export function ScriptOperationProgress({ operation, t }: { operation: ScriptOperation; t: PluginTranslate }) {
  if (!operation.active) return null
  const stage = operation.stage === 'runtime' ? t('正在下载 Bun 运行时')
    : operation.stage === 'package' ? t('正在下载脚本插件')
    : operation.stage === 'waiting' ? t('等待脚本插件的在途请求和登录完成')
    : operation.stage === 'restoring' ? t('正在恢复原有脚本插件')
    : t('正在加载脚本插件')
  return <div className='space-y-2 border-t border-hairline pt-3' aria-live='polite'>
    <div className='flex flex-wrap items-center justify-between gap-2 text-xs'>
      <span className='text-subtle'>{stage}</span>
      {(operation.received || 0) > 0 ? <span className='tabular-nums text-muted-foreground'>
        {((operation.received || 0) / 1024 / 1024).toFixed(1)} MiB
      </span> : null}
    </div>
    <Progress value={null} aria-label={stage} />
  </div>
}

export function ScriptOperationCard({ operation, t }: { operation: ScriptOperation; t: PluginTranslate }) {
  const name = operation.target?.name || operation.target?.source || t('Magpie 插件')
  return <Card render={<article />} data-plugin-card='' aria-label={name} className='flex h-full flex-col gap-3 p-4'>
    <div className='flex items-start gap-3.5'>
      <PluginArtwork kind='script' />
      <div className='min-w-0 flex-1'>
        <h3 className='m-0 break-words text-[14px] font-semibold leading-5'>{name}</h3>
        {operation.target?.source && operation.target.source !== name ? <p className='m-0 mt-1.5 break-all text-xs text-muted-foreground'>
          {operation.target.source}
        </p> : null}
      </div>
    </div>
    <ScriptOperationProgress operation={operation} t={t} />
  </Card>
}
