import type { ReactNode } from 'react'
import { Badge, Button, Card, CardContent } from '../ui-kit/src'
import './sponsor-cards.css'

type SponsorCardProps = {
  t: (key: string) => string
  title: string
  logo: ReactNode
  description: string
  url: string
  linkLabel: string
  openExternal: (url: string) => void | Promise<void>
  tone?: 'gold' | 'blue' | 'neutral'
  tags?: string[]
  note?: string
  action?: { label: string; onClick: () => void }
  className?: string
}

export function SponsorCard({
  t, title, logo, description, url, linkLabel, openExternal,
  tone = 'neutral', tags = [], note, action, className = '',
}: SponsorCardProps) {
  return <Card render={<section aria-label={title} />}
    className={`sponsor-card sponsor-card--${tone} ${className}`}>
    <CardContent className='sponsor-card-content'>
      <div className='sponsor-card-heading'>
        <span className='sponsor-card-logo' aria-hidden='true'>{logo}</span>
        <h3 className='sponsor-card-title'>{title}</h3>
        <Badge variant='outline' shape='tag' className='sponsor-card-label'>{t('赞助商')}</Badge>
      </div>
      <p className='sponsor-card-description'>{description}</p>
      {tags.length ? <div className='sponsor-card-tags'>
        {tags.map(tag => <span key={tag}>{tag}</span>)}
      </div> : null}
      {note ? <p className='sponsor-card-note'>{note}</p> : null}
      <div className='sponsor-card-actions'>
        <a href={url} target='_blank' rel='noopener noreferrer sponsored' referrerPolicy='no-referrer'
          className='sponsor-card-link'
          onClick={event => { event.preventDefault(); void openExternal(url) }}>
          {linkLabel}<span aria-hidden='true'>↗</span>
        </a>
        {action ? <Button size='sm' variant='outline' onClick={action.onClick}>
          {action.label}
        </Button> : null}
      </div>
    </CardContent>
  </Card>
}
