/**
 * 余额格上的「积分明细」入口 —— 逐份积分包（面额 / 已用 / 剩余 + 领取 → 到期）。
 *
 * ── 为什么单开一个入口，而不是把这些家也塞进 usageDetail ──────
 * `usageDetail` 是**整套两行形态**（套餐徽标 + 读数按钮 + token 平铺行，CodeArts）：
 * 启用它会替掉余额格现在的样子。主额度桶那格是「套餐名 + 进度条 + 剩 x / 共 y」，
 * 换过去等于把进度条丢掉 —— 那正是这一列最值钱的一眼读数。所以本形态只**加**
 * 一个入口，各行原有的排版一个字不动：读数、悬停提示、进度条、徽章都还是原来的。
 *
 * ── 入口就是读数本身（与 CodeArts 那格同一套交互）────────────
 * 不做「读数 + 一个小箭头按钮」那种两件式：箭头只有十来个像素，是这一列里最难点
 * 中的东西（余额格一百多像素宽，鼠标还得先瞄过去）。这里照 CodeArts 的办法 ——
 * **整行读数就是按钮**（`.usage-toggle`：占满格子宽度并向两侧出血、悬停有底色、
 * caret 绝对定位不占布局宽度、点哪儿都开），两种格形态各配一个变体：
 *   * `sum`  单行读数形态（WorkBuddy 那类）：`.val` 居中，与 CodeArts 那格逐字同款；
 *   * `pool` 主额度桶形态（ZCode 那类，`usagePool` 主导的格子）：进度条与读数一起
 *     住在按钮里，左对齐（左边有进度条，居中会让条与读数分家）。
 *
 * ── 数据来源（后端契约 `credits[]`）──────────────────────────
 * 一行一份上游的积分包，多家字段逐键对齐（各家的行构造各在自己模块里，如
 * Trae：`trae::usage::credit_rows`；WorkBuddy：`billing::credit_packs_from`）：
 * ```text
 *   name          这一份是什么（上游的套餐名 / 商品名）
 *   total/used/remain   面额 / 已用 / 剩余（缺键 = 上游没给，不补 0）
 *   grantAt       领取（发放）时刻，毫秒
 *   startAt       生效（抵扣开始）时刻，毫秒 —— 发放时刻缺失时顶上左边那一半
 *   expireAt      到期时刻，毫秒
 *   unit          与主读数不同单位时才在（Trae 的套餐包 vs 积分包）
 *   daily         每日重置的包（到期字段对它是「今天 24 点」的意思）
 *   grantReason   WorkBuddy 的发放原因（官方活动 / 拉新奖励…），可无
 * ```
 * 这里只读不猜：谁缺键谁就不显示那一段。`null` 与 `0` 的区别在这一层就得守住 ——
 * `Number(null) === 0`，把"没读到"当 0 会画出「剩 0 / 共 0」这种看起来像读到了的
 * 假读数（后端 module 头那条纪律，前端这边是它的最后一站）。
 *
 * ── 时间显示分两级 ──────────────────────────────────────────
 * 行上一律 `MM-DD HH:mm`（弹层要一眼扫完，年份是噪音），完整时刻（含年）只在悬停
 * 的 title 里 —— 与 `pendingPlansTitle` 同一处置。已过期的包不给"到期倒计时"，
 * 直接标「已过期」并压成灰字：一个负数天数和一句「已过期」是同一件事，后者好读。
 *
 * ── 花不出去的收在末尾，且默认折叠 ──────────────────────────
 * 能花的在前（照后端给的顺序：最近到期的排在先），**剩余 0 与已过期的**收进末尾
 * 那组「已用完 / 已过期」，默认收起、点表头展开（判据在 `CreditRow.spent`）。
 * 上游发得勤时这一列几十份、大半是花完的，混排的结果是"打开弹层先看到一堆没用的"。
 * 弹层**每次打开都从最上面开始**（初始焦点落在容器上，理由见 `initialFocus`），展开
 * 那组时把第一行新行带进一眼（见那段 layout effect）。
 */

import * as React from 'react'
import { Meter, Popover, PopoverContent, PopoverTrigger, Button } from '@ui'
import { t } from '../i18n'
import { formatTime, numberText, type UsageEntry } from './accounts-shared'

