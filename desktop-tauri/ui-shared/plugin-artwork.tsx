import * as React from 'react'
import { Avatar, AvatarFallback, AvatarImage } from '@ui'
import {
  artworkCacheKey, cachedPluginArtwork, EMPTY_PLUGIN_ARTWORK, hasCachedPluginArtwork, scheduleInspectPluginArtwork,
} from './plugin-artwork-appearance'
import './plugin-artwork.css'

export type PluginArtworkKind = 'web' | 'providers' | 'script'

export const PluginArtwork = React.memo(function PluginArtwork({ src, kind = 'script', fallback }: {
  src?: string | null
  kind?: PluginArtworkKind
  fallback?: React.ReactNode
}) {
  const source = src || ''
  const sourceKey = artworkCacheKey(source)
  const [detected, setDetected] = React.useState(() => ({ source, appearance: cachedPluginArtwork(source) }))
  const appearance = detected.source === source ? detected.appearance : cachedPluginArtwork(source)
  const currentSourceRef = React.useRef(source)
  currentSourceRef.current = source
  const cancelInspectRef = React.useRef<(() => void) | null>(null)

  React.useEffect(() => () => {
    cancelInspectRef.current?.()
    cancelInspectRef.current = null
  }, [source])

  const applyAppearance = React.useCallback((targetSource: string, next: typeof EMPTY_PLUGIN_ARTWORK) => {
    if (currentSourceRef.current !== targetSource) return
    setDetected(current => current.source === targetSource && current.appearance.native === next.native
      && current.appearance.tone === next.tone && current.appearance.hue === next.hue
      ? current : { source: targetSource, appearance: next })
  }, [])

  const inspect = React.useCallback((image: HTMLImageElement) => {
    if (!source) return
    if (hasCachedPluginArtwork(source)) {
      applyAppearance(source, cachedPluginArtwork(source))
      return
    }
    cancelInspectRef.current?.()
    cancelInspectRef.current = scheduleInspectPluginArtwork(image, source, next => {
      applyAppearance(source, next)
    })
  }, [source, applyAppearance])

  const imageRef = React.useCallback((image: HTMLImageElement | null) => {
    if (image?.complete && image.naturalWidth) inspect(image)
  }, [inspect])

  return <Avatar className='plugin-artwork size-12 rounded-md' aria-hidden='true'
    data-native={appearance.native ? '' : undefined} data-tone={appearance.tone}
    style={appearance.hue === undefined ? undefined : { '--plugin-artwork-hue': appearance.hue } as React.CSSProperties}>
    {source ? <AvatarImage key={sourceKey} src={source} alt='' referrerPolicy='no-referrer' keepMounted
      loading='lazy' decoding='async'
      className={`m-auto object-contain ${appearance.native ? 'size-full' : 'size-9'}`} ref={imageRef}
      onLoad={event => inspect(event.currentTarget)}
      onError={() => {
        cancelInspectRef.current?.()
        cancelInspectRef.current = null
        setDetected({ source, appearance: EMPTY_PLUGIN_ARTWORK })
      }} /> : null}
    <AvatarFallback className='bg-transparent text-muted-foreground'>
      {fallback ?? <svg viewBox='0 0 24 24' className='size-6' fill='none' stroke='currentColor' strokeWidth='1.8'
        strokeLinecap='round' strokeLinejoin='round'>
        {kind === 'web' ? <>
          <circle cx='12' cy='12' r='8.4' />
          <path d='M3.6 12h16.8M12 3.6c2.2 2.4 3.3 5.2 3.3 8.4S14.2 18 12 20.4C9.8 18 8.7 15.2 8.7 12S9.8 6 12 3.6Z' />
        </> : kind === 'providers' ? <path d='m12 3.4 2.4 5.4 5.4 2.4-5.4 2.4L12 19l-2.4-5.4L4.2 11.2l5.4-2.4Z' />
          : <>
            <rect x='4.5' y='4.5' width='15' height='15' rx='4' />
            <path d='m10 8-4 4 4 4m4-8 4 4-4 4' />
          </>}
      </svg>}
    </AvatarFallback>
  </Avatar>
})
