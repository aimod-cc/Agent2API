/**
 * Agent2API · 自定义提供商的**余额查询方案**界面元数据（六种，与后端
 * `providers::custom::balance` 的 `Scheme` 一一对应）。
 *
 * 它**不是岛**（不向 window 注册任何接口）：只是被几个岛（账号设置弹窗、
 * 新增账号表单）共用的数据与组件模块 —— `.tsx` 后缀是因为里面有一段 JSX，
 * `index.tsx` 的 glob 会顺带 import 它，那是无副作用的空转。
 *
 * ── 为什么要这一张表 ────────────────────────────────────────
 * 方案是「这一家是谁」的派生事实（预置卡片在创建时把它写进提供商记录），
 * 用户**不选方案**；界面要回答的是另一个问题：这个方案需要用户补哪些凭证、
 * 每个框怎么填。那件事只有前端知道（标签 / 占位 / 提示都是文案），
 * 后端只认「五个字段键」这一层契约（`custom_accounts::BALANCE_QUERY_FIELDS`）。
 *
 * ── 什么不用补 ──────────────────────────────────────────────
 * 四条只用**账号自己的 API Key**（sub2api / DeepSeek / OpenCode Go / 智谱国际
 * 站）—— 它们的 `fields` 是空数组，界面只显示一句方案说明（用户「DeepSeek 不用
 * 额外填东西」的口径）；新增账号与「查询设置」两处都照这个数组渲染，不写死分支。
 *
 * ── 两个调用点的绑定方式不同（说清楚免得改坏）────────────────
 *   · 新增账号表单（`add-custom-provider.tsx`）：输入框是**非受控**的
 *     （草稿存在模块级 draft 里，提交时按 DOM id 现读），因此那边自己用
 *     `balanceFieldId` 拼 id、自己写草稿；本文件的 `BalanceQuerySection`
 *     只被「账号设置」弹窗用（那是受控的 React 状态）。
 *   · 值只认两种：**非空 = 要写入**、**空 = 不修改**（清除走显式按钮，
 *     传 null 给后端）—— 与 apiKey / 余额凭证那一套语义逐字一致。
 */

import * as React from 'react'
import { Button, Input } from '@ui'

import { t } from '../i18n'

/** 方案 id（与后端 `custom_providers::BALANCE_SCHEME_*` 逐字一致，勿改拼写） */
export type BalanceScheme =
  | 'sub2api'
  | 'new_api'
  | 'deepseek'
  | 'opencode'
  | 'sensenova'
  | 'zhipu'
  | 'zhipu_zai'

/** 记录键（与后端 `custom_accounts::BALANCE_QUERY_FIELDS` 逐字一致） */
export type BalanceFieldKey = 'userId' | 'accessToken' | 'username' | 'password' | 'token'

/** 账号公开形态里的 `balanceQuery`（后端注入；只有登记了方案的家才有这个键） */
export type BalanceQueryPublic = {
  scheme?: string
  userId?: string
  username?: string
  /** 三个密字段**只有存在标记**，值绝不透出（与 apiKey 同一纪律） */
  hasAccessToken?: boolean
  hasPassword?: boolean
  hasToken?: boolean
}

export type BalanceField = {
  key: BalanceFieldKey
  label: string
  /** 密字段：输入框遮显、留空 = 不修改、清除走按钮（传 null） */
  secret?: boolean
  placeholder: string
  hint?: string
}

export type BalanceSchemeSpec = {
  /** 展示名（与后端 `Scheme::label` 同口径） */
  label: string
  /** 「打哪里」（表单里那行小字，与 OmniProxy 的提示位置一致） */
  endpoint: string
  /** 方案说明（中性提示；讲清要填什么、去哪拿） */
  note: string
  /** 这个方案要用户补的凭证（空数组 = 只用账号自己的 API Key） */
  fields: BalanceField[]
}

/**
 * 七个方案的界面定义。文案移植自 OmniProxy 的网关词典（`balance_*` 一批键），
 * 措辞按本项目的口径微调（"提供商设置" → 「账号设置 → 查询设置」）；
 * 智谱国际站（zhipu_zai）是本项目自研的补充，OmniProxy 没有这一条。
 */
