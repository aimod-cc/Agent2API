import * as React from 'react'
import {
  Badge,
  Button,
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Input,
  Label,
  SegmentedControl,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Switch,
} from '@ui'
import { errorMessage, shared, toast } from './settings-model'
import { t } from '../i18n'

/**
 * 设置 → 数据 → 云同步（对标 cc-switch 的 WebDAV / S3 同步区块）。
 *
 * ── 为什么独立成文件、自带数据流 ────────────────────────────
 * settings-page.tsx 已经 2300 多行，而这一块与设置页的其它面板**没有状态交集**
 * （它自己读写 `/api/cloud-sync`）。与 `settings-prompt-editor.tsx` 同一先例：
 * 面板独立成文件、自己在挂载时取数，宿主只负责放一个 `<CloudSyncPanel />`。
 *
 * ── 表单为什么不复用 settings-state 的快照 ──────────────────
 * 那一层服务的是「读一次、整页共享」的设置项；本面板是**一份可编辑表单**
 * （用户改一半切走再回来、保存前不能丢），用局部 state 更直接。密码类字段
 * 按「留空 = 沿用」的语义处理 —— 这条判断在**后端**做（空串即沿用库里那份），
 * 前端不需要另存「是否触碰过」，少一个会与后端不一致的状态。
 *
 * ── 两种服务商共用一套流程 ──────────────────────────────────
 * 后端把两套配置都存在同一个 `cloudSync` 键里（切换不丢已填内容），所以这里
 * 只切表单的 `provider`，不隐藏 / 不互斥。测试、保存、上传、下载都只作用于
 * 当前选中的那一家。
 *
 * ── 下载是「替换」语义 ──────────────────────────────────────
 * 与整库恢复一致：本机业务数据会被远端快照整体替换（日志、请求记录、面板
 * 登录态不在同步范围，保持原样）。所以下载前必须有二次确认，并把「会先自动
 * 备份」写清楚。
 */

/* ─── 桥返回的形状（与 api::cloud_sync 的 JSON 一一对应） ───── */

type SyncStatus = {
  lastSyncAt?: number
  lastAction?: string
  lastSnapshotId?: string
  lastLocalHash?: string
  lastKeyFingerprint?: string
  lastError?: string
  lastErrorSource?: string
}

type CloudSyncView = {
  provider: string
  webdav: { baseUrl: string; username: string; hasPassword: boolean }
  s3: {
    region: string
    bucket: string
    accessKeyId: string
    endpoint: string
    hasSecretKey: boolean
  }
  hasSyncPassword: boolean
  enabled: boolean
  autoSync: boolean
  remoteRoot: string
  profile: string
  status: SyncStatus
  remotePath: string
  keyFingerprint: string | null
}

type RemoteInfo = {
  empty?: boolean
  remotePath?: string
  format?: string
  version?: number
  appVersion?: string
  schemaVersion?: number
  deviceName?: string
  createdAt?: string
  snapshotId?: string
  artifacts?: string[]
  compatible?: boolean
  incompatibleReason?: string
  keyFingerprint?: string | null
  localKeyFingerprint?: string | null
  keyRequired?: boolean
  hasKeyArtifact?: boolean
}

type Form = {
  provider: string
  webdavBaseUrl: string
  webdavUsername: string
  webdavPassword: string
  s3Region: string
  s3Bucket: string
  s3AccessKeyId: string
  s3SecretAccessKey: string
  s3Endpoint: string
  syncPassword: string
  enabled: boolean
  autoSync: boolean
  remoteRoot: string
  profile: string
}

const PROVIDER_WEBDAV = 'webdav'
const PROVIDER_S3 = 's3'