/** 明细里的一行（`creditRowsOf` 的产物；渲染输入与 `usageDetailOf` 的行同形） */
export type CreditRow = {
  name: string
  /** 右侧读数：『剩 x / y [单位]』；一个数都读不到时给「—」 */
  view: string
  tone: 'ok' | 'warn' | 'bad' | 'muted'
  /** **剩余**百分比（条画的是剩余，与其余几处同向）；null = 不画条 */
  percent: number | null
  /** 「领取 … · 到期 …」那一行（两边都没有就 null） */
  window: string | null
  /** 悬停全文（完整时刻、来源、为什么没条 —— 界面上不放长句的原因） */
  title: string
  /**
   * 这一份还能不能花。两条判据：**剩余读到 0**（或已用 ≥ 总量）算耗尽，
   * **到期时刻已过**算作废 —— 对看盘的人来说「花不出去」是同一件事，
   * 都收进弹层末尾那组（见 `CreditDetailButton`）。
   *
   * 两种"看着像用完、其实没完"的必须留在主列表：**每日重置**的包明天回满
   * （`remain` 现在就是 0），**不限量**的（`-1` 哨兵）本来就花不完。
   *
   * **流水行恒为 false**（见 {@link CreditRow.amount}）：一笔已经发生的收支没有
   * "还能不能花"可言，一条 `-0.96` 的消耗也不是"用完了的积分包"。
   */
  spent: boolean
  /**
   * 这一行是**流水一笔**（`record: true`）而不是积分包：有它时右侧读数是带符号
   * 的金额（`+1000` / `-0.96`），没有「剩 / 共」那套措辞、也不画比例条。
   *
   * 判据走**后端给的标记**而不是「有没有 total」这类形状探测：上游给一笔
   * 没有剩余概念的记录，和给一份没写面额的包，在形状上可以长得一样，
   * 只有产出它的那一端知道它是哪一种。
   */
  amount: number | null
  /** 到期时刻（毫秒）；上游没给就是 null —— 读不到就不谈「即将过期」，不替它编一个 */
  expireAt: number | null
  /** 剩余量（与 `view` 里那个数同源）；「即将过期」求和用，null = 没读到 */
  remain: number | null
  /**
   * 落在「即将过期」窗口内、且还能花。
   *
   * 它**不参与排序**：行的先后次序照后端给的那份（见 `creditRowsOf` 头那段），
   * 这个标记只决定「进弹层的哪一组」—— 即将过期的挑出来置顶成一组，其余行
   * 保持原顺序逐行往下排（见 `CreditDetailButton`）。
   */
  soon: boolean
}

/**
 * 数值字段 → number 或 null。
 *
 * 与 `usagePool` 的 `numberOrNull` 同一口径且同样不能省：`Number(null)` 是 0，
 * 照直转换会把「上游没给」变成「这个数是 0」。后端的 `credits[]` 已经缺键不写，
 * 这里是第二道闸。
 */
function numberOrNull(value: unknown): number | null {
  if (value === null || value === undefined || value === '') return null
  const parsed = Number(value)
  return Number.isFinite(parsed) ? parsed : null
}

/**
 * 「即将过期」窗口：7 天。
 *
 * 与网关侧选号用的 `expiring_soon` 窗口同一口径 —— 账号页提示「这些积分快作废」
 * 与后端「优先消耗快过期的包」说的应该是同一批钱，两边窗口不一致会让人以为
 * 选路算错了。
 */
export const EXPIRING_SOON_MS = 7 * 24 * 60 * 60 * 1000

/** 弹层里置顶那一组的汇总（`expiringSoonOf` 的产物） */
export type ExpiringSoon = {
  /** 窗口内、还能花的那些包（保持后端给的相对顺序） */
  rows: CreditRow[]
  /** 这些包的剩余之和；没有这样的包时为 0 */
  sum: number
  /** 份数 */
  count: number
  /** 其中**今天内**（自然日）到期的份数 */
  todayCount: number
  /** 其中今天内到期的剩余之和 —— 这才是「今天必须花掉」的那笔 */
  todaySum: number
}

/**
 * 挑出窗口内即将过期的积分包并求和。
 *
 * 判据全部复用行上已经算好的位（`soon` / `remain`），不重算一遍：
 * 「什么算即将过期」只在 `creditRowsOf` 里有一处定义。
 *
 * 另外单独汇总**今天内**（今天 24 点前）到期的那部分：7 天是「该安排了」，
 * 今天内是「今天不花就没了」，两件事紧急程度差一个数量级，摊在一个数里会
 * 把最急的那笔藏起来。口径按自然日（跨零点不算今天），与行首标签同一判据。
 *
 * 这里**不改行序**。置顶是渲染时的分组（见 `CreditDetailButton`），不是排序：
 * 后端各家都按自己的口径排好了（见 `creditRowsOf` 头那段），前端再排一次只会
 * 让两处的口径分叉。
 */
