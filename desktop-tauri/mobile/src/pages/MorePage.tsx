import { PageHead, SectionHead, IconButton } from '../components/ui'
import { Icon, type IconName } from '../components/Icon'
import { store, useStore, type ThemeMode } from '../state/store'
import { apiBase, baseLabel } from '../lib/servers'
import { isRateLimited } from '../lib/format'
import { usePullToRefresh, PullIndicator } from '../components/PullToRefresh'
import { t } from '../i18n'
import { Card, Button, SegmentedControl } from '@ui'

/**
 * 更多。
 *
 * 桌面端 13 个一级菜单在这里收成三组宫格。**宫格不是垃圾桶**：按
 * 「转发链路 / 诊断 / 系统」分组，每组内部按使用频率排；每个入口带实时
 * 角标（待签数、限流数），不用点进去才知道有事。
 */

type Entry = { id: string; label: string; icon: IconName; badge?: string }

export function MorePage() {
  const { session } = useStore()
  const accounts = session?.accounts?.accounts || []
  const limited = accounts.filter(item => item.enabled && isRateLimited(item)).length

  // 本页没有自己的列表请求：下拉只需重拉会话（角标与网关状态都来自它）
  const pull = usePullToRefresh({ onRefresh: () => store.refresh(true) })

  const groups: Array<{ title: string; entries: Entry[] }> = [
    {
      title: t('转发链路'),
      entries: [
        { id: 'checkin', label: t('签到中心'), icon: 'checkin' },
        { id: 'models', label: t('模型管理'), icon: 'model' },
        { id: 'agent', label: t('Agent 配置'), icon: 'agent' },
        { id: 'proxies', label: t('网络代理'), icon: 'proxy' },
        { id: 'keys', label: t('网关 Key'), icon: 'key' },
        { id: 'docs', label: t('接口文档'), icon: 'doc' },
      ],
    },
    {
      title: t('诊断'),
      entries: [
        { id: 'requests', label: t('请求日志'), icon: 'request', badge: limited ? String(limited) : undefined },
        { id: 'logs', label: t('运行日志'), icon: 'log' },
        { id: 'tasks', label: t('定时任务'), icon: 'task' },
      ],
    },
    {
      title: t('系统'),
      entries: [
        { id: 'plugins', label: t('网关插件'), icon: 'plugin', badge: session?.accounts?.hiddenAccounts?.total ? String(session.accounts.hiddenAccounts.total) : undefined },
        { id: 'settings', label: t('设置'), icon: 'settings' },
        { id: 'report', label: t('报表'), icon: 'chart' },
      ],
    },
  ]

  return (
    <div className='m-page' ref={pull.bindRef}>
      <PullIndicator pull={pull.pull} refreshing={pull.refreshing} />
      <PageHead
        title={t('更多')}
        subtitle={t('转发链路 · 诊断 · 系统')}
        actions={<IconButton name='settings' label={t('设置')} onClick={() => store.push('settings')} />}
      />

      <div className='m-section' style={{ marginTop: 0 }}>
        <Card className='m-card' style={{ padding: '10px 0' }}>
          {groups.map((group, index) => (
            <div key={group.title} style={{ padding: index ? '8px 0 4px' : '0 0 4px' }}>
              <div style={{ padding: '0 20px 4px', fontSize: 11.5, fontWeight: 600, color: 'var(--m-text-3)', letterSpacing: '.02em' }}>
                {group.title}
              </div>
              <div style={{ display: 'grid', gridTemplateColumns: 'repeat(4, 1fr)', gap: 4, padding: '4px 10px 10px' }}>
                {group.entries.map(entry => (
                  <Button size='default' variant='ghost'
                    key={entry.id}
                    type='button'
                    onClick={() => store.push(entry.id)}
                    style={{
                      display: 'flex',
                      flexDirection: 'column',
                      alignItems: 'center',
                      gap: 7,
                      padding: '12px 2px',
                      borderRadius: 13,
                      border: 0,
                      background: 'none',
                      fontSize: 11.5,
                      color: 'var(--m-text-2)',
                      position: 'relative',
                    }}
                  >
                    <span
                      style={{
                        width: 44,
                        height: 44,
                        borderRadius: 14,
                        display: 'grid',
                        placeItems: 'center',
                        background: 'var(--m-surface)',
                        border: '1px solid var(--m-line)',
                        color: 'var(--m-primary)',
                        boxShadow: 'var(--m-shadow-1)',
                      }}
                    >
                      <Icon name={entry.icon} style={{ width: 22, height: 22 }} />
                    </span>
                    {entry.label}
                    {entry.badge ? (
                      <span
                        style={{
                          position: 'absolute',
                          top: 8,
                          right: '50%',
                          transform: 'translateX(26px)',
                          minWidth: 16,
                          height: 16,
                          padding: '0 4px',
                          borderRadius: 8,
                          background: 'var(--m-danger)',
                          color: '#fff',
                          fontSize: 10,
                          fontWeight: 600,
                          display: 'grid',
                          placeItems: 'center',
                        }}
                      >
                        {entry.badge}
                      </span>
                    ) : null}
                  </Button>
                ))}
              </div>
            </div>
          ))}
        </Card>
      </div>

      <div className='m-section'>
        <SectionHead title={t('外观')} />
        <Card className='m-card'>
          <div className='m-row' style={{ flexWrap: 'wrap', gap: 8 }}>
            <span className='m-logo'>
              <Icon name='moon' />
            </span>
            <div className='m-grow'>
              <div className='m-t1'>{t('主题')}</div>
              <div className='m-t2'>{t('跟随系统 · 浅色 · 深色')}</div>
            </div>
            <ThemeSwitch />
          </div>
        </Card>
      </div>

      <div className='m-section'>
        <Card className='m-card'>
          <Button size='default' variant='ghost' type='button' className='m-row h-auto min-h-[52px] w-full justify-start rounded-none border-0 px-3.5 py-3 text-left' onClick={() => store.push('settings')}>
            <span className='m-logo'>
              <Icon name='server' />
            </span>
            <div className='m-grow'>
              <div className='m-t1'>{t('网关状态')}</div>
              <div className='m-t2 m-mono' style={{ fontSize: 11.5 }}>
                {baseLabel(apiBase())}
              </div>
            </div>
            <span className='m-chev'>
              <Icon name='chev' />
            </span>
          </Button>
          <Button size='default' variant='ghost' type='button' className='m-row h-auto min-h-[52px] w-full justify-start rounded-none border-0 px-3.5 py-3 text-left' onClick={() => store.push('docs')}>
            <span className='m-logo'>
              <Icon name='doc' />
            </span>
            <div className='m-grow'>
              <div className='m-t1'>{t('接口地址')}</div>
              <div className='m-t2'>{t('把 Base URL 复制给客户端')}</div>
            </div>
            <span className='m-chev'>
              <Icon name='chev' />
            </span>
          </Button>
        </Card>
      </div>

      <div className='m-section' style={{ paddingBottom: 8 }}>
        <Card className='m-card m-card-pad' style={{ fontSize: 11.5, color: 'var(--m-text-3)', lineHeight: 1.7 }}>
          {t('手机端可管理所连接网关的账号、签到与插件，并使用提供商声明的动态登录入口；其余既有添加入口、编辑凭证和改写客户端配置仍需桌面面板。')}
        </Card>
      </div>
    </div>
  )
}

/** 主题五档：跟随系统 / 浅色 / 深色 / 枫叶·白 / 枫叶·黑（与桌面端设置页同一份取值） */
export function ThemeSwitch() {
  const { theme } = useStore()
  const options: Array<{ value: ThemeMode; label: string }> = [
    { value: 'system', label: t('系统') },
    { value: 'light', label: t('浅色') },
    { value: 'dark', label: t('深色') },
    { value: 'maple-light', label: t('枫叶·白') },
    { value: 'maple-dark', label: t('枫叶·黑') },
  ]
  return <SegmentedControl options={options} value={theme} aria-label={t('主题')}
    onValueChange={value => store.setTheme(value)} />
}