/** WebDAV 服务商预设：选中只填地址占位与提示，不改用户已输入的内容 */
const WEBDAV_PRESETS = [
  {
    id: 'jianguoyun',
    label: t('坚果云'),
    baseUrl: 'https://dav.jianguoyun.com/dav/',
    hint: t('坚果云需使用「第三方应用密码」（账户信息 → 安全选项里生成），地址必须是 /dav/ 下的目录。'),
  },
  {
    id: 'nextcloud',
    label: t('Nextcloud'),
    baseUrl: 'https://your-server/remote.php/dav/files/USERNAME/',
    hint: t('把地址里的 your-server 与 USERNAME 换成你自己的；目录不存在时会自动创建。'),
  },
  {
    id: 'synology',
    label: t('群晖 NAS'),
    baseUrl: 'http://your-nas-ip:5005/',
    hint: t('群晖需在「控制面板 → 应用程序门户 → WebDAV」里启用服务，端口默认 5005（HTTP）/ 5006（HTTPS）。'),
  },
  { id: 'custom', label: t('自定义'), baseUrl: '', hint: '' },
] as const

/** S3 预设：只影响提示与 Region 占位符（Endpooint 由用户填） */
const S3_PRESETS = [
  {
    id: 'aws',
    label: 'Amazon S3',
    regionPlaceholder: 'us-east-1',
    hint: t('Endpoint 留空即走 AWS 官方地址；Region 必须与桶所在区域一致。'),
  },
  {
    id: 'r2',
    label: 'Cloudflare R2',
    regionPlaceholder: 'auto',
    hint: t('Endpoint 填 https://<账户ID>.r2.cloudflarestorage.com，Region 固定填 auto。'),
  },
  {
    id: 'minio',
    label: 'MinIO',
    regionPlaceholder: 'us-east-1',
    hint: t('Endpoint 形如 http://192.168.1.10:9000（自建通常用 http）；Region 一般填 us-east-1。'),
  },
  {
    id: 'oss',
    label: t('阿里云 OSS'),
    regionPlaceholder: 'cn-hangzhou',
    hint: t('Endpoint 填 https://oss-cn-hangzhou.aliyuncs.com，Region 填 cn-hangzhou（按实际地域改）。'),
  },
  {
    id: 'cos',
    label: t('腾讯云 COS'),
    regionPlaceholder: 'ap-guangzhou',
    hint: t('Endpoint 填 https://cos.ap-guangzhou.myqcloud.com，Region 填 ap-guangzhou（按实际地域改）。'),
  },
  {
    id: 'obs',
    label: t('华为云 OBS'),
    regionPlaceholder: 'cn-north-4',
    hint: t('Endpoint 填 https://obs.cn-north-4.myhuaweicloud.com，Region 填 cn-north-4（按实际地域改）。'),
  },
  { id: 'custom', label: t('自定义'), regionPlaceholder: 'us-east-1', hint: '' },
] as const

function formatTime(ms?: number): string {
  if (!ms) return ''
  try {
    return new Date(ms).toLocaleString()
  } catch {
    return String(ms)
  }
}

function formatDateText(raw?: string): string {
  if (!raw) return '—'
  const parsed = new Date(raw)
  return Number.isNaN(parsed.getTime()) ? raw : parsed.toLocaleString()
}

function emptyForm(): Form {
  return {
    provider: PROVIDER_WEBDAV,
    webdavBaseUrl: '',
    webdavUsername: '',
    webdavPassword: '',
    s3Region: '',
    s3Bucket: '',
    s3AccessKeyId: '',
    s3SecretAccessKey: '',
    s3Endpoint: '',
    syncPassword: '',
    enabled: false,
    autoSync: false,
    remoteRoot: 'agent2api-sync',
    profile: 'default',
  }
}

function formFromView(view: CloudSyncView): Form {
  return {
    provider: view.provider === PROVIDER_S3 ? PROVIDER_S3 : PROVIDER_WEBDAV,
    webdavBaseUrl: view.webdav.baseUrl,
    webdavUsername: view.webdav.username,
    // 密码不回显：留空 = 沿用后端已保存的那份
    webdavPassword: '',
    s3Region: view.s3.region,
    s3Bucket: view.s3.bucket,
    s3AccessKeyId: view.s3.accessKeyId,
    s3Endpoint: view.s3.endpoint,
    s3SecretAccessKey: '',
    syncPassword: '',
    enabled: view.enabled,
    autoSync: view.autoSync,
    remoteRoot: view.remoteRoot,
    profile: view.profile,
  }
}

