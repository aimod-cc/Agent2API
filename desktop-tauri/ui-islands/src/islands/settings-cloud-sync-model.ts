import { t } from '../i18n'

export type SyncStatus = {
  lastSyncAt?: number
  lastAction?: string
  lastSnapshotId?: string
  lastLocalHash?: string
  lastKeyFingerprint?: string
  lastError?: string
  lastErrorSource?: string
}

export type CloudSyncView = {
  provider: string
  webdav: { baseUrl: string; username: string; hasPassword: boolean; userAgent: string }
  s3: {
    region: string
    bucket: string
    accessKeyId: string
    endpoint: string
    hasSecretKey: boolean
    userAgent: string
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

export type RemoteInfo = {
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

export type Form = {
  provider: string
  webdavBaseUrl: string
  webdavUsername: string
  webdavPassword: string
  webdavUserAgent: string
  s3Region: string
  s3Bucket: string
  s3AccessKeyId: string
  s3SecretAccessKey: string
  s3Endpoint: string
  s3UserAgent: string
  syncPassword: string
  enabled: boolean
  autoSync: boolean
  remoteRoot: string
  profile: string
}

export const PROVIDER_WEBDAV = 'webdav'
export const PROVIDER_S3 = 's3'

export const WEBDAV_PRESETS = [
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

export const S3_PRESETS = [
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

export function formatTime(ms?: number): string {
  if (!ms) return ''
  try { return new Date(ms).toLocaleString() }
  catch { return String(ms) }
}

export function formatDateText(raw?: string): string {
  if (!raw) return '—'
  const parsed = new Date(raw)
  return Number.isNaN(parsed.getTime()) ? raw : parsed.toLocaleString()
}

export function emptyForm(): Form {
  return {
    provider: PROVIDER_WEBDAV,
    webdavBaseUrl: '', webdavUsername: '', webdavPassword: '', webdavUserAgent: '',
    s3Region: '', s3Bucket: '', s3AccessKeyId: '', s3SecretAccessKey: '', s3Endpoint: '', s3UserAgent: '',
    syncPassword: '', enabled: false, autoSync: false, remoteRoot: 'agent2api-sync', profile: 'default',
  }
}

export function formFromView(view: CloudSyncView): Form {
  return {
    provider: view.provider === PROVIDER_S3 ? PROVIDER_S3 : PROVIDER_WEBDAV,
    webdavBaseUrl: view.webdav.baseUrl,
    webdavUsername: view.webdav.username,
    // 密码不回显：留空 = 沿用后端已保存的那份。
    webdavPassword: '',
    webdavUserAgent: view.webdav.userAgent,
    s3Region: view.s3.region,
    s3Bucket: view.s3.bucket,
    s3AccessKeyId: view.s3.accessKeyId,
    s3Endpoint: view.s3.endpoint,
    s3SecretAccessKey: '',
    s3UserAgent: view.s3.userAgent,
    syncPassword: '',
    enabled: view.enabled,
    autoSync: view.autoSync,
    remoteRoot: view.remoteRoot,
    profile: view.profile,
  }
}

export function payloadOf(form: Form): Record<string, unknown> {
  return {
    provider: form.provider,
    webdavBaseUrl: form.webdavBaseUrl,
    webdavUsername: form.webdavUsername,
    webdavPassword: form.webdavPassword,
    webdavUserAgent: form.webdavUserAgent,
    s3Region: form.s3Region,
    s3Bucket: form.s3Bucket,
    s3AccessKeyId: form.s3AccessKeyId,
    s3SecretAccessKey: form.s3SecretAccessKey,
    s3Endpoint: form.s3Endpoint,
    s3UserAgent: form.s3UserAgent,
    syncPassword: form.syncPassword,
    enabled: form.enabled,
    autoSync: form.autoSync,
    remoteRoot: form.remoteRoot,
    profile: form.profile,
  }
}
