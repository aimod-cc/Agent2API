import * as React from 'react'
import {
  Alert, Badge, Button, Card, CardContent, CardHeader,
  Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle,
  Input, Label, Switch,
} from '@ui'
import { NOTES, TIPS } from './settings-model'
import {
  cancelDbExport, confirmDbExport, confirmDbRestore, exportDatabase,
  resolveDbBackupConfirm, toggleDbIncludeLogs, useSettings,
} from './settings-state'
import { t } from '../i18n'

export function DatabaseBackupPanel(): React.ReactElement {
  const snap = useSettings()
  const busy = snap.busy === 'dbExport' || snap.busy === 'dbImport' || snap.dbRestore?.active === true
  return (
    <Card render={<section />} className='panel'>
      <CardHeader className='panel-head'>
        <h2>{t('整库备份 / 恢复')}</h2>
        <span className='tip-q' data-tip={TIPS.backup} />
        <Badge variant='outline'>{t('密码加密 · 包含恢复密钥')}</Badge>
        <div className='head-actions'>
          <Button id='btn-settings-db-export' variant='outline' disabled={busy} onClick={confirmDbExport}>
            {snap.busy === 'dbExport' ? t('备份中…') : t('备份整库')}
          </Button>
          <Button id='btn-settings-db-restore' variant='destructive' disabled={busy} onClick={confirmDbRestore}>
            {snap.busy === 'dbImport' ? t('恢复中…') : t('恢复整库')}
          </Button>
        </div>
      </CardHeader>
      <CardContent className='panel-body'>
        {snap.dbWarning ? (
          <Alert variant='destructive' className='danger-zone' id='settings-db-warning'>
            <strong>{t('⚠️ 密钥警告')}</strong><br />{snap.dbWarning}
          </Alert>
        ) : null}
        <div className='retention-row'>
          <Label htmlFor='settings-db-include-logs'>{t('包含日志与调试报文')}</Label>
          <Switch id='settings-db-include-logs' checked={snap.dbIncludeLogs} disabled={snap.busy !== null || busy} onCheckedChange={toggleDbIncludeLogs} />
          <div className='hint'>
            {t('取消勾选后只备份业务数据（账号、配置与余额记录），文件显著变小；事件日志、请求统计与调试报文不会进备份文件。')}
          </div>
        </div>
        <p className='hint'>
          {t('一个 .a2ab 文件即可跨机器恢复，账号、网关 Key 与云同步凭据无需另行复制密钥。插件程序与库外文件不在整库备份范围。')}
        </p>
        <Alert variant='destructive' className='danger-zone'>{NOTES.backupDanger}</Alert>
      </CardContent>
    </Card>
  )
}

export function DatabaseBackupDialogs(): React.ReactElement {
  const snap = useSettings()
  return (
    <>
      {snap.dbExportConfirm ? <BackupPasswordDialog mode='export' /> : null}
      {snap.dbBackupConfirm ? <BackupPasswordDialog mode='restore' /> : null}
    </>
  )
}

function BackupPasswordDialog({ mode }: { mode: 'export' | 'restore' }): React.ReactElement {
  const passwordRef = React.useRef<HTMLInputElement>(null)
  const repeatRef = React.useRef<HTMLInputElement>(null)
  const cancelRef = React.useRef<HTMLButtonElement>(null)
  const [error, setError] = React.useState('')
  const exporting = mode === 'export'
  const clear = () => {
    if (passwordRef.current) passwordRef.current.value = ''
    if (repeatRef.current) repeatRef.current.value = ''
  }
  const close = () => {
    clear()
    if (exporting) cancelDbExport()
    else resolveDbBackupConfirm(false)
  }
  const submit = () => {
    const password = passwordRef.current?.value ?? ''
    if (new TextEncoder().encode(password).length > 1024) {
      setError(t('备份密码不能超过 1024 字节'))
      return
    }
    if (exporting && Array.from(password).length < 12) {
      setError(t('备份密码至少需要 12 位，建议使用较长且不重复的密码'))
      return
    }
    if (exporting && password !== repeatRef.current?.value) {
      setError(t('两次输入的备份密码不一致'))
      return
    }
    clear()
    if (exporting) void exportDatabase(password)
    else resolveDbBackupConfirm(true, password)
  }
  return (
    <Dialog open onOpenChange={next => { if (!next) close() }}>
      <DialogContent className='w-[min(480px,calc(100vw-48px))]' initialFocus={cancelRef}>
        <DialogHeader>
          <DialogTitle>{exporting ? t('设置备份密码') : t('恢复整库备份')}</DialogTitle>
        </DialogHeader>
        <DialogBody>
          {exporting ? (
            <Alert>{t('数据库与恢复密钥一起加密保存。密码不会保存在应用或备份中，忘记后无法恢复，请妥善保管。')}</Alert>
          ) : (
            <Alert variant='destructive'>
              {t('本机全部数据会被备份整体替换，恢复前自动保留安全备份，完成后需要重新登录管理面板。本机已有密钥不会被覆盖。')}
            </Alert>
          )}
          <form id={`db-${mode}-password-form`} onSubmit={event => { event.preventDefault(); submit() }}>
            <Label htmlFor={`db-${mode}-password`} className='mt-3 mb-1 block'>{t('备份密码')}</Label>
            <Input id={`db-${mode}-password`} ref={passwordRef} type='password' autoComplete={exporting ? 'new-password' : 'off'} maxLength={1024} placeholder={exporting ? t('至少 12 位') : t('加密备份必填，旧 SQL 可留空')} onChange={() => setError('')} />
            {exporting ? (
              <>
                <Label htmlFor='db-export-repeat' className='mt-3 mb-1 block'>{t('确认备份密码')}</Label>
                <Input id='db-export-repeat' ref={repeatRef} type='password' autoComplete='new-password' maxLength={1024} onChange={() => setError('')} />
              </>
            ) : (
              <p className='hint mt-2'>{t('支持 .a2ab 加密备份和旧 .sql 文件；旧 SQL 不包含恢复密钥，跨机器导入仍需原机密钥。')}</p>
            )}
            {error ? <p role='alert' className='mt-2 text-destructive'>{error}</p> : null}
          </form>
        </DialogBody>
        <DialogFooter>
          <Button ref={cancelRef} variant='outline' onClick={close}>{t('取消')}</Button>
          <Button variant={exporting ? 'default' : 'destructive'} type='submit' form={`db-${mode}-password-form`}>
            {exporting ? t('选择位置并加密备份') : t('选择备份文件并恢复')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
