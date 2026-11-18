import { useAdvertisements, advertisementCopy } from '../../../ui-shared/advertisements'
import { RemoteSponsorCard } from '../../../ui-shared/remote-sponsor-card'
import { shared, type PresetRecord } from './add-account-bridge'
import { openExternal } from './update-shared'
import { t } from '../i18n'

export function PresetSponsorCard({ preset, onPick }: { preset: PresetRecord; onPick?: () => void }) {
  const ad = useAdvertisements().find(ad => ad.category === 'provider' && ad.presetKey === preset.key)
  return ad ? <RemoteSponsorCard ad={ad} t={t} openExternal={openExternal}
    className={onPick ? 'sponsor-provider-card' : ''} action={onPick ? { label: t('使用此 API'), onClick: onPick } : undefined} /> : null
}

export function ProviderSponsorCards({ onPick, keyword = '' }: { onPick: (id: string) => void; keyword?: string }) {
  const items = useAdvertisements().filter(ad => ad.category === 'provider' && (!ad.presetKey
    || !shared().wbPresetProviders?.presetOf?.(ad.presetKey)) && (!keyword
    || `${advertisementCopy(ad).title} ${advertisementCopy(ad).description}`.toLowerCase().includes(keyword)))
  return <>{items.map(ad => <RemoteSponsorCard key={ad.id} ad={ad} t={t} openExternal={openExternal} className='sponsor-provider-card'
    action={!ad.presetKey ? { label: t('新建自定义提供商'), onClick: () => onPick('__new__') } : undefined} />)}</>
}