export const BALANCE_SCHEME_SPECS: Record<BalanceScheme, BalanceSchemeSpec> = {
  sub2api: {
    label: 'Sub2API',
    endpoint: '{base}/v1/usage',
    note: t('用这条账号的 API Key 查询，不需要额外配置。'),
    fields: [],
  },
  new_api: {
    label: 'NewAPI',
    endpoint: '{base}/api/user/self + /api/status',
    note: t('用 NewAPI 站点的个人访问令牌查询账户余额（不是这条账号的上游 API Key）。令牌在站点的「个人设置」里生成，只有旧版 / 定制站点才需要额外填用户 ID。'),
    fields: [
      {
        key: 'accessToken',
        label: t('NewAPI 个人访问令牌'),
        secret: true,
        placeholder: t('在 NewAPI 站点「个人设置」里生成的访问令牌'),
        hint: t('必填；留空 = 不修改，清除用右边的按钮'),
      },
      {
        key: 'userId',
        label: t('NewAPI 用户 ID'),
        placeholder: t('旧版 / 定制站点才需要，新版可留空'),
        hint: t('可留空：新版请求头只认令牌；旧版会把用户 ID 放进 New-Api-User 头'),
      },
    ],
  },
  deepseek: {
    label: 'DeepSeek',
    endpoint: '{base}/user/balance',
    note: t('用这条账号的 API Key 查询，不需要额外配置。'),
    fields: [],
  },
  opencode: {
    label: 'OpenCode Go',
    endpoint: '{base}/v1/usage',
    note: t('用这条账号的 API Key 查询订阅用量，不需要额外配置；读数展开成 5 小时 / 每周 / 每月三个窗口。'),
    fields: [],
  },
  sensenova: {
    label: '商汤日日新',
    endpoint: 'platform.sensenova.cn 控制台',
    note: t('用商汤日日新控制台的账号密码查询（与推理用的 API Key 不是同一套）。网关会自动换取登录票并缓存，过期后自动重登，不需要手动维护。'),
    fields: [
      {
        key: 'username',
        label: t('账号'),
        placeholder: t('商汤日日新控制台的登录账号'),
        hint: t('必填'),
      },
      {
        key: 'password',
        label: t('密码'),
        secret: true,
        placeholder: t('商汤日日新控制台的登录密码'),
        hint: t('留空 = 不修改，清除用右边的按钮'),
      },
    ],
  },
  zhipu: {
    label: '智谱',
    endpoint: 'bigmodel.cn 控制台',
    note: t('用 bigmodel.cn 控制台的访问令牌查询账户余额（人民币元）。令牌是登录后浏览器 Cookie `bigmodel_token_production` 的值：开发者工具 → 应用（Application）→ Cookie 里复制。'),
    fields: [
      {
        key: 'token',
        label: t('访问令牌'),
        secret: true,
        placeholder: t('bigmodel_token_production 的值'),
        hint: t('必填；留空 = 不修改，清除用右边的按钮'),
      },
    ],
  },
  // 智谱国际站（z.ai）：同一条账户报告接口，但鉴权不同 —— 控制台类接口只认
  // 「API Key 铸 SIGN 签名」（后端 `zhipu_sign_jwt`），裸 Key / 控制台令牌都进不去。
  // 签名原料就是转发要填的那把 API Key，所以这一支**没有任何输入框**（deepseek 同款）。
  zhipu_zai: {
    label: '智谱（z.ai 国际）',
    endpoint: 'api.z.ai 控制台',
    note: t('用这条账号的 API Key 生成签名后查询账户余额（美元），不需要额外配置。要求 API Key 是 z.ai 的「id.secret」形态（控制台生成的 Key 就是）。'),
    fields: [],
  },
}

/** 方案 → 界面定义（未知 / 缺省给 null = 不渲染这一段） */
export function balanceSpecOf(scheme: string | undefined | null): BalanceSchemeSpec | null {
  if (!scheme) return null
  return (BALANCE_SCHEME_SPECS as Record<string, BalanceSchemeSpec | undefined>)[scheme] ?? null
}

/** 某个密字段是否已配置（读公开形态的 `has*` 标记） */
export function balanceFieldConfigured(
  query: BalanceQueryPublic | null | undefined,
  field: BalanceField,
): boolean {
  if (!field.secret) return false
  if (field.key === 'accessToken') return query?.hasAccessToken === true
  if (field.key === 'password') return query?.hasPassword === true
  if (field.key === 'token') return query?.hasToken === true
  return false
}

/**
 * 该方案的查询凭证是否**齐了**（齐了才会真去查；不齐后端回「未配置」，
 * 余额列显示中性提示而不是红色失败）。
 *
 * 与后端各方案的必填判据逐条对齐（`balance.rs` 的各处早退）：
 *   · sub2api / deepseek / opencode / zhipu_zai → 账号有 API Key（那是账号凭证，不在本段）；
 *   · new_api → 访问令牌；sensenova → 账号 + 密码；zhipu → 令牌。
 *   商汤的登录票缓存（后端记录里）不进这一判断：它过期后自动重登。
 */
export function balanceQueryConfigured(query: BalanceQueryPublic | null | undefined): boolean {
  const scheme = String(query?.scheme || '')
  if (scheme === 'new_api') return query?.hasAccessToken === true
  if (scheme === 'sensenova') {
    return String(query?.username || '').trim() !== '' && query?.hasPassword === true
  }
  if (scheme === 'zhipu') return query?.hasToken === true
  // 其余四家靠账号的 API Key：这里不判（界面上「未配置凭证」徽章由账号层给）
  return true
}

