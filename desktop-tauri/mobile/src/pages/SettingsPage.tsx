import { useState } from 'react'
import { PageIntro, SectionHead, IconButton } from '../components/ui'
import { Icon } from '../components/Icon'
import { ThemeSwitch } from './MorePage'
import { store, useStore } from '../state/store'
import { locale, localeOptions, setLocale } from '../i18n'
import { storeApiKey } from '../api/client'
import { usePwa } from '../lib/usePwa'
import { applyUpdate, promptInstall } from '../lib/pwa'
import { apiBase, baseLabel } from '../lib/servers'
import { t } from '../i18n'

/**
 * 设置。
 *
 * ── 手机上只放「改了就生效、且不需要复杂表单」的项 ────────────
 * 桌面端设置页有 11 个分类、几十个字段。手机上保留：
 *   · 外观与语言（纯前端偏好）
 *   · 账号凭证（兼容模式的 API Key、退出登录）
 *   · 网关地址（复制给客户端）
 *   · 关于与安装（PWA 安装引导、版本）
 * 端口、路由、重试、超时、数据保留这些需要看文档才能填对的，一律留在桌面端
 * —— 摆出来只会让人误改。
 */
export function SettingsPage() {
  const { session, packaged, server, servers } = useStore()
  const pwa = usePwa()
  const [installing, setInstalling] = useState(false)
  const currentLocale = locale()
  const options = localeOptions()

  const install = async () => {
    setInstalling(true)
    const outcome = await promptInstall()
    setInstalling(false)
    if (outcome === 'accepted') store.toast(t('正在安装…'))
    else if (outcome === 'dismissed') store.toast(t('已取消安装'))
    // 'unavailable'：没有可用事件（iOS 或已安装），界面本来就展示手动指引
  }

  const signOut = async () => {
    const { logout } = await import('../api/auth')
    const { clearPanelAuth } = await import('../api/client')
    await logout()
    clearPanelAuth()
    store.setAuthLost('login')
    store.toast(t('已退出登录'))
  }

  return (
    <div className='m-page'>
      <PageIntro subtitle={t('外观 · 接入 · 关于')}
        actions={<IconButton name='refresh' label={t('刷新')} onClick={() => void store.refresh(true)} />}
      />

      {/* 服务器：APK 才有（浏览器里地址就是当前 origin，没什么可配的） */}
      {packaged ? (
        <div className='m-section' style={{ marginTop: 0 }}>
          <SectionHead title={t('服务器')} />
          <div className='m-card'>
            <button type='button' className='m-row' onClick={() => store.push('servers')}>
              <span className='m-logo' style={{ background: 'var(--m-primary-soft)', borderColor: 'transparent', color: 'var(--m-primary)' }}>
                <Icon name='server' />
              </span>
              <div className='m-grow'>
                <div className='m-t1'>{server?.name || baseLabel(server?.base || '')}</div>
                <div className='m-t2 m-mono' style={{ fontSize: 11.5 }}>
                  {servers.length > 1 ? t('{n} 台可选 · 点此切换', { n: servers.length }) : baseLabel(server?.base || '')}
                </div>
              </div>
              <span className='m-chev'>
                <Icon name='chev' />
              </span>
            </button>
          </div>
        </div>
      ) : null}

      {/* 网关状态 */}
      <div className='m-section' style={packaged ? undefined : { marginTop: 0 }}>
        <div className='m-card'>
          <div className='m-row'>
            <span
              className='m-logo'
              style={{ background: 'var(--m-ok-soft)', borderColor: 'transparent', color: 'var(--m-ok)' }}
            >
              <Icon name='bolt' />
            </span>
            <div className='m-grow'>
              <div className='m-t1'>{session?.health?.upstreamConfigured ? t('网关运行中') : t('上游未配置')}</div>
              <div className='m-t2 m-mono' style={{ fontSize: 11.5 }}>
                {baseLabel(apiBase())}
              </div>
            </div>
            <span className={`m-badge ${session?.health?.upstreamConfigured ? 'm-badge-ok' : 'm-badge-warn'}`}>
              <i />
              {session?.health?.upstreamConfigured ? t('正常') : t('待配置')}
            </span>
          </div>
        </div>
      </div>

      <div className='m-section'>
        <SectionHead title={t('外观')} />
        <div className='m-card'>
          <div className='m-row'>
            <span className='m-logo'>
              <Icon name='moon' />
            </span>
            <div className='m-grow'>
              <div className='m-t1'>{t('主题')}</div>
              <div className='m-t2'>{t('跟随系统 · 浅色 · 深色')}</div>
            </div>
            <ThemeSwitch />
          </div>
          <div className='m-row'>
            <span className='m-logo'>
              <Icon name='globe' />
            </span>
            <div className='m-grow'>
              <div className='m-t1'>{t('界面语言')}</div>
              <div className='m-t2'>{options.find(item => item.code === currentLocale)?.label || currentLocale}</div>
            </div>
            <select
              value={currentLocale}
              onChange={event => setLocale(event.target.value)}
              aria-label={t('界面语言')}
              style={{
                height: 34,
                padding: '0 8px',
                borderRadius: 9,
                border: '1px solid var(--m-line)',
                background: 'var(--m-surface-2)',
                color: 'var(--m-text)',
                // 16px 是 iOS 的阈值：小于它聚焦时 Safari 会自动放大整页
                fontSize: 16,
                flex: 'none',
              }}
            >
              {options.map(item => (
                <option key={item.code} value={item.code}>
                  {item.label}
                </option>
              ))}
            </select>
          </div>
        </div>
      </div>

      <div className='m-section'>
        <SectionHead title={t('接入')} />
        <div className='m-card'>
          <button type='button' className='m-row' onClick={() => store.push('docs')}>
            <span className='m-logo'>
              <Icon name='doc' />
            </span>
            <div className='m-grow'>
              <div className='m-t1'>{t('接口地址')}</div>
              <div className='m-t2 m-mono' style={{ fontSize: 11.5 }}>
                {apiBase()}/v1
              </div>
            </div>
            <span className='m-chev'>
              <Icon name='chev' />
            </span>
          </button>
          <button type='button' className='m-row' onClick={() => store.push('keys')}>
            <span className='m-logo'>
              <Icon name='key' />
            </span>
            <div className='m-grow'>
              <div className='m-t1'>{t('网关 Key')}</div>
              <div className='m-t2'>{t('复制给客户端')}</div>
            </div>
            <span className='m-chev'>
              <Icon name='chev' />
            </span>
          </button>
        </div>
      </div>

      <div className='m-section'>
        <SectionHead title={t('账号')} />
        <div className='m-card'>
          {/* APK 有真正的登录态，给「退出登录」；浏览器端只有 API Key 可清 */}
          {packaged ? (
            <button type='button' className='m-row' onClick={() => void signOut()}>
              <span className='m-logo'>
                <Icon name='logout' />
              </span>
              <div className='m-grow'>
                <div className='m-t1'>{t('退出登录')}</div>
                <div className='m-t2'>{t('退出这台服务器的登录，不影响其他服务器')}</div>
              </div>
              <span className='m-chev'>
                <Icon name='chev' />
              </span>
            </button>
          ) : (
            <button
              type='button'
              className='m-row'
              onClick={() => {
                storeApiKey('')
                store.setAuthLost(null)
                store.toast(t('已清除本机保存的 API Key'))
                void store.refresh(true)
              }}
            >
              <span className='m-logo'>
                <Icon name='logout' />
              </span>
              <div className='m-grow'>
                <div className='m-t1'>{t('清除本机凭证')}</div>
                <div className='m-t2'>{t('只清掉这个浏览器存的 Key，不影响网关')}</div>
              </div>
              <span className='m-chev'>
                <Icon name='chev' />
              </span>
            </button>
          )}
        </div>
      </div>

      {/* APK 不需要装到桌面（本身就是 App），这一段只给浏览器形态 */}
      {packaged ? null : (
      <div className='m-section'>
        <SectionHead title={t('安装到桌面')} />
        <div className='m-card'>
          <div className='m-row'>
            <span className='m-logo'>
              <Icon name='install' />
            </span>
            <div className='m-grow'>
              <div className='m-t1'>{t('当前形态')}</div>
              <div className='m-t2'>
                {pwa.standalone ? t('已作为独立应用运行') : t('浏览器标签页')}
              </div>
            </div>
            <span className={`m-badge ${pwa.standalone ? 'm-badge-ok' : 'm-badge-mute'}`}>
              {pwa.standalone ? t('已安装') : t('未安装')}
            </span>
          </div>

          {pwa.standalone ? (
            <div className='m-card-pad' style={{ borderTop: '1px solid var(--m-line)', fontSize: 12.5, lineHeight: 1.7, color: 'var(--m-text-2)' }}>
              {t('从桌面图标启动，全屏、无地址栏。想卸载：长按图标删除，或清除浏览器站点数据。')}
            </div>
          ) : pwa.canInstall ? (
            <div style={{ borderTop: '1px solid var(--m-line)', padding: '12px 15px 14px' }}>
              <div style={{ fontSize: 12.5, lineHeight: 1.7, color: 'var(--m-text-2)' }}>
                {t('装成 App 后从桌面图标直接进入：全屏打开、没有地址栏。')}
              </div>
              <button
                type='button'
                className='m-btn'
                style={{ marginTop: 10, height: 40 }}
                disabled={installing}
                onClick={() => void install()}
              >
                <Icon name='download' />
                {installing ? t('正在安装…') : t('安装到主屏幕')}
              </button>
            </div>
          ) : (
            <div style={{ borderTop: '1px solid var(--m-line)', padding: '12px 15px 14px' }}>
              <div style={{ fontSize: 12.5, lineHeight: 1.75, color: 'var(--m-text-2)' }}>
                {pwa.showIosHint
                  ? t('Safari 没有安装按钮：点底部的「分享」，选「添加到主屏幕」，再点「添加」。')
                  : t('用浏览器菜单里的「添加到主屏幕」把这一页装成 App：全屏打开、没有地址栏，和原生应用一样。')}
              </div>
              {pwa.showIosHint ? (
                <div
                  style={{
                    display: 'flex',
                    alignItems: 'center',
                    gap: 8,
                    marginTop: 10,
                    padding: '9px 11px',
                    borderRadius: 11,
                    background: 'var(--m-surface-2)',
                    fontSize: 12,
                    color: 'var(--m-text-3)',
                  }}
                >
                  <Icon name='share' style={{ width: 16, height: 16, flex: 'none' }} />
                  <span>{t('分享图标在 Safari 底部工具栏中间')}</span>
                </div>
              ) : null}
            </div>
          )}
        </div>
      </div>
      )}

      {/* 只有真有事时才出现：平时不占版面 */}
      {pwa.updateReady || pwa.swState === 'unsupported' ? (
        <div className='m-section'>
          <SectionHead title={t('应用状态')} />
          <div className='m-card'>
            {pwa.updateReady ? (
              <div className='m-row'>
                <span className='m-logo' style={{ background: 'var(--m-primary-soft)', borderColor: 'transparent', color: 'var(--m-primary)' }}>
                  <Icon name='up' />
                </span>
                <div className='m-grow'>
                  <div className='m-t1'>{t('有新版本')}</div>
                  <div className='m-t2'>{t('重启后生效，不影响当前操作')}</div>
                </div>
                <button type='button' className='m-btn m-sm' onClick={() => void applyUpdate()}>
                  {t('重启更新')}
                </button>
              </div>
            ) : null}
            {pwa.swState === 'unsupported' ? (
              <div className='m-row'>
                <span className='m-logo'>
                  <Icon name='info' />
                </span>
                <div className='m-grow'>
                  <div className='m-t1'>{t('离线缓存未启用')}</div>
                  <div className='m-t2'>{t('浏览器只在 https 或 localhost 下提供离线缓存；局域网 http 访问不影响正常使用')}</div>
                </div>
              </div>
            ) : null}
          </div>
        </div>
      ) : null}

      <div className='m-section' style={{ paddingBottom: 8 }}>
        <div className='m-card'>
          <div className='m-row'>
            <span className='m-logo'>
              <Icon name='info' />
            </span>
            <div className='m-grow'>
              <div className='m-t1'>{t('Agent2API 手机端')}</div>
              <div className='m-t2'>{t('与网关同源的面板；完整管理功能在桌面端')}</div>
            </div>
          </div>
          <div className='m-row' style={{ borderTop: '1px solid var(--m-line)' }}>
            <span className='m-logo'>
              <Icon name='heart' />
            </span>
            <div className='m-grow'>
              <div className='m-t1'>{t('社区与支持')}</div>
              <div className='m-t2'>{t('QQ 群 · Telegram · Discord')}</div>
            </div>
            <span className='m-chev'>
              <Icon name='chev' />
            </span>
          </div>
        </div>
      </div>
    </div>
  )
}

