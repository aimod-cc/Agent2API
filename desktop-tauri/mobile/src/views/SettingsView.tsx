import { useEffect, useState } from 'react'
import { compactRequestsDb, fetchStorageInfo, type StorageSummary } from '../core/api'
import { logout } from '../core/auth'
import { probe } from '../core/client'
import { formatBytes, formatNumber } from '../core/format'
import { localeOptions, rawLocale, setLocale, t } from '../core/i18n'
import { isChineseUnits, type UnitsPreference } from '../core/units'
import { baseLabel, nativePlatform, openExternalUrl, serverIdentity } from '../core/servers'
import { useLoginNoAutoOpen } from '../../../ui-shared/login-browser-prefs'
import { store, useStore, type ThemeMode } from '../core/store'
import {
  APP_VERSION,
  cancelApkDownload,
  canSelfUpdate,
  checkMobileUpdate,
  downloadApk,
  hasDownloadedApk,
  installDownloadedApk,
  resolveUpdatePageUrl,
  type DownloadProgress,
  type MobileUpdateInfo,
} from '../core/updater'
import {
  CommonCard,
  CommonScaffold,
  CommonSwitch,
  getTabMeta,
  ListGroup,
  ListItem,
  SectionHeader,
  SegmentedControl,
  TAB_CATALOG,
  TonalChip,
} from '../ui/components'
import { GlyphIcon } from '../ui/icons'
import { PickerSheet, PickerTrigger, type PickerOption } from '../ui/pickers'

const THEME_OPTIONS: ReadonlyArray<{ id: ThemeMode; label: string }> = [
  { id: 'system', label: '跟随系统' },
  { id: 'light', label: '浅色' },
  { id: 'dark', label: '深色' },
]

const UNITS_OPTIONS: ReadonlyArray<{ id: UnitsPreference; label: string }> = [
  { id: 'auto', label: '跟随设置' },
  { id: 'zh', label: '万 / 亿' },
  { id: 'en', label: 'k / M / B' },
]