/** 保存 / 测试共用的请求体（字段名与 `api::cloud_sync::parse_save_input` 对齐） */
function payloadOf(form: Form): Record<string, unknown> {
  return {
    provider: form.provider,
    webdavBaseUrl: form.webdavBaseUrl,
    webdavUsername: form.webdavUsername,
    webdavPassword: form.webdavPassword,
    s3Region: form.s3Region,
    s3Bucket: form.s3Bucket,
    s3AccessKeyId: form.s3AccessKeyId,
    s3SecretAccessKey: form.s3SecretAccessKey,
    s3Endpoint: form.s3Endpoint,
    syncPassword: form.syncPassword,
    enabled: form.enabled,
    autoSync: form.autoSync,
    remoteRoot: form.remoteRoot,
    profile: form.profile,
  }
}

type Busy = 'load' | 'test' | 'save' | 'upload' | 'download' | 'remote' | 'key' | null
type DialogKind = 'upload' | 'download' | 'key' | null

export function CloudSyncPanel() {
  const bridge = shared().workbuddyDesktop
  const [view, setView] = React.useState<CloudSyncView | null>(null)
  const [form, setForm] = React.useState<Form>(emptyForm)
  const [dirty, setDirty] = React.useState(false)
  const [busy, setBusy] = React.useState<Busy>('load')
  const [loadFailed, setLoadFailed] = React.useState(false)
  const [remote, setRemote] = React.useState<RemoteInfo | null>(null)
  const [dialog, setDialog] = React.useState<DialogKind>(null)
  const [keyPassword, setKeyPassword] = React.useState('')

  const isWebDav = form.provider === PROVIDER_WEBDAV
  const webdavPreset = WEBDAV_PRESETS.find(item => item.baseUrl && form.webdavBaseUrl.includes(new URL(item.baseUrl).host))
  const s3Preset = S3_PRESETS.find(item => item.id !== 'custom' && item.label === 'Cloudflare R2' && form.s3Region === 'auto') ?? S3_PRESETS[0]

  /** 取一次设置并铺进表单（保留用户正在编辑的密码输入） */
  const load = React.useCallback(async (keepPassword = false) => {
    if (!bridge?.getCloudSync) {
      setLoadFailed(true)
      setBusy(null)
      return
    }
    setBusy(current => current ?? 'load')
    try {
      const data = (await bridge.getCloudSync()) as CloudSyncView | null | undefined
      if (!data) {
        setLoadFailed(true)
        return
      }
      setLoadFailed(false)
      setView(data)
      setForm(current => {
        const next = formFromView(data)
        if (keepPassword) {
          next.webdavPassword = current.webdavPassword
          next.s3SecretAccessKey = current.s3SecretAccessKey
          next.syncPassword = current.syncPassword
        }
        return next
      })
      setDirty(false)
    } catch (error) {
      setLoadFailed(true)
      toast(t('读取云同步设置失败：{message}', { message: errorMessage(error) }), 'err')
    } finally {
      setBusy(null)
    }
  }, [bridge])

  React.useEffect(() => {
    void load()
  }, [load])

  const patch = (changes: Partial<Form>) => {
    setForm(current => ({ ...current, ...changes }))
    setDirty(true)
  }

  const disabled = busy !== null

  /* ─── 动作 ─────────────────────────────────────────────── */

  const test = async () => {
    if (!bridge?.testCloudSync) return
    setBusy('test')
    try {
      const result = (await bridge.testCloudSync(payloadOf(form))) as { remotePath?: string } | null
      toast(t('✅ 连接成功：{path}', { path: result?.remotePath || '' }))
    } catch (error) {
      toast(t('连接失败：{message}', { message: errorMessage(error) }), 'err')
    } finally {
      setBusy(null)
    }
  }

  const save = async () => {
    if (!bridge?.saveCloudSync) return
    setBusy('save')
    try {
      const data = (await bridge.saveCloudSync(payloadOf(form))) as CloudSyncView | null | undefined
      if (data) {
        setView(data)
        setForm(formFromView(data))
      }
      setDirty(false)
      toast(t('✅ 云同步设置已保存'))
    } catch (error) {
      toast(t('保存失败：{message}', { message: errorMessage(error) }), 'err')
      return
    } finally {
      setBusy(null)
    }
    // 保存后自动测一次连接（与 cc-switch 同款：保存成功 ≠ 凭据可用）
    await test()
  }

  const fetchRemote = async (): Promise<RemoteInfo | null> => {
    if (!bridge?.getCloudSyncRemote) return null
    setBusy('remote')
    try {
      return (await bridge.getCloudSyncRemote()) as RemoteInfo
    } catch (error) {
      toast(t('读取远端信息失败：{message}', { message: errorMessage(error) }), 'err')
      return null
    } finally {
      setBusy(null)
    }
  }

  const beginUpload = async () => {
    if (dirty) {
      toast(t('请先保存设置再同步'), 'err')
      return
    }
    const info = await fetchRemote()
    if (!info) return
    setRemote(info.empty ? null : info)
    setDialog('upload')
  }

  const confirmUpload = async () => {
    if (!bridge?.cloudSyncUpload) return
    setDialog(null)
    setBusy('upload')
    try {
      const result = (await bridge.cloudSyncUpload()) as { bytes?: number; rows?: Record<string, number> } | null
      toast(t('✅ 已上传到云端（{size}）', { size: `${Math.round((Number(result?.bytes) || 0) / 1024)} KB` }))
      await load()
    } catch (error) {
      toast(t('上传失败：{message}', { message: errorMessage(error) }), 'err')
    } finally {
      setBusy(null)
    }
  }

  const beginDownload = async () => {
    if (dirty) {
      toast(t('请先保存设置再同步'), 'err')
      return
    }
    const info = await fetchRemote()
    if (!info) return
    if (info.empty) {
      toast(t('远端还没有可下载的同步数据'))
      return
    }
    if (info.compatible === false) {
      toast(info.incompatibleReason || t('远端数据与当前版本不兼容'), 'err')
      return
    }
    setRemote(info)
    if (info.keyRequired) {
      // 远端说用了另一把密钥，但文件却没传上来（旧版本上传 / 手删了文件）：
      // 这时输入密码也解不出来，直接说清楚，别让用户白输一次
      if (info.hasKeyArtifact === false) {
        toast(
          t('远端快照使用了另一把账号密钥，但没有携带密钥文件，无法在本机解密。请在原来那台机器上重新上传一次。'),
          'err'
        )
        return
      }
      setKeyPassword('')
      setDialog('key')
      return
    }
    setDialog('download')
  }

  const confirmDownload = async () => {
    if (!bridge?.cloudSyncDownload) return
    setDialog(null)
    setBusy('download')
    try {
      const result = (await bridge.cloudSyncDownload()) as { deviceName?: string; safetyBackup?: string } | null
      const note = result?.safetyBackup
        ? t('，替换前的数据已备份为 {name}', { name: result.safetyBackup })
        : ''
      toast(t('✅ 已从云端恢复{note}', { note }))
      await load()
      // 账号、配置、签到 / 余额记录都换了 —— 主界面（账号列表、导航计数）要重绘
      await shared().wbApp?.refresh?.()
    } catch (error) {
      toast(t('下载失败：{message}', { message: errorMessage(error) }), 'err')
    } finally {
      setBusy(null)
    }
  }

  const importKey = async () => {
    if (!bridge?.importCloudSyncKey) return
    if (!keyPassword) {
      toast(t('请输入同步密码'), 'err')
      return
    }
    setBusy('key')
    try {
      await bridge.importCloudSyncKey(keyPassword)
      setKeyPassword('')
      toast(t('✅ 已导入账号密钥，请继续下载'))
      const info = await fetchRemote()
      setDialog(null)
      if (info && !info.empty) {
        setRemote(info)
        setDialog('download')
      }
    } catch (error) {
      toast(t('导入密钥失败：{message}', { message: errorMessage(error) }), 'err')
    } finally {
      setBusy(null)
    }
  }

  /* ─── 渲染 ─────────────────────────────────────────────── */

  const status = view?.status || {}
  const showAutoError = Boolean(status.lastError) && status.lastErrorSource === 'auto'
  const lastSync = formatTime(status.lastSyncAt)

  return (
    <>
      <section className='panel'>
        <div className='panel-head'>
          <h2>{t('云同步')}</h2>
          <span
            className='tip-q'
            data-tip={t(
              '把账号、网关配置、自定义提供商、Agent 配置、代理池 / 订阅、签到与余额记录同步到 WebDAV 或 S3 兼容存储，换台机器后一键取回。事件日志、请求明细、调试报文与面板登录态不在同步范围内（它们是本机的运行痕迹）。上传的账号凭证仍是加密形态，解密用的账号密钥由「同步密码」单独加密后一并上传 —— 同步密码不会保存到云端，换机下载时需要重新输入。'
            )}
          ></span>
          <Badge variant={loadFailed ? 'destructive' : view?.enabled ? 'success' : 'outline'}>
            {loadFailed ? t('不可用') : view?.enabled ? t('已启用') : t('未启用')}
          </Badge>
          <div className='head-actions'>
            <Button variant='outline' disabled={disabled} onClick={() => void load(true)}>
              {t('刷新')}
            </Button>
          </div>
        </div>
        <div className='panel-body'>
          {loadFailed ? (
            <div className='hint'>{t('未能读取云同步设置，请稍后重试。')}</div>
          ) : (
            <>
              <div className='retention-list'>
                <div className='retention-row'>
                  <label>{t('服务商')}</label>
                  <div className='routing-row'>
                    <SegmentedControl
                      options={[
                        { value: PROVIDER_WEBDAV, label: 'WebDAV' },
                        { value: PROVIDER_S3, label: t('S3 兼容存储') },
                      ]}
                      value={form.provider}
                      disabled={disabled}
                      aria-label={t('服务商')}
                      onValueChange={value => patch({ provider: String(value) })}
                    />
                  </div>
                </div>

                {isWebDav ? (
                  <>
                    <div className='retention-row'>
                      <label>{t('预设')}</label>
                      <div className='routing-row'>
                        <Select
                          value={webdavPreset?.id || 'custom'}
                          disabled={disabled}
                          onValueChange={value => {
                            const preset = WEBDAV_PRESETS.find(item => item.id === value)
                            if (preset?.baseUrl) patch({ webdavBaseUrl: preset.baseUrl })
                          }}
                        >
                          <SelectTrigger className='min-w-[220px]'>
                            <SelectValue>{webdavPreset?.label || t('自定义')}</SelectValue>
                          </SelectTrigger>
                          <SelectContent>
                            {WEBDAV_PRESETS.map(item => (
                              <SelectItem key={item.id} value={item.id}>
                                {item.label}
                              </SelectItem>
                            ))}
                          </SelectContent>
                        </Select>
                      </div>
                    </div>
                    <div className='retention-row'>
                      <label htmlFor='cloud-webdav-url'>{t('服务器地址')}</label>
                      <Input
                        id='cloud-webdav-url'
                        value={form.webdavBaseUrl}
                        disabled={disabled}
                        placeholder='https://dav.example.com/dav/'
                        onChange={event => patch({ webdavBaseUrl: event.target.value })}
                      />
                    </div>
                    <div className='retention-row'>
                      <label htmlFor='cloud-webdav-user'>{t('用户名')}</label>
                      <Input
                        id='cloud-webdav-user'
                        value={form.webdavUsername}
                        disabled={disabled}
                        autoComplete='off'
                        onChange={event => patch({ webdavUsername: event.target.value })}
                      />
                    </div>
                    <div className='retention-row'>
                      <label htmlFor='cloud-webdav-password'>{t('密码')}</label>
                      <Input
                        id='cloud-webdav-password'
                        type='password'
                        value={form.webdavPassword}
                        disabled={disabled}
                        autoComplete='off'
                        placeholder={view?.webdav.hasPassword ? t('已保存，留空沿用') : t('第三方应用密码')}
                        onChange={event => patch({ webdavPassword: event.target.value })}
                      />
                    </div>
                    {webdavPreset?.hint ? <div className='hint'>{webdavPreset.hint}</div> : null}
                  </>
                ) : (
                  <>
                    <div className='retention-row'>
                      <label>{t('预设')}</label>
                      <div className='routing-row'>
                        <Select
                          value={s3Preset.id}
                          disabled={disabled}
                          onValueChange={value => {
                            const preset = S3_PRESETS.find(item => item.id === value)
                            if (preset) patch({ s3Region: preset.regionPlaceholder })
                          }}
                        >
                          <SelectTrigger className='min-w-[220px]'>
                            <SelectValue>{s3Preset.label}</SelectValue>
                          </SelectTrigger>
                          <SelectContent>
                            {S3_PRESETS.map(item => (
                              <SelectItem key={item.id} value={item.id}>
                                {item.label}
                              </SelectItem>
                            ))}
                          </SelectContent>
                        </Select>
                      </div>
                    </div>
                    <div className='retention-row'>
                      <label htmlFor='cloud-s3-region'>{t('区域（Region）')}</label>
                      <Input
                        id='cloud-s3-region'
                        value={form.s3Region}
                        disabled={disabled}
                        placeholder={s3Preset.regionPlaceholder}
                        onChange={event => patch({ s3Region: event.target.value })}
                      />
                    </div>
                    <div className='retention-row'>
                      <label htmlFor='cloud-s3-bucket'>{t('存储桶（Bucket）')}</label>
                      <Input
                        id='cloud-s3-bucket'
                        value={form.s3Bucket}
                        disabled={disabled}
                        onChange={event => patch({ s3Bucket: event.target.value })}
                      />
                    </div>
                    <div className='retention-row'>
                      <label htmlFor='cloud-s3-key'>{t('Access Key ID')}</label>
                      <Input
                        id='cloud-s3-key'
                        value={form.s3AccessKeyId}
                        disabled={disabled}
                        autoComplete='off'
                        onChange={event => patch({ s3AccessKeyId: event.target.value })}
                      />
                    </div>
                    <div className='retention-row'>
                      <label htmlFor='cloud-s3-secret'>{t('Secret Access Key')}</label>
                      <Input
                        id='cloud-s3-secret'
                        type='password'
                        value={form.s3SecretAccessKey}
                        disabled={disabled}
                        autoComplete='off'
                        placeholder={view?.s3.hasSecretKey ? t('已保存，留空沿用') : ''}
                        onChange={event => patch({ s3SecretAccessKey: event.target.value })}
                      />
                    </div>
                    <div className='retention-row'>
                      <label htmlFor='cloud-s3-endpoint'>{t('Endpoint')}</label>
                      <Input
                        id='cloud-s3-endpoint'
                        value={form.s3Endpoint}
                        disabled={disabled}
                        placeholder={t('留空使用 AWS 官方地址')}
                        onChange={event => patch({ s3Endpoint: event.target.value })}
                      />
                    </div>
                    {s3Preset.hint ? <div className='hint'>{s3Preset.hint}</div> : null}
                  </>
                )}

                <div className='retention-row'>
                  <label htmlFor='cloud-remote-root'>{t('远程根目录')}</label>
                  <Input
                    id='cloud-remote-root'
                    value={form.remoteRoot}
                    disabled={disabled}
                    placeholder='agent2api-sync'
                    onChange={event => patch({ remoteRoot: event.target.value })}
                  />
                </div>
                <div className='retention-row'>
                  <label htmlFor='cloud-profile'>{t('档案')}</label>
                  <Input
                    id='cloud-profile'
                    value={form.profile}
                    disabled={disabled}
                    placeholder='default'
                    onChange={event => patch({ profile: event.target.value })}
                  />
                </div>
                <div className='retention-row'>
                  <label htmlFor='cloud-sync-password'>{t('同步密码')}</label>
                  <Input
                    id='cloud-sync-password'
                    type='password'
                    value={form.syncPassword}
                    disabled={disabled}
                    autoComplete='new-password'
                    placeholder={view?.hasSyncPassword ? t('已保存，留空沿用') : t('至少 8 位，换机下载时需要')}
                    onChange={event => patch({ syncPassword: event.target.value })}
                  />
                  <div className='hint'>
                    {t(
                      '用于加密上云的账号密钥。请自己记牢 —— 它不保存到云端，换机下载时必须输入同一个密码，否则账号数据无法解密。'
                    )}
                  </div>
                </div>
                <div className='retention-row'>
                  <label htmlFor='cloud-auto-sync'>{t('自动同步')}</label>
                  <Switch
                    id='cloud-auto-sync'
                    checked={form.autoSync}
                    disabled={disabled || !form.enabled}
                    onCheckedChange={checked => patch({ autoSync: checked })}
                  />
                  <div className='hint'>
                    {t('本地账号、配置、签到与余额记录变化后自动上传（合并 1 秒内的连续改动，最长 10 秒传一次；内容没变时不发请求）。')}
                  </div>
                </div>
                <div className='retention-row'>
                  <label htmlFor='cloud-enabled'>{t('启用云同步')}</label>
                  <Switch
                    id='cloud-enabled'
                    checked={form.enabled}
                    disabled={disabled}
                    onCheckedChange={checked =>
                      patch({ enabled: checked, autoSync: checked ? form.autoSync : false })
                    }
                  />
                  <div className='hint'>{t('关闭后手动上传 / 下载也会被拒绝。')}</div>
                </div>
              </div>

              {dirty ? (
                <div className='hint'>{t('有未保存的改动：保存后才能同步（测试连接不受影响）。')}</div>
              ) : null}

              <div className='head-actions mt-3 flex flex-wrap gap-2'>
                <Button variant='outline' disabled={disabled} onClick={() => void test()}>
                  {busy === 'test' ? t('测试中…') : t('测试连接')}
                </Button>
                <Button variant='outline' disabled={disabled || !dirty} onClick={() => void save()}>
                  {busy === 'save' ? t('保存中…') : t('保存')}
                </Button>
                <Button
                  disabled={disabled || dirty || !view?.enabled}
                  onClick={() => void beginUpload()}
                >
                  {busy === 'upload' ? t('上传中…') : busy === 'remote' ? t('读取远端…') : t('上传到云端')}
                </Button>
                <Button
                  variant='secondary'
                  disabled={disabled || dirty || !view?.enabled}
                  onClick={() => void beginDownload()}
                >
                  {busy === 'download' ? t('下载中…') : t('从云端恢复')}
                </Button>
              </div>

              <div className='hint'>
                {lastSync
                  ? t('上次同步：{time}（{action}）', {
                      time: lastSync,
                      action: status.lastAction === 'download' ? t('下载') : t('上传'),
                    })
                  : t('还没有同步过。')}
                {view?.remotePath ? ` ${t('远端路径：{path}', { path: view.remotePath })}` : ''}
              </div>

              {showAutoError ? (
                <div className='danger-zone' id='cloud-sync-auto-error'>
                  <strong>{t('自动同步上次失败')}</strong>
                  <br />
                  {status.lastError}
                  <br />
                  {t('改动仍保存在本机，修复网络或凭据后下一次改动会自动重试，也可以手动点「上传到云端」。')}
                </div>
              ) : null}

              <div className='danger-zone'>
                <strong>{t('上传内容含加密后的账号凭证与网关配置')}</strong>
                {t(
                  '，以及代理池 / 订阅里可能存在的代理凭据。请使用只有你能访问的私有存储；账号密钥由同步密码单独加密，泄露存储本身不等于泄露账号。'
                )}
              </div>
            </>
          )}
        </div>
      </section>

      {/* ─── 上传确认 ─── */}
      <Dialog open={dialog === 'upload'} onOpenChange={next => { if (!next) setDialog(null) }}>
        <DialogContent className='w-[min(460px,calc(100vw-48px))]'>
          <DialogHeader>
            <DialogTitle>{t('上传到云端')}</DialogTitle>
          </DialogHeader>
          <DialogBody>
            <div>
              {t('本机的业务数据（账号、配置、Agent 配置、代理池 / 订阅、签到与余额记录）将上传并')}
              <strong>{t('覆盖')}</strong>
              {t('远端同一路径下的旧快照。事件日志、请求明细与调试报文不会上传。')}
            </div>
            <div className='hint mt-2'>
              {t('目标路径：{path}', { path: view?.remotePath || '' })}
            </div>
            {remote ? (
              <div className='danger-zone mt-3'>
                <strong>{t('远端已有一份快照，将被覆盖')}</strong>
                <br />
                {t('来自 {device}，生成于 {time}', {
                  device: remote.deviceName || t('未知设备'),
                  time: formatDateText(remote.createdAt),
                })}
              </div>
            ) : (
              <div className='hint mt-3'>{t('远端该路径下还没有快照，本次是首次上传。')}</div>
            )}
          </DialogBody>
          <DialogFooter>
            <Button variant='outline' onClick={() => setDialog(null)}>
              {t('取消')}
            </Button>
            <Button onClick={() => void confirmUpload()}>{t('确认上传')}</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* ─── 下载确认 ─── */}
      <Dialog open={dialog === 'download'} onOpenChange={next => { if (!next) setDialog(null) }}>
        <DialogContent className='w-[min(480px,calc(100vw-48px))]'>
          <DialogHeader>
            <DialogTitle>{t('从云端恢复')}</DialogTitle>
          </DialogHeader>
          <DialogBody>
            <div className='danger-zone'>
              {t('本机的账号、网关配置、Agent 配置、代理池 / 订阅、签到与余额记录将被远端快照')}
              <strong>{t('整体替换')}</strong>
              {t('。事件日志、请求明细、调试报文与面板登录态保持不变。替换前会自动把当前业务数据备份到配置目录的 backups/ 下。')}
            </div>
            <div className='mt-3 text-[12.5px]'>
              <div>{t('来源设备：{device}', { device: remote?.deviceName || t('未知设备') })}</div>
              <div>{t('生成时间：{time}', { time: formatDateText(remote?.createdAt) })}</div>
              <div>{t('应用版本：{version}', { version: remote?.appVersion || '—' })}</div>
              <div>{t('包含：{items}', { items: (remote?.artifacts || []).join(', ') || '—' })}</div>
            </div>
          </DialogBody>
          <DialogFooter>
            <Button variant='outline' onClick={() => setDialog(null)}>
              {t('取消')}
            </Button>
            <Button variant='destructive' onClick={() => void confirmDownload()}>
              {t('替换并恢复')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* ─── 同步密码（远端密钥与本机不一致时） ─── */}
      <Dialog open={dialog === 'key'} onOpenChange={next => { if (!next) setDialog(null) }}>
        <DialogContent className='w-[min(440px,calc(100vw-48px))]'>
          <DialogHeader>
            <DialogTitle>{t('输入同步密码')}</DialogTitle>
          </DialogHeader>
          <DialogBody>
            <div>
              {t(
                '远端快照使用的账号密钥与本机不同。请输入上传那台机器设置的同步密码，用它解出密钥装到本机后再继续下载。'
              )}
            </div>
            <div className='hint mt-2'>
              {t('导入前会把本机的 account.key 改名备份（account.key.bak-时间戳），不会直接删除。')}
            </div>
            <Label htmlFor='cloud-sync-key-password' className='mt-3 mb-[5px] block text-[12.5px]'>
              {t('同步密码')}
            </Label>
            <Input
              id='cloud-sync-key-password'
              type='password'
              value={keyPassword}
              autoComplete='off'
              onChange={event => setKeyPassword(event.target.value)}
              onKeyDown={event => {
                if (event.key === 'Enter') void importKey()
              }}
            />
          </DialogBody>
          <DialogFooter>
            <Button variant='outline' onClick={() => setDialog(null)}>
              {t('取消')}
            </Button>
            <Button disabled={busy === 'key'} onClick={() => void importKey()}>
              {busy === 'key' ? t('导入中…') : t('导入密钥并继续')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}
