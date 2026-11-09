import * as React from 'react'
import { Button, Input, Field, FieldLabel } from '@ui'
import { AddFoot, AddGroup } from './add-account-kit'
import { MAX_NAME_LENGTH, type ProviderConfig } from './add-account-configs'
import { afterAdd, addedLabelOf, describeError, draftProps, postAccount, readField, toast, type AccountRecord } from './add-account-bridge'
import { readFileAsDataUrl } from '../../../ui-shared/file-read'
import { accountFilePayload, isAccountExport } from './add-account-file'
import { Note, runAdd } from './add-provider-common'
import { useAddAccountProxy, useAccountAddBusy } from './add-account-proxy'
import { t } from '../i18n'

/* ─── 导入凭证文件（通用版；WorkBuddy 那段是另一套 payload，不要合并）─── */


function base64BodyOf(dataUrl: string): string {
  const comma = dataUrl.indexOf(',')
  return comma >= 0 ? dataUrl.slice(comma + 1) : dataUrl
}

/**
 * 「导入凭证文件」段（通用）：用户手选桌面客户端的凭证文件，前端读内容
 * （文本 → `authFileContent`；二进制 → `authFileB64`）交给网关解析落库。
 *
 * 与 `DesktopSection` 的差别：那边只有一颗按钮、读的是本机固定路径；这边选
 * 文件、文件可来自任意机器 / 备份，落库后是**可独立转发**的账号。
 * WorkBuddy 的同类段落不并进来：它把文件内容**摊平**成请求体字段
 * （credentials 直读顶层键），payload 形态与这里的三字段契约不同。
 */
export function FileImportSection({
  config,
  visible,
  footerHost,
  userAgent = '',
}: {
  config: ProviderConfig
  visible: boolean
  footerHost: HTMLElement | null
  /** 账号级自定义 User-Agent（草稿在 ProviderBlock 一级；留空 = 用该家默认） */
  userAgent?: string
}): React.ReactElement | null {
  const [busy, setBusy] = useAccountAddBusy()
  const proxy = useAddAccountProxy()
  // 选中文件名回显：KukuAI 的 Cookies 没有扩展名，按钮旁看不出选的是哪一份
  const [hint, setHint] = React.useState('')
  const fileInput = React.useRef<HTMLInputElement | null>(null)
  const spec = config.fileImport
  // 备注名走草稿机制（与 ManualSection 的 name 字段同一形态）
  const nameId = `${config.provider}-file-name`
  if (!spec) return null

  async function importFile(file: File): Promise<void> {
    const fileImport = config.fileImport
    if (!fileImport) return
    await runAdd(setBusy, async () => {
      try {
        const remark = readField(nameId)
        const payload: Record<string, unknown> = {
          provider: config.provider,
          // 文件名只用于错误文案（后端会把原文件名带上），不参与解析
          authFileName: file.name,
        }
        if (fileImport.binary) {
          const prefix = await file.slice(0, 64).text()
          if (prefix.trimStart().startsWith('{')) {
            const parsed: unknown = JSON.parse(await file.text())
            if (!isAccountExport(parsed)) {
              throw new Error(t('请选择客户端凭证文件，或本软件导出的单账号 JSON 文件'))
            }
            Object.assign(payload, accountFilePayload(parsed, config.provider, remark))
          } else {
            payload.authFileB64 = base64BodyOf(await readFileAsDataUrl(file, t('文件读取失败')))
          }
        } else {
          const text = await file.text()
          // JSON 预检只挡「选错文件」这一层：解析失败 / 不是对象在这里就变成
          // 人话，而不是后端凭据模块吐出来的一条远端错误（与 WorkBuddy 同取向）
          let parsed: unknown
          try {
            parsed = JSON.parse(text)
          } catch {
            toast(t('选择的文件不是合法 JSON'), 'err')
            return
          }
          if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
            toast(t('不是有效的凭证文件：内容要是一个 JSON 对象'), 'err')
            return
          }
          if (isAccountExport(parsed)) {
            Object.assign(payload, accountFilePayload(parsed, config.provider, remark))
          } else {
            // 原生文件由提供商解析；软件导出文件走统一回流，保留凭证附加字段。
            payload.authFileContent = text
          }
        }
        if (remark) payload.name = remark
        // 账号文件（导出回流）自带 UA 时不覆盖用户这次填的；凭证文件导入没有
        // 这个字段，用弹窗里填的那份。
        if (userAgent.trim() && payload.userAgent === undefined) payload.userAgent = userAgent.trim()
        Object.assign(payload, isAccountExport(payload) ? proxy.importFields() : { proxy: proxy.read() })
        const data = (await postAccount(payload)) as { account?: AccountRecord } | null
        setHint('')
        await afterAdd(addedLabelOf(data?.account), config.label, data)
      } catch (error) {
        // 文件本身的问题（缺字段 / 密文解不开 / 凭证无效）后端都给 400 + 带文件名的
        // 原因，原样透出即可
        toast(t('导入失败：{reason}', { reason: describeError(error) }), 'err')
      }
    })
  }

  return (
    <AddGroup label={t('导入凭证文件')} hidden={!visible}>
      <Note text={spec.note} />
      <p className='detail'>{t('也支持本软件导出的单账号 JSON 文件，保留账号凭证与附加信息。')}</p>
      <Field orientation='horizontal' className='field-row'>
        <FieldLabel className='lb-inline' htmlFor={nameId}>{t('备注名')}</FieldLabel>
        <Input
          id={nameId}
          type='text'
          maxLength={MAX_NAME_LENGTH}
          placeholder={t('可选，留空则用文件里的昵称')}
          {...draftProps(nameId)}
        />
      </Field>
      <Input
        ref={fileInput}
        type='file'
        // 空串 = 选择器不过滤（KukuAI 的 Cookies 没有扩展名，无从按类型筛）
        accept={spec.accept}
        hidden
        onChange={event => {
          const file = event.target.files?.[0]
          // 清空 value：同一份文件改内容后再选一次也要能触发 onChange
          event.target.value = ''
          if (file) {
            setHint(file.name)
            void importFile(file)
          }
        }}
      />
      <AddFoot host={footerHost} active={visible} note={<span className='detail'>{hint}</span>}>
        <Button onClick={() => fileInput.current?.click()} disabled={busy}>
          {busy ? t('导入中…') : t('选择凭证文件导入')}
        </Button>
      </AddFoot>
    </AddGroup>
  )
}