export function expiringSoonOf(rows: CreditRow[], now: number = Date.now()): ExpiringSoon {
  const soon = rows.filter(row => row.soon)
  const sum = soon.reduce((total, row) => total + (row.remain ?? 0), 0)
  const endOfToday = (() => {
    const date = new Date(now)
    date.setHours(23, 59, 59, 999)
    return date.getTime()
  })()
  const today = soon.filter(row => row.expireAt !== null && row.expireAt <= endOfToday)
  const todaySum = today.reduce((total, row) => total + (row.remain ?? 0), 0)
  return {
    rows: soon,
    sum: Number(sum.toFixed(2)),
    count: soon.length,
    todayCount: today.length,
    todaySum: Number(todaySum.toFixed(2)),
  }
}

/**
 * 到期还剩多久 → 行首那枚标签。
 *
 * 两档口径，按自然日切：
 *   · **今天内**（今天 24 点前）：细到 `N 小时后到期`，不足一小时给 `M 分钟后到期`
 *     —— 这一档是「今天不花就没了」，小时数才有决策价值；
 *   · 明天及以后：`明天到期` / `N 天后到期`。
 *
 * 为什么不一概用小时：明天凌晨到期的包按小时算是「5 小时」，但用户心里的日历
 * 是「明天」，两种口径混在一个标签里只会让人反复换算。
 */
export function daysLeftLabel(expireAt: number | null, now: number = Date.now()): string {
  if (expireAt === null || expireAt <= 0) return ''
  const target = new Date(expireAt)
  if (Number.isNaN(target.getTime())) return ''
  const left = expireAt - now
  if (!Number.isFinite(left) || left <= 0) return ''
  const today = new Date(now)
  if (
    target.getFullYear() === today.getFullYear()
    && target.getMonth() === today.getMonth()
    && target.getDate() === today.getDate()
  ) {
    const hours = Math.floor(left / 3_600_000)
    if (hours < 1) {
      // 不足一小时就别再拆成秒：`1 分钟后到期` 在最后一分钟里是准确的
      const minutes = Math.max(1, Math.floor(left / 60_000))
      return t('{minutes} 分钟后到期', { minutes })
    }
    return t('{hours} 小时后到期', { hours })
  }
  const dayOf = (date: Date): number =>
    Date.UTC(date.getFullYear(), date.getMonth(), date.getDate())
  const days = Math.round((dayOf(target) - dayOf(today)) / 86_400_000)
  if (days <= 1) return t('明天到期')
  return t('{days} 天后到期', { days })
}

/** 毫秒时间戳 → `MM-DD HH:mm`（年份在悬停的完整时刻里；取不出来给空串） */
function shortTime(value: number): string {
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return ''
  const pad = (part: number): string => String(part).padStart(2, '0')
  return `${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`
}

/**
 * 行单位 → 界面文案。
 *
 * 后端 `credits[].unit` 是**口径名**（积分 / 额度），不是界面文案；主读数那一路
 * （`usageSummary` 的「可用 {value} {unit}」）也照原样显示，中文界面下两者一致。
 * 这里只把能翻译的两个过一遍词典，别让英文界面里冒出「额度」两个汉字
 * （其余取值原样透出 —— 认不出的口径宁显原名，与各家 `quota_meter_label`
 * 的回落同一取向）。
 */
function unitLabel(unit: string): string {
  if (unit === '积分') return t('积分')
  if (unit === '额度') return t('额度')
  return unit
}

/**
 * 读数文档 → 明细行（没有 `credits` 数组、或数组为空 → 空数组）。
 *
 * 顺序**照后端给的那份**：各家都在后端排好了（如 Trae 按最近到期在前、WorkBuddy
 * 按套餐优先级 + 到期升序；排序口径与各自的聚合读数同源）。前端再排一次只会让两处
 * 的口径分叉。
 */