export function SettingsView() {
  const {
    server,
    servers,
    packaged,
    theme,
    unitsPref,
    gatewayLocale,
    navTabs,
    session,
    loading,
    sessionError,
    mobileUpdate,
    revision,
  } = useStore()
  const currentLocale = rawLocale()
  const [loginNoAutoOpen, setLoginNoAutoOpen] = useLoginNoAutoOpen()
  const [langPickerOpen, setLangPickerOpen] = useState(false)
  const [storage, setStorage] = useState<StorageSummary | null>(null)
  const [compacting, setCompacting] = useState(false)
  const [probing, setProbing] = useState(false)
  const [gatewayVer, setGatewayVer] = useState<string>('')

  const [updatePhase, setUpdatePhase] = useState<
    'idle' | 'checking' | 'latest' | 'available' | 'downloading' | 'downloaded'
  >('idle')
  const [updateInfo, setUpdateInfo] = useState<MobileUpdateInfo | null>(null)
  const [dlProgress, setDlProgress] = useState<DownloadProgress | null>(null)
  const [updateErr, setUpdateErr] = useState('')
  const platform = nativePlatform()
  const selfUpdate = canSelfUpdate()

  useEffect(() => {
    void hasDownloadedApk().then(res => {
      if (res.exists) setUpdatePhase('downloaded')
    })
  }, [])

  useEffect(() => {
    if (!mobileUpdate) {
      setUpdateInfo(null)
      setUpdatePhase(prev => (prev === 'available' || prev === 'latest' ? 'idle' : prev))
      return
    }
    setUpdateInfo(mobileUpdate)
    setUpdatePhase(prev => {
      if (prev === 'downloading' || prev === 'downloaded' || prev === 'checking') return prev
      if (mobileUpdate.hasUpdate === true) return 'available'
      if (mobileUpdate.hasUpdate === false) return 'latest'
      return prev
    })
  }, [mobileUpdate])

  useEffect(() => {
    const controller = new AbortController()
    const id = serverIdentity()
    if (!server?.base) return
    fetchStorageInfo(controller.signal)
      .then(info => {
        if (serverIdentity() === id) setStorage(info)
      })
      .catch(() => {})
    probe(server.base)
      .then(res => {
        if (serverIdentity() === id && res.version) setGatewayVer(res.version)
      })
      .catch(() => {})
    return () => controller.abort()
  }, [revision, server?.base])

  const handleCompactDb = async () => {
    if (compacting) return
    setCompacting(true)
    try {
      await compactRequestsDb()
      store.toast('数据库压缩整理完成', 'success')
      const fresh = await fetchStorageInfo().catch(() => null)
      if (fresh) setStorage(fresh)
    } catch (err) {
      store.toast(err instanceof Error ? err.message : '压缩失败', 'danger')
    } finally {
      setCompacting(false)
    }
  }

  const handleCheckGateway = async () => {
    if (!server?.base || probing) return
    setProbing(true)
    try {
      const res = await probe(server.base)
      if (res.ok) {
        if (res.version) setGatewayVer(res.version)
        store.toast(
          `网关在线${res.version ? ` v${res.version}` : ''} · 延迟 ${res.latencyMs ?? 1} ms`,
          'success',
        )
      } else {
        store.toast('无法连接网关，请检查服务器地址', 'danger')
      }
    } finally {
      setProbing(false)
    }
  }

  const updatePageUrl = resolveUpdatePageUrl(updateInfo?.pageUrl, {
    platform,
    version: updateInfo?.latestVersion,
    prerelease: updateInfo?.prerelease,
  })
  const hasDirectIpa = platform === 'ios' && Boolean(updateInfo?.asset?.url)

  const handleCheckAppUpdate = async () => {
    if (updatePhase === 'checking' || updatePhase === 'downloading') return
    setUpdatePhase('checking')
    setUpdateErr('')
    try {
      const res = await checkMobileUpdate()
      setUpdateInfo(res)
      store.setMobileUpdate(res)
      const hasNew = res.hasUpdate === true
      setUpdatePhase(hasNew ? 'available' : res.hasUpdate === false ? 'latest' : 'idle')
      if (res.hasUpdate === false) store.toast('当前已是最新版本', 'success')
      else if (res.hasUpdate === null) store.toast('暂未发现适配当前设备的安装包', 'info')
    } catch (err) {
      setUpdateErr(err instanceof Error ? err.message : '检查更新失败')
      setUpdatePhase('idle')
    }
  }

  const handleOpenExternalUpdate = () => {
    const target = hasDirectIpa ? updateInfo?.asset?.url : updatePageUrl
    void openExternalUrl(target || updatePageUrl)
  }

  const handleDownloadApk = async () => {
    const url = updateInfo?.asset?.url
    if (!url) {
      void openExternalUrl(resolveUpdatePageUrl(updateInfo?.pageUrl))
      return
    }
    setUpdatePhase('downloading')
    setUpdateErr('')
    setDlProgress({ received: 0, total: updateInfo.asset?.size || 0, percent: -1 })
    try {
      await downloadApk(url, p => setDlProgress(p))
      setUpdatePhase('downloaded')
      store.toast('新版本安装包下载完成', 'success')
    } catch (err) {
      setUpdateErr(err instanceof Error ? err.message : '下载失败')
      setUpdatePhase('available')
    } finally {
      setDlProgress(null)
    }
  }

  const handleInstallApk = async () => {
    setUpdateErr('')
    try {
      const res = await installDownloadedApk()
      if (res === 'needs_permission') {
        store.toast('请授予「安装未知来源应用」权限后再次点击安装', 'info')
      } else {
        store.toast('正在打开系统安装器…', 'success')
      }
    } catch (err) {
      setUpdateErr(err instanceof Error ? err.message : '安装失败')
      const cached = await hasDownloadedApk()
      if (!cached.exists) {
        setUpdatePhase(updateInfo?.hasUpdate === true ? 'available' : 'idle')
      }
    }
  }

  const handleLogout = async () => {
    await logout()
    store.clearCredentials()
    store.toast('已退出当前网关会话', 'info')
  }

  const hiddenTabs = TAB_CATALOG.filter(item => !navTabs.includes(item.id))
  const localeList: PickerOption[] = [
    { value: 'auto', label: t('跟随系统') },
    ...localeOptions().map(item => ({ value: item.code, label: item.label })),
  ]
  const currentLocaleLabel = localeList.find(o => o.value === currentLocale)?.label || t('跟随系统')
  const chineseActive = isChineseUnits()
  const navSummary = navTabs.map(id => t(getTabMeta(id).label)).join(' · ')

  return (
    <CommonScaffold title={t('设置')}>
      {hiddenTabs.length > 0 ? (
        <>
          <SectionHeader title={t('更多功能')} />
          <ListGroup>
            {hiddenTabs.map(item => (
              <ListItem
                key={item.id}
                icon={item.icon}
                title={t(item.label)}
                subtitle={t(item.subtitle)}
                chevron
                onClick={() => store.setTab(item.id)}
              />
            ))}
          </ListGroup>
        </>
      ) : null}

      <SectionHeader
        title='网关连接'
        trailing={
          packaged && servers.length > 1 ? (
            <span style={{ fontSize: 12, color: 'var(--fc-text-tertiary)' }}>
              已存 {servers.length} 台
            </span>
          ) : null
        }
      />
      <ListGroup>
        <ListItem
          icon='server'
          title={server?.name || (server?.base ? baseLabel(server.base) : '未配置服务器')}
          subtitle={server?.base || '点击添加网关服务器地址'}
          trailing={
            <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
              <TonalChip
                tone={
                  sessionError
                    ? 'danger'
                    : loading
                      ? 'primary'
                      : session
                        ? 'success'
                        : 'neutral'
                }
              >
                {sessionError ? '离线' : loading ? '连接中' : session ? '在线' : '未连接'}
              </TonalChip>
              <button
                type='button'
                className='fc-btn is-sm is-pill'
                disabled={probing}
                onClick={e => {
                  e.stopPropagation()
                  void handleCheckGateway()
                }}
              >
                {probing ? '测速中' : '测速'}
              </button>
            </div>
          }
          chevron={packaged}
          onClick={() => packaged && store.openSheet({ kind: 'servers' })}
        />
      </ListGroup>

      <SectionHeader title='监控与转发' />
      <ListGroup>
        <ListItem
          icon='activity'
          title='请求日志'
          subtitle={
            storage?.database?.requests !== undefined
              ? `已记录 ${formatNumber(storage.database.requests)} 条下游调用明细`
              : '查看每次模型调用的状态码、耗时与 Token'
          }
          chevron
          onClick={() => store.openSheet({ kind: 'requests' })}
        />
        <ListItem
          icon='terminal'
          title='运行日志'
          subtitle='查看网关系统事件与开启调试日志'
          chevron
          onClick={() => store.openSheet({ kind: 'logs' })}
        />
        <ListItem
          icon='sliders'
          title='转发与超时参数'
          subtitle='自动续写、失败重试次数、连接超时与协议兼容开关'
          chevron
          onClick={() => store.openSheet({ kind: 'forwarding-config' })}
        />
        <ListItem
          icon='bell'
          title='系统通知'
          subtitle='登录成功/失败提醒、自动签到失败通知（Webhook / Telegram / Bark）'
          chevron
          onClick={() => store.openSheet({ kind: 'notifications' })}
        />
        <ListItem
          icon='dollar'
          title='模型计价与汇率'
          subtitle='自定义模型单价、同款参考、提供商倍率及美元/人民币切换'
          chevron
          onClick={() => store.openSheet({ kind: 'pricing' })}
        />
      </ListGroup>

      <SectionHeader title='外观与偏好' />
      <CommonCard>
        <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
          <span className='fc-list-icon-well'>
            <GlyphIcon name='moon' size={18} />
          </span>
          <span style={{ fontSize: 14.5, fontWeight: 600 }}>界面主题</span>
        </div>
        <SegmentedControl
          inCard
          options={THEME_OPTIONS}
          value={theme}
          onChange={next => store.setTheme(next)}
        />

        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'space-between',
            gap: 12,
            paddingTop: 4,
          }}
        >
          <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
            <span className='fc-list-icon-well'>
              <GlyphIcon name='globe' size={18} />
            </span>
            <span style={{ fontSize: 14.5, fontWeight: 600 }}>{t('界面语言')}</span>
          </div>
          <PickerTrigger
            inList
            valueLabel={currentLocaleLabel}
            onClick={() => setLangPickerOpen(true)}
            ariaLabel={t('界面语言')}
          />
        </div>

        <div style={{ display: 'flex', flexDirection: 'column', gap: 8, paddingTop: 6 }}>
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 10 }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
              <span className='fc-list-icon-well'>
                <GlyphIcon name='database' size={18} />
              </span>
              <span style={{ fontSize: 14.5, fontWeight: 600 }}>{t('数据单位')}</span>
            </div>
            <span style={{ fontSize: 12, color: 'var(--fc-text-tertiary)' }}>
              {unitsPref === 'auto'
                ? gatewayLocale
                  ? `${t('数据库')}: ${chineseActive ? '万/亿' : 'k/M/B'}`
                  : chineseActive
                    ? '万 / 亿'
                    : 'k / M / B'
                : chineseActive
                  ? '万 / 亿'
                  : 'k / M / B'}
            </span>
          </div>
          <SegmentedControl
            inCard
            options={UNITS_OPTIONS.map(o => ({ id: o.id, label: t(o.label) }))}
            value={unitsPref}
            onChange={next => store.setUnitsPref(next)}
          />
        </div>
      </CommonCard>

      <ListGroup>
        <ListItem
          icon='link'
          title={t('使用系统浏览器时不自动打开')}
          subtitle={t('仅生成只读授权链接供复制，不自动跳转浏览器')}
          trailing={
            <CommonSwitch
              checked={loginNoAutoOpen}
              onChange={setLoginNoAutoOpen}
            />
          }
        />
        <ListItem
          icon='sliders'
          title={t('自定义底部导航栏')}
          subtitle={navSummary}
          chevron
          onClick={() => store.openSheet({ kind: 'nav-customizer' })}
        />
      </ListGroup>

      <PickerSheet
        open={langPickerOpen}
        title={t('选择界面语言')}
        options={localeList}
        value={currentLocale}
        onSelect={next => setLocale(next)}
        onClose={() => setLangPickerOpen(false)}
      />

      <SectionHeader title='应用更新与数据安全' />
      <CommonCard>
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 10 }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 10, minWidth: 0 }}>
            <span className='fc-icon-badge'>
              <GlyphIcon name='bolt' size={18} />
            </span>
            <div style={{ minWidth: 0 }}>
              <div style={{ fontSize: 14.5, fontWeight: 700 }}>
                {updatePhase === 'available'
                  ? `发现新版本 ${updateInfo?.latestVersion || ''}`
                  : updatePhase === 'downloaded'
                    ? '新版本安装包已就绪'
                    : updatePhase === 'latest'
                      ? '已是最新版本'
                      : `移动端 v${APP_VERSION}`}
              </div>
              <div style={{ fontSize: 12, color: 'var(--fc-text-secondary)', marginTop: 2 }}>
                {updatePhase === 'downloading'
                  ? dlProgress && dlProgress.percent >= 0
                    ? `正在下载安装包 (${dlProgress.percent}%)…`
                    : '正在连接官网下载安装包…'
                  : gatewayVer
                    ? `网关服务端 v${gatewayVer}`
                    : '检查官方网站最新客户端版本'}
              </div>
            </div>
          </div>

          {updatePhase === 'downloading' ? (
            <button
              type='button'
              className='fc-btn is-sm is-pill'
              onClick={async () => {
                await cancelApkDownload()
                setUpdatePhase('available')
                setDlProgress(null)
              }}
            >
              取消
            </button>
          ) : updatePhase === 'downloaded' ? (
            <button
              type='button'
              className='fc-btn is-primary is-sm is-pill'
              onClick={handleInstallApk}
            >
              安装
            </button>
          ) : updatePhase === 'available' ? (
            <button
              type='button'
              className='fc-btn is-primary is-sm is-pill'
              onClick={selfUpdate ? handleDownloadApk : handleOpenExternalUpdate}
            >
              {selfUpdate ? '立即更新' : hasDirectIpa ? '下载 IPA' : '官网下载'}
            </button>
          ) : (
            <button
              type='button'
              className='fc-btn is-tonal is-sm is-pill'
              disabled={updatePhase === 'checking'}
              onClick={handleCheckAppUpdate}
            >
              {updatePhase === 'checking' ? '检查中…' : '检查更新'}
            </button>
          )}
        </div>

        {updatePhase === 'downloading' ? (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
            <div
              style={{
                height: 6,
                borderRadius: 999,
                background: 'var(--fc-inset)',
                overflow: 'hidden',
              }}
            >
              <div
                style={{
                  height: '100%',
                  width: `${Math.max(5, dlProgress?.percent ?? 15)}%`,
                  background: 'var(--fc-primary)',
                  transition: 'width 180ms ease',
                }}
              />
            </div>
          </div>
        ) : null}

        {updatePhase === 'available' && platform === 'ios' ? (
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8, flexWrap: 'wrap' }}>
            <span style={{ fontSize: 12, color: 'var(--fc-text-secondary)' }}>
              未签名 IPA，下载后需通过 AltStore / SideStore 或自签名安装
            </span>
            {hasDirectIpa ? (
              <button
                type='button'
                className='fc-btn is-tonal is-sm is-pill'
                onClick={() => void openExternalUrl(updatePageUrl)}
              >
                官网下载页
              </button>
            ) : null}
          </div>
        ) : null}

        {updateInfo?.notes && (updatePhase === 'available' || updatePhase === 'downloaded') ? (
          <div
            className='fc-inner-block'
            style={{ fontSize: 12, color: 'var(--fc-text-secondary)', whiteSpace: 'pre-wrap' }}
          >
            {updateInfo.notes}
          </div>
        ) : null}

        {updateErr ? (
          <div style={{ fontSize: 12.5, color: 'var(--fc-danger)' }}>{updateErr}</div>
        ) : null}
      </CommonCard>

      <ListGroup>
        <ListItem
          icon='database'
          title='数据库空间'
          subtitle={
            storage?.database?.bytes
              ? `当前占用 ${formatBytes(storage.database.bytes)}`
              : '点击压缩整理历史数据库碎片'
          }
          trailing={
            <button
              type='button'
              className='fc-btn is-tonal is-sm is-pill'
              disabled={compacting}
              onClick={e => {
                e.stopPropagation()
                void handleCompactDb()
              }}
            >
              {compacting ? '整理中…' : '压缩整理'}
            </button>
          }
        />
        <ListItem
          icon='shield'
          title='修改管理员密码'
          subtitle='更新当前网关管理面板的登录密码'
          chevron
          onClick={() => store.openSheet({ kind: 'password' })}
        />
        <ListItem
          icon='link'
          title='第三方登录 (OAuth)'
          subtitle='配置 Google / GitHub 登录白名单与绑定账号'
          chevron
          onClick={() => store.openSheet({ kind: 'oauth' })}
        />
        <ListItem
          icon='logout'
          title={<span style={{ color: 'var(--fc-danger)' }}>退出当前网关登录</span>}
          subtitle='清除本地保存的会话令牌与 API Key'
          onClick={() => void handleLogout()}
        />
      </ListGroup>
    </CommonScaffold>
  )
}