/* ─── 草稿与「读改动」（账号设置弹窗用；新增账号表单走自己的 DOM 草稿）── */

/**
 * 账号公开形态 → 本段草稿。
 *
 * **密字段一律空串**：公开形态只有 `has*` 标记、没有值（后端只给标记，
 * 值与尾号两条管道分开）—— 输入框因此永远是空的，空 = 不修改。
 * 两个非密字段（`userId` / `username`）原样回显，用户看得见自己配了什么。
 */
export function balanceQueryDraftOf(
  query: BalanceQueryPublic | null | undefined,
): Record<string, string> {
  return {
    userId: String(query?.userId || ''),
    username: String(query?.username || ''),
    accessToken: '',
    password: '',
    token: '',
  }
}

/**
 * 读本段的改动 → `PATCH /api/accounts` 里的 `balanceQuery` 子对象。
 * `null` = 这一家没有可填的方案字段（不提交这个键）。
 *
 * 语义与后端 `normalize_balance_query_patch` 一致：
 *   · 密字段：**留空 = 不修改**（清除走「清除」按钮，它立刻发 null）；
 *   · 非密字段：原样提交（草稿从记录初始化，没动过就是原值 → 后端判无变化；
 *     用户清空 = 显式清掉这一项，看得见自己清的是谁）。
 */
export function readBalanceQueryChanges(
  scheme: string | undefined | null,
  values: Record<string, string>,
): Record<string, string> | null {
  const spec = balanceSpecOf(scheme)
  if (!spec || !spec.fields.length) return null
  const patch: Record<string, string> = {}
  for (const field of spec.fields) {
    const next = String(values[field.key] ?? '').trim()
    if (field.secret) {
      if (next) patch[field.key] = next
      continue
    }
    patch[field.key] = next
  }
  return Object.keys(patch).length ? patch : null
}

/* ─── 渲染：账号设置弹窗的「查询设置」段 / 新增账号表单共用 ──── */

/**
 * 一行方案字段（受控）。
 *
 * `configured` 只对密字段有意义（公开形态里的 `has*`）：已配置时占位提示
 * 改成「已配置，留空则不修改」并给出「清除」按钮 —— 与 CatPaw 的余额凭证、
 * 自定义账号的 API Key 两处同款。
 */
export function BalanceFieldRow({
  field, value, configured, onValue, onClear, clearBusy, idPrefix,
}: {
  field: BalanceField
  value: string
  configured: boolean
  onValue: (next: string) => void
  /** 清除已配置的密字段（传 null 给后端）；非密字段不给这个动作 */
  onClear?: () => void
  clearBusy?: boolean
  /** DOM id 前缀（两处调用点的 id 不能撞） */
  idPrefix: string
}): React.ReactElement {
  const id = `${idPrefix}-${field.key}`
  return (
    <div className='field-row mt-2.5'>
      <label htmlFor={id}>{field.label}</label>
      <Input id={id} className='min-w-[220px]' type={field.secret ? 'password' : 'text'}
        autoComplete={field.secret ? 'new-password' : 'off'}
        placeholder={configured ? t('已配置，留空则不修改') : field.placeholder}
        title={field.hint} value={value}
        onChange={event => onValue(event.currentTarget.value)} />
      {configured && onClear ? (
        <Button variant='outline' size='sm' disabled={clearBusy === true} onClick={onClear}
          title={t('清除已配置的 {label}', { label: field.label })}>{t('清除')}</Button>
      ) : null}
    </div>
  )
}

/**
 * 一整段方案字段（说明行 + 逐字段行）。`values` 只装**正在编辑**的值
 * （密字段留空 = 不修改）；`configured` 是公开形态来的「已配置」标记。
 */
export function BalanceQuerySection({
  spec, values, configured, onValue, onClear, clearBusy, idPrefix,
}: {
  spec: BalanceSchemeSpec
  values: Record<string, string>
  configured: Record<string, boolean>
  onValue: (key: BalanceFieldKey, next: string) => void
  onClear?: (key: BalanceFieldKey) => void
  clearBusy?: boolean
  idPrefix: string
}): React.ReactElement {
  return (
    <>
      <p className='detail mt-2.5'>
        {t('余额查询方案：{label}', { label: spec.label })}
        <span className='ml-1 text-subtle'>{spec.endpoint}</span>
        {' — '}{spec.note}
      </p>
      {spec.fields.map(field => (
        <BalanceFieldRow key={field.key} field={field} idPrefix={idPrefix}
          value={values[field.key] || ''} configured={configured[field.key] === true}
          onValue={next => onValue(field.key, next)}
          onClear={onClear ? () => onClear(field.key) : undefined}
          clearBusy={clearBusy} />
      ))}
    </>
  )
}