export function creditRowsOf(entry: UsageEntry): CreditRow[] {
  if (!entry || typeof entry !== 'object' || Array.isArray(entry)) return []
  const data = entry as Record<string, unknown>
  const packs = Array.isArray(data.credits) ? data.credits as Array<Record<string, unknown>> : []
  const now = Date.now()
  return packs.map(pack => {
    const name = String(pack?.name || '').trim() || t('积分包')

    // ── 流水一笔：另一套读法（见 CreditRow.amount）──────────────
    // 右侧读数是**带符号的金额**，不是「剩 x / y」：一笔已经发生的收支没有
    // 剩余可言。符号必须留着 —— 取掉绝对值后，「获得 1000」与「消耗 1000」
    // 在屏幕上长得一模一样，而这一列恰恰就是靠符号读的。
    if (pack?.record === true) {
      const amount = numberOrNull(pack?.amount)
      const unit = String(pack?.unit || '').trim()
      const at = numberOrNull(pack?.at)
      const expireAt = numberOrNull(pack?.expireAt)
      const value = amount === null ? '—' : `${amount > 0 ? '+' : ''}${numberText(amount)}`
      const view = unit && value !== '—' ? `${value} ${unitLabel(unit)}` : value
      const parts: string[] = []
      if (at !== null && at > 0) parts.push(shortTime(at))
      if (expireAt !== null && expireAt > 0) parts.push(t('{time} 到期', { time: shortTime(expireAt) }))
      const remark = String(pack?.remark || '').trim()
      return {
        name,
        view,
        // 获得是这一列的"进项"，消耗不该抢同样的注意力；两者都不用红
        tone: amount !== null && amount < 0 ? 'muted' : 'ok',
        percent: null,
        window: parts.length ? parts.join(' · ') : null,
        title: [
          name,
          view,
          at !== null && at > 0 ? formatTime(at) : '',
          expireAt !== null && expireAt > 0
            ? t('到期 {time}', { time: formatTime(expireAt) })
            : '',
          remark,
        ].filter(Boolean).join(' · '),
        // 流水恒不进「已用完 / 已过期」那组（见 CreditRow.spent）
        spent: false,
        amount,
        expireAt,
        // 流水没有「还剩多少」可言，也就不参与即将过期求和
        remain: null,
        soon: false,
      }
    }

    const unit = String(pack?.unit || '').trim()
    const total = numberOrNull(pack?.total)
    const used = numberOrNull(pack?.used)
    const remain = numberOrNull(pack?.remain)
    const unlimited = pack?.unlimited === true
    const grantAt = numberOrNull(pack?.grantAt)
    const startAt = numberOrNull(pack?.startAt)
    const expireAt = numberOrNull(pack?.expireAt)
    const daily = pack?.daily === true
    const reason = String(pack?.grantReason || '').trim()
    const expired = expireAt !== null && expireAt > 0 && expireAt <= now

    // ── 读数 ────────────────────────────────────────────────
    // 「不限」写在读数里而不是画一个数：-1 那种哨兵值摆上屏会被读成"欠费"。
    // 单位贴在整串末尾（「剩 400 / 500 额度」），不夹在中间 —— 中间那一版读起来
    // 像「剩 400 额度 / 共 500 额度」的省略写法，数字与单位的归属全靠猜。
    // 三个数都过 `numberText`：上游给的是高精度浮点（`39.430000335` 这种），
    // 照直拼会把小数尾巴铺满弹层（口径见 accounts-shared 那个函数）。
    const hasTotal = total !== null && total > 0
    const amount = unlimited
      ? t('不限')
      : remain !== null
        ? hasTotal
          ? t('剩 {remaining} / {total}', { remaining: numberText(remain), total: numberText(total) })
          : t('剩 {remaining}', { remaining: numberText(remain) })
        : used !== null
          ? t('已用 {used}', { used: numberText(used) })
          : '—'
    const view = !unlimited && unit && amount !== '—' ? `${amount} ${unitLabel(unit)}` : amount
    // 剩余比例：只在有真总量时算（画不出比例就不装作有）
    const percent = hasTotal && remain !== null
      ? Math.max(0, Math.min(100, (remain / total) * 100))
      : null

    // ── 还能不能花（分组用，见 CreditRow.spent）────────────────
    // 「用完」认两个读数：剩余读到 0；剩余读不到但「已用 ≥ 总量」也认 ——
    // 上游只给 used/total 的那一档同样答得出这个问题，不该因为缺 remain 就漏判。
    // 两者都读不到时**不猜**（宁留在主列表让人看见，也不凭空的把它折叠起来）。
    const exhausted = (remain !== null && remain <= 0)
      || (remain === null && used !== null && hasTotal && used >= total)
    const spent = !unlimited && !daily && (exhausted || expired)

    // ── 即将过期（弹层置顶组 + 余额格那枚徽标）──────────────────
    // 三个条件缺一不可：读得到到期时刻、落在窗口内、这笔钱真的还能花（`spent`
    // 已经把用完 / 已过期 / 每日重置 / 不限量这几档排掉了）。缺 `remain` 时不猜成
    // 0 —— 它照样进那组（「会作废」这件事已经确定），只是不参与求和。
    const soon = !spent
      && expireAt !== null && expireAt > now && expireAt <= now + EXPIRING_SOON_MS

    // ── 领取 → 到期 ─────────────────────────────────────────
    // 左边那一半两个来源，**名字必须跟着来源变**：`grantAt`（上游的发放时刻）
    // 才叫「领取」；只剩 `startAt`（抵扣开始）时写「生效」—— 这两件事不能混称，
    // 缺了发放时刻就标成「领取」等于替上游编一个到手时间。
    const parts: string[] = []
    if (grantAt !== null && grantAt > 0) {
      parts.push(t('{time} 领取', { time: shortTime(grantAt) }))
    } else if (startAt !== null && startAt > 0) {
      parts.push(t('{time} 生效', { time: shortTime(startAt) }))
    }
    if (daily) {
      // 日额度包的「到期」是今天 24 点（周期边界），不是这笔钱作废 ——
      // 照「到期」写会让人以为额度明天就没了（实际是重置回满）
      parts.push(t('每日重置'))
    } else if (expireAt !== null && expireAt > 0) {
      parts.push(expired ? t('已过期') : t('{time} 到期', { time: shortTime(expireAt) }))
    }
    if (reason) parts.push(reason)

    // ── 语义色与悬停 ────────────────────────────────────────
    const tone: CreditRow['tone'] = expired || remain === 0
      ? 'muted'
      : percent !== null && percent < 20
        ? 'warn'
        : remain === null && used === null && !unlimited
          ? 'muted'
          : 'ok'
    const title = [
      name,
      view,
      grantAt !== null && grantAt > 0
        ? t('领取 {time}', { time: formatTime(grantAt) })
        : startAt !== null && startAt > 0
          ? t('生效 {time}', { time: formatTime(startAt) })
          : '',
      expireAt !== null && expireAt > 0
        ? (expired
          ? t('已过期（{time}）', { time: formatTime(expireAt) })
          : t('到期 {time}', { time: formatTime(expireAt) }))
        : '',
      daily ? t('每日重置（额度按周期回满，不是作废）') : '',
      reason,
      percent === null && remain !== null ? t('上游未给总量，画不出剩余比例') : '',
      used !== null && total !== null ? t('已用 {used} / 共 {total}', { used: numberText(used), total: numberText(total) }) : '',
    ].filter(Boolean).join(' · ')
    return {
      name,
      view,
      tone,
      percent,
      window: parts.length ? parts.join(' · ') : null,
      title,
      spent,
      amount: null,
      expireAt,
      remain,
      soon,
    }
  })
}

