import * as React from 'react'
import {
  Badge,
  Button,
  Card,
  CardContent,
  CardHeader,
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Input,
  Label,
  Switch,
} from '@ui'
import { t } from '../i18n'
import { TIPS } from './settings-model'
import {
  cancelLanRegister,
  resolveLanConfirm,
  submitLanRegister,
  toggleLan,
  toggleLanPanel,
  type LanConfirm,
  type LanRegister,
  type SettingsSnapshot,
} from './settings-state'

export function NetworkAccessPanel({ snap }: { snap: SettingsSnapshot }) {
  if (snap.panelLogin) return null
  const { app } = snap
  const ready = app.status === 'ready'
  const disabled = !ready || snap.busy === 'lan'
  const base = app.lanIp && app.port ? `http://${app.lanIp}:${app.port}` : null

  return (
    <Card render={<section />} className='panel'>
      <CardHeader className='panel-head'>
        <h2>{t('网络访问')}</h2>
        <span className='tip-q' data-tip={TIPS.networkAccess} />
        <Badge variant={!ready ? (app.status === 'unavailable' ? 'destructive' : 'outline') : app.lanAccess ? 'success' : 'outline'}>
          {!ready ? (app.status === 'unavailable' ? t('不可用') : t('检测中…')) : app.lanAccess ? t('已开启') : t('仅本机')}
        </Badge>
      </CardHeader>
      <CardContent className='panel-body'>
        <div className='settings-switches'>
          <Label className='switch'>
            <Switch id='settings-lan-access' checked={app.lanAccess} disabled={disabled} onCheckedChange={toggleLan} />
            <span>{t('允许其他设备访问网关')}</span>
          </Label>
          <Label className='switch'>
            <Switch id='settings-lan-panel' checked={app.lanPanel} disabled={disabled || !app.lanAccess} onCheckedChange={toggleLanPanel} />
            <span>{t('同时开放网页管理面板')}</span>
          </Label>
        </div>
        <div className='settings-state'>
          {!ready
            ? app.status === 'unavailable' ? t('未能读取网络访问设置，请稍后重试') : t('检测中…')
            : app.lanAccess
              ? t('已开启：监听 0.0.0.0，允许其他设备访问。')
              : t('未开启：网关只监听 127.0.0.1，仅本机可以访问。')}
        </div>
        {ready && app.lanAccess ? (
          <div className='mt-3 space-y-2 text-sm'>
            {base ? (
              <>
                <div className='flex flex-wrap items-baseline gap-x-2 gap-y-1'>
                  <span className='text-muted-foreground'>{t('本机 API 地址')}</span>
                  <code className='break-all'>{base}/v1</code>
                </div>
                {app.lanPanel ? (
                  <div className='flex flex-wrap items-baseline gap-x-2 gap-y-1'>
                    <span className='text-muted-foreground'>{t('网页管理地址')}</span>
                    <code className='break-all'>{base}</code>
                  </div>
                ) : null}
                <div className='hint'>{t('访问时请使用目标设备能连接到的地址；这里显示的本机地址不代表公网可达。')}</div>
              </>
            ) : (
              <div className='hint'>{t('暂未获取到本机访问地址，请使用其他设备能连接到的本机 IP 与网关端口。')}</div>
            )}
          </div>
        ) : null}
        <div className='hint retention-note'>
          {t('开启后监听所有 IPv4 网卡（0.0.0.0），并不局限于局域网，也不会自动开放到公网；实际访问范围取决于网络与防火墙设置。修改后需要重启应用。')}
        </div>
      </CardContent>
    </Card>
  )
}

const NETWORK_CONFIRM_COPY: Record<
  NonNullable<LanConfirm>['mode'],
  { title: string; body: (panel: boolean) => React.ReactNode; label: string }
