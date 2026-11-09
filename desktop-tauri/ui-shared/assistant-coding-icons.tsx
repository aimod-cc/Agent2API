type CodingIconName = 'model' | 'effort' | 'speed' | 'chevron' | 'back' | 'send' | 'stop' | 'check' | 'usage' | 'folder' | 'plus' | 'chat'

export function CodingIcon({ name, className = 'size-4' }: { name: CodingIconName; className?: string }) {
  return <svg viewBox='0 0 24 24' className={className} fill='none' stroke='currentColor' strokeWidth='1.8' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
    {name === 'folder' ? <path d='M3 7V5h6l2 2h10v12H3V7Z' />
      : name === 'plus' ? <path d='M12 5v14M5 12h14' />
      : name === 'chat' ? <path d='M4 4h16v12H9l-5 4V4Z' />
      : name === 'model' ? <><path d='m12 4 8 4-8 4-8-4 8-4Z' /><path d='m4 12 8 4 8-4M4 16l8 4 8-4' /></>
      : name === 'effort' ? <><path d='M4 7h16M4 17h16' /><circle cx='9' cy='7' r='2' /><circle cx='15' cy='17' r='2' /></>
      : name === 'speed' ? <path d='m13 3-8 11h6l-1 7 9-12h-6l1-6Z' />
      : name === 'chevron' ? <path d='m7 9 5 5 5-5' />
      : name === 'back' ? <path d='m14 6-6 6 6 6' />
      : name === 'send' ? <path d='M12 20V5m-6 6 6-6 6 6' />
      : name === 'stop' ? <rect x='7' y='7' width='10' height='10' rx='2' fill='currentColor' stroke='none' />
      : name === 'check' ? <path d='m5 12 4 4L19 6' />
      : <><circle cx='12' cy='12' r='8' /><path d='M12 4v8h8' /></>}
  </svg>
}