/** 明细弹层里的一行（视觉与 CodeArts 那份明细同一套类名，观感一致） */
function CreditRowView({ row }: { row: CreditRow }) {
  return (
    <div className='usage-pop-row' title={row.title}>
      <div className='usage-pop-line'>
        <span className='usage-pop-name'>{row.name}</span>
        {/* 只会出现在置顶那一组里（`row.soon` 就是分组判据），其余行照旧 */}
        {row.soon ? <span className='usage-pop-soon-tag'>{daysLeftLabel(row.expireAt)}</span> : null}
        <span className={`usage-pop-view ${row.tone}`}>{row.view}</span>
      </div>
      {row.window ? <div className='usage-pop-window'>{row.window}</div> : null}
      {row.percent !== null ? (
        <Meter value={row.percent} variant={row.tone === 'ok' ? 'success' : row.tone === 'warn' ? 'warning' : 'muted'}
          className='mt-1.5' trackClassName='h-[3px]' aria-label={t('剩余 {percent}%', { percent: Math.round(row.percent) })} />
      ) : null}
    </div>
  )
}

/**
 * 读数本体即入口：整行可点，点开就是这份明细（交互与 CodeArts 那格同款）。
 *
 * `variant` 决定这一行长什么样（见文件头）：`sum` 是单行读数的居中形态
 * （`.usage-toggle` + `.val`，与 CodeArts 逐字同款），`pool` 是「进度条 + 读数」
 * 的桶形态（同一个 `.usage-toggle` 加 `.usage-pool-line` 把间距与对齐改回桶那套）。
 * `children` 就是原来的那点读数 DOM —— 读数怎么渲染仍由调用方（`UsageCell`）决定，
 * 这里只负责"把它变成按钮"。
 *
 * `subscriptionLine` 由调用方用 `subscriptionText` 拼好递进来（套餐 / 到期 /
 * 余量那一行）：那份措辞是悬停提示的既有产物，两处各写一份迟早会漂。
 */