> = {
  enable: {
    title: t('开启网络访问'),
    body: () => (
      <>
        {t('开启后网关将监听所有 IPv4 网卡（0.0.0.0），访问范围不再限于本机，也不局限于局域网。是否能从公网访问取决于网络与防火墙设置。')}<br />
        {t('开启前需要注册面板管理员，已注册会跳过此步骤。管理接口需要管理员会话或网关 Key；API 调用需要启用的网关 Key，没有时会自动创建一把「默认」Key。')}<br />
        <strong>{t('保存后应用将重启以生效。')}</strong>{t('确定继续？')}
      </>
    ),
    label: t('继续'),
  },
  disable: {
    title: t('关闭网络访问'),
    body: () => (
      <>
        {t('关闭后网关只监听 127.0.0.1，其他设备将无法直接连接，网页管理面板也会一并关闭。已配置的网关 Key 与账号不受影响。')}<br />
        <strong>{t('保存后应用将重启以生效。')}</strong>{t('确定继续？')}
      </>
    ),
    label: t('关闭并重启'),
  },
  panel: {
    title: t('变更网页管理面板'),
    body: panel => (
      <>
        {panel
          ? t('开启后，能连接到网关的设备可通过浏览器访问网页管理面板，需使用管理员账号登录。访问范围与网关相同，并不局限于局域网。')
          : t('关闭后不再提供网页管理面板，本机桌面程序仍可正常管理；已开启的 API 转发不受影响。')}<br />
        <strong>{t('保存后应用将重启以生效。')}</strong>{t('确定继续？')}
      </>
    ),
    label: t('保存并重启'),
  },
}

export function NetworkAccessDialog({ confirm, register }: { confirm: LanConfirm; register: LanRegister }) {
  const cancelRef = React.useRef<HTMLButtonElement | null>(null)
  const nameRef = React.useRef<HTMLInputElement | null>(null)
  const passwordRef = React.useRef<HTMLInputElement | null>(null)
  const open = confirm !== null || register !== null
  const close = () => {
    if (register !== null) cancelLanRegister()
    else resolveLanConfirm(false)
  }
  const submitRegister = () => {
    submitLanRegister(nameRef.current?.value ?? '', passwordRef.current?.value ?? '')
  }

  return (
    <Dialog open={open} onOpenChange={next => { if (!next) close() }}>
      <DialogContent className='w-[min(480px,calc(100vw-48px))]' initialFocus={cancelRef}>
        {register !== null ? (
          <>
            <DialogHeader>
              <DialogTitle>{t('注册管理员账号')}</DialogTitle>
            </DialogHeader>
            <DialogBody>
              <div>{t('允许其他设备访问前，需要设置面板管理员账号与密码。注册完成后会继续开启流程并重启应用。')}</div>
              <form onSubmit={event => { event.preventDefault(); if (!register.busy) submitRegister() }}>
                <Label htmlFor='lan-admin-name' className='mt-3.5 mb-[5px] block text-[12.5px]'>{t('账号')}</Label>
                <Input ref={nameRef} id='lan-admin-name' autoComplete='username' placeholder={t('管理员账号')} disabled={register.busy} />
                <Label htmlFor='lan-admin-password' className='mt-3.5 mb-[5px] block text-[12.5px]'>{t('密码')}</Label>
                <Input
                  ref={passwordRef}
                  id='lan-admin-password'
                  type='password'
                  autoComplete='new-password'
                  placeholder={t('至少 8 位')}
                  disabled={register.busy}
                />
              </form>
              <div className='mt-3 min-h-5 text-[13px] whitespace-pre-wrap text-destructive'>{register.error}</div>
            </DialogBody>
            <DialogFooter>
              <div className='mr-auto' />
              <Button ref={cancelRef} variant='outline' onClick={cancelLanRegister} disabled={register.busy}>
                {t('取消')}
              </Button>
              <Button onClick={submitRegister} disabled={register.busy}>
                {register.busy ? t('正在注册…') : t('注册并开启')}
              </Button>
            </DialogFooter>
          </>
        ) : (
          <>
            <DialogHeader>
              <DialogTitle>{confirm ? NETWORK_CONFIRM_COPY[confirm.mode].title : ''}</DialogTitle>
            </DialogHeader>
            <DialogBody>
              <div>{confirm ? NETWORK_CONFIRM_COPY[confirm.mode].body(confirm.panel) : null}</div>
            </DialogBody>
            <DialogFooter>
              <div className='mr-auto' />
              <Button ref={cancelRef} variant='outline' onClick={() => resolveLanConfirm(false)}>{t('取消')}</Button>
              <Button onClick={() => resolveLanConfirm(true)}>
                {confirm ? NETWORK_CONFIRM_COPY[confirm.mode].label : t('继续')}
              </Button>
            </DialogFooter>
          </>
        )}
      </DialogContent>
    </Dialog>
  )
}