export function CreditDetailButton({ rows, subscriptionLine, variant, kind, title, children }: {
  rows: CreditRow[]
  subscriptionLine: string
  variant: 'sum' | 'pool'
  /** 语义档（ok / warn / …）：`.usage-toggle` 按它上色，与读数本来的档同一套 */
  kind: string
  /** 悬停提示（读数那份摘要，与没挂入口时逐字相同） */
  title: string
  children: React.ReactNode
}) {
  const [open, setOpen] = React.useState(false)
  /**
   * 末尾那组「已用完 / 已过期」的开合，**默认收起**：上游发得勤的时候这一列能有
   * 几十份，全是花不出去的，铺在上面会把还能花的挤到看不见的地方 —— 第一眼该看到
   * 的是"我还能用哪几笔"。开合状态跟着这个组件走（弹层关掉再开还是原样）：
   * 一个刚刚展开过的人在重新打开时又要再点一次，只会觉得它坏了。
   */
  const [spentOpen, setSpentOpen] = React.useState(false)
  const liveRows = rows.filter(row => !row.spent)
  const spentRows = rows.filter(row => row.spent)
  /**
   * 即将过期的那批：挑出来单独成组，**不动其余行的顺序**。
   *
   * `restRows` 逐字保持 `liveRows` 的相对次序（也就是后端给的那份），只把这批
   * 「最该先花掉」的提到「积分明细」标题正下方并标出还剩几天。既有排序口径
   * 归后端（见 `creditRowsOf` 头段），这里做的是分组，不是重排。
   */
  const expiring = expiringSoonOf(liveRows)
  const restRows = liveRows.filter(row => !row.soon)
  const className = variant === 'pool' ? `usage-toggle usage-pool-line ${kind}` : `usage-toggle ${kind}`

  /**
   * 滚动容器本身（挂载时归零、当初始焦点，见下面两段注释）。
   *
   * 用**回调 ref** 而不是 useRef + useEffect：要抢在浏览器把"获得焦点的元素
   * 滚进视野"之前把位置定下来，回调 ref 是节点刚插进 DOM 的那一刻，最早。
   */
  const boxRef = React.useRef<HTMLDivElement | null>(null)
  const attachBox = React.useCallback((node: HTMLDivElement | null) => {
    boxRef.current = node
    // 每次挂载都从最上面开始。节点既然是新的，这里本来就是 0 —— 写出来是为了
    // 钉住这条约定：明细从上往下读（能花的在最前面），弹层不该带着上次的滚动位置重开。
    if (node) node.scrollTop = 0
  }, [])

  /**
   * 初始焦点落在**容器**上，而不是"弹层里第一个可聚焦的元素"。
   *
   * 库的默认行为是后者（见 PopoverPopup 的 `initialFocus` 文档），而这个弹层里唯一
   * 可聚焦的元素是末尾那个「已用完 / 已过期」组表头 —— 它就贴在列表最底部。于是
   * 浏览器为了把获得焦点的它滚进视野，会把整个滚动容器拉到最底部：用户报的"一打开
   * 就停在底部、每次都要先往上滚"就是这么来的（实测：先点过组表头再重开，
   * scrollTop 直接是 59/236 这种"刚好够露出那个按钮"的值，而不是 0）。
   *
   * 落在容器上就没事：容器的盒子本来就整块可见，滚不到它；键盘用户按 Tab 也照样
   * 进得来（下一个可聚焦的正是那个组表头）。容器因此带 tabIndex={-1}。
   */
  const initialFocus = React.useCallback(() => boxRef.current ?? null, [])

  /**
   * 展开分组后把新行带进一眼。
   *
   * 那个表头平时就贴在滚动容器的**底部**，点开之后新行全在下方视野外 ——
   * 不滚这一下，点击的可见反馈只剩一个三角翻面，看着像没反应。
   * `block: 'nearest'` 只滚必需的那点距离，不做整屏跳。
   *
   * 必须放在 layout effect 里（而不是 onClick 的 rAF 里）：React 19 的提交是异步的，
   * rAF 常常跑在新行还没进 DOM 的时候，`querySelector` 拿不到东西（第一版就是这么
   * 静默失效的）。滚的是**第一行新行**而不是整个组：组比容器还高时，对整组用
   * 'nearest' 会把组的**底边**对齐到容器底部（规范如此），结果又变回"一展开就在底部"。
   */
  React.useLayoutEffect(() => {
    if (!spentOpen) return
    const first = boxRef.current?.querySelector('.usage-pop-spent .usage-pop-row')
    if (first instanceof HTMLElement) first.scrollIntoView({ block: 'nearest' })
  }, [spentOpen])

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger
        render={<Button size='default' variant='ghost' type='button' className={className} title={title}
          aria-label={t('积分明细')} />}
      >
        {variant === 'pool' ? children : <span className='val'>{children}</span>}
        {/* 徽标只挂单行读数形态：pool 那行里已经有进度条与「剩 x / y」，再挤一枚
            胶囊会把读数压成省略号（余额列一百多像素宽，见那段 CSS 注释） */}
        {variant !== 'pool' && expiring.count > 0 ? (
          <span className='usage-toggle-soon'>{t('7天 {n}', { n: numberText(expiring.sum) })}</span>
        ) : null}
        <span className='caret' aria-hidden>▾</span>
      </PopoverTrigger>
      {/* `align='center'`：浮层以读数按钮为中心展开。原来是 'end'（右缘对齐），
          而这一列的按钮本身就靠右，弹层再向右贴齐就整个甩到表格右外侧，读起来像
          挂在了「操作」列下面。居中后它落在自己的格子上，与哪一行是哪一行对得上。 */}
      <PopoverContent align='center' sideOffset={6} className='w-[320px]' initialFocus={initialFocus}>

        <div className='usage-pop-credit' ref={attachBox} tabIndex={-1}>
          {subscriptionLine ? <div className='usage-pop-sub'>{subscriptionLine}</div> : null}
          <div className='usage-pop-group'>{t('积分明细')}</div>
          {expiring.count > 0 ? (
            <>
              <div className='usage-pop-soon'>
                {expiring.todayCount > 0
                  ? t('即将过期 {n} 积分（7 天内 {c} 份，今天内 {d}）', {
                    n: numberText(expiring.sum),
                    c: expiring.count,
                    d: numberText(expiring.todaySum),
                  })
                  : t('即将过期 {n} 积分（7 天内 {c} 份）', {
                    n: numberText(expiring.sum), c: expiring.count,
                  })}
              </div>
              {expiring.rows.map((row, index) => (
                <CreditRowView key={`soon-${row.name}-${index}`} row={row} />
              ))}
            </>
          ) : null}
          {restRows.map((row, index) => <CreditRowView key={`${row.name}-${index}`} row={row} />)}
          {spentRows.length > 0 ? (
            <div className='usage-pop-spent'>
              <Button size='default' variant='ghost'
                type='button'
                className='usage-pop-spent-head'
                aria-expanded={spentOpen}
                title={t('展开或收起（默认收起）')}
                onClick={() => setSpentOpen(value => !value)}
              >
                {/* ⚠️ 这个 span 必须**空着**：三角由 CSS 的 border 画
                    （见 page-accounts-table.css 的 .usage-pop-spent-head .caret），
                    里面再塞一个 ▾ 字形就会叠出两个箭头 —— 一个 border 画的、
                    一个字形溢出的（width/height 是 0，字形照样画得出来）。 */}
                <span className='caret' aria-hidden />
                <span>{t('已用完 / 已过期')}</span>
                <span className='n'>{t('{count} 份', { count: spentRows.length })}</span>
              </Button>
              {spentOpen
                ? spentRows.map((row, index) => (
                  <CreditRowView key={`spent-${row.name}-${index}`} row={row} />
                ))
                : null}
            </div>
          ) : null}
        </div>
      </PopoverContent>
    </Popover>
  )
}
