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
 *   daily         每日重置的包（**它的 `expireAt` 是「今天 24 点」** —— 后端按
 *                 `isDaily` 特意从 `CycleEndTime` 取，不是作废时刻）
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
import { Popover, PopoverContent, PopoverTrigger } from '@ui'
import { t } from '../i18n'
import { formatTime, type UsageEntry } from './accounts-shared'
import { expiryMillis } from './accounts-domain'

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
   * 这一份的到期时刻（毫秒）；没有到期字段（或不是正数）给 null。
   *
   * 界面上**用不到**它（那一行由 `window` 拼好），它是给账号表那两列
   * 「最近到期」用的读数：让那边从字段上取，而不是回到 `window` 的反解或
   * 对 `credits[]` 再解一遍 —— 否则「最近到期」与明细迟早会给出两个日子。
   */
  expireAt: number | null
  /** 这一份还剩多少（与读数同一口径）；读不到给 null。「不限」的哨兵不当数字 */
  remain: number | null
  /** 这一份的剩余量单位（`credits` / `积分`）；没给给空串 */
  unit: string
  /** 日额度包：它的「到期」是周期边界（今天 24 点回满），不是这笔钱作废。
   *  账号表那两列在「没有一次性包」时会退到它，读数因此是「今日剩余」——
   *  同一份 `expireAt`，两种说法（见 `nearestCreditOf`）。 */
  daily: boolean
  /**
   * 这一份还能不能花。两条判据：**剩余读到 0**（或已用 ≥ 总量）算耗尽，
   * **到期时刻已过**算作废 —— 对看盘的人来说「花不出去」是同一件事，
   * 都收进弹层末尾那组（见 `CreditDetailButton`）。
   *
   * 两种"看着像用完、其实没完"的必须留在主列表：**每日重置**的包明天回满
   * （`remain` 现在就是 0），**不限量**的（`-1` 哨兵）本来就花不完。
   */
  spent: boolean
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
export function creditRowsOf(entry: UsageEntry, now: number = Date.now()): CreditRow[] {
  if (!entry || typeof entry !== 'object' || Array.isArray(entry)) return []
  const data = entry as Record<string, unknown>
  const packs = Array.isArray(data.credits) ? data.credits as Array<Record<string, unknown>> : []
  return packs.map(pack => {
    const name = String(pack?.name || '').trim() || t('积分包')
    const unit = String(pack?.unit || '').trim()
    const total = numberOrNull(pack?.total)
    const used = numberOrNull(pack?.used)
    const remain = numberOrNull(pack?.remain)
    const unlimited = pack?.unlimited === true
    const grantAt = numberOrNull(pack?.grantAt)
    const startAt = numberOrNull(pack?.startAt)
    // 到期时刻走 `expiryMillis` 的同一套归一（秒 → 毫秒）：与「有效期」列、
    // 与各处 `expiryMillis` 的调用口径一致。当前所有 provider 实测都发毫秒，
    // 但少这一层就等于把「上游哪天改成秒」变成一次静默的 1970 年
    const rawExpire = numberOrNull(pack?.expireAt)
    const expireAt = rawExpire === null ? null : (expiryMillis(rawExpire) || null)
    const daily = pack?.daily === true
    const reason = String(pack?.grantReason || '').trim()
    const expired = expireAt !== null && expireAt > 0 && expireAt <= now

    // ── 读数 ────────────────────────────────────────────────
    // 「不限」写在读数里而不是画一个数：-1 那种哨兵值摆上屏会被读成"欠费"。
    // 单位贴在整串末尾（「剩 400 / 500 额度」），不夹在中间 —— 中间那一版读起来
    // 像「剩 400 额度 / 共 500 额度」的省略写法，数字与单位的归属全靠猜。
    const hasTotal = total !== null && total > 0
    const amount = unlimited
      ? t('不限')
      : remain !== null
        ? hasTotal
          ? t('剩 {remaining} / {total}', { remaining: remain, total })
          : t('剩 {remaining}', { remaining: remain })
        : used !== null
          ? t('已用 {used}', { used: used })
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
      used !== null && total !== null ? t('已用 {used} / 共 {total}', { used, total }) : '',
    ].filter(Boolean).join(' · ')
    return {
      name, view, tone, percent, window: parts.length ? parts.join(' · ') : null, title, spent,
      // 原始值（`window` 是给人看的串，这两个是给表里那两列用的数）
      expireAt: expireAt !== null && expireAt > 0 ? expireAt : null,
      // 「不限」（-1 哨兵）不算一个剩余量：把它当数画进「最近到期积分」会被读成欠费
      remain: unlimited ? null : remain,
      unit: unit && unit !== '—' ? unitLabel(unit) : unit,
      daily,
    }
  })
}

/** 明细弹层里的一行（视觉与 CodeArts 那份明细同一套类名，观感一致） */
function CreditRowView({ row }: { row: CreditRow }) {
  return (
    <div className='usage-pop-row' title={row.title}>
      <div className='usage-pop-line'>
        <span className='usage-pop-name'>{row.name}</span>
        <span className={`usage-pop-view ${row.tone}`}>{row.view}</span>
      </div>
      {row.window ? <div className='usage-pop-window'>{row.window}</div> : null}
      {row.percent !== null ? (
        <span className='usage-pop-track'>
          <i style={{ width: `${row.percent}%` }} aria-label={t('剩余 {percent}%', { percent: Math.round(row.percent) })} />
        </span>
      ) : null}
    </div>
  )
}

/**
 * 「最近到期」那一份积分包（账号表「最近到期」/「最近到期积分」两列的数据源）。
 *
 * ── 为什么复用 `creditRowsOf` 而不是自己再解一遍 `credits[]` ──────
 * credits 的解析已经有一处（`creditRowsOf`），再做第二处就是第三份同构实现。
 * 这里只把它的产物里「最先作废的那一份」挑出来 —— 判据、读数、单位与明细逐字
 * 同源，于是表里说「最近 09-30 到期 剩 200」，点开明细里那一行必然是同一天、
 * 同一个数。原始值（`expireAt` / `remain` / `unit` / `daily`）由 `CreditRow`
 * 直接带出来，不从 `window` 那个给人看的串上反解。
 *
 * ── 取哪一份 ────────────────────────────────────────────────
 * **还没到期、且不是日额度包**的那些里到期最早的一份。三条排除各有理由：
 *   · 已过期的一份是「已经作废」不是「将作废」，拿它当「最近到期」等于这一列
 *     永远指着过去（用户问的是「接下来哪笔先没」）；
 *   · 日额度包的「到期」是周期边界（今天 24 点回满，见 `daily` 分支），它答的是
 *     「每天重置」不是「要作废了」；
 *   · 这两类排除后仍没有 → 返回 null，界面显示 `—`。
 *
 * **不能**假设后端给的顺序就是「到期升序」：Trae 那条路径才是（`credit_rows`
 * 以 `expireAt` 升序、缺失垫底），而 WorkBuddy 的 `billing::usage` 是**先按套餐
 * 优先级**、同优先级才看到期时间 —— `plan_priority` 一不同就 `return`，到期根本不
 * 参与比较。实测 `Free Plan Subscription`（2035 年到期）排在 `Bonus Pack`
 * （10 天后到期）前面。所以「挑哪一份」必须自己按 `expireAt` 排一次，
 * 详见 `nearestCreditOf` 里那段说明。
 */
export type NearestCredit = {
  /** 到期时刻（毫秒）。**日额度包的这一位是「今天 24 点」**（见下面「取哪一份」） */
  expireAt: number
  /** 这一份叫什么（悬停里要回答「是哪一笔」） */
  name: string
  /** 还剩多少（数值；读不到给 null）。界面上 0 也要画出来 —— 「马上作废 0」
   *  比「—」更准确（那一份确实要作废，只是已经花完了） */
  remain: number | null
  /** 剩余量的单位（`积分` / `credits`）；没给给空串 */
  unit: string
  /**
   * 这一份是**日额度包**（`daily: true`）。
   *
   * 界面据此换说法：一次性包的到期是「这笔钱作废」，日额度包的到期是
   * 「今天 24 点回满」—— 同一个 `expireAt`，两句话。读数（倒计时）本身
   * 一样，只有悬停与列标题要分开写。
   */
  daily: boolean
}

/**
 * 取「最先作废」的那一份（没有 → null）。
 *
 * ── 两轮筛选，一次性包永远压过日额度包 ──────────────────────
 *   ① 还没过期、且**不是**日额度包的 → 到期最早的那一份（主答案：钱要作废了）；
 *   ② ① 没有时，退到日额度包 → 它的 `expireAt` 是**今天 24 点**，读数因此是
 *      「离今天结束还有多久」（这一列在纯日额度账号上不再是 `—`）。
 *
 * ── 为什么把日额度包排在后面，而不是一视同仁 ────────────────
 * 两者问的不是同一件事：「哪笔钱要作废了」远比「今天还剩多少额度到点回满」更值得
 * 占这一列。只有在一份一次性包都没有时才轮得到后者 —— 这时候答案也确实有用
 * （它答的是「今天这家的免费额度还能用到几点」）。
 *
 * ── 日额度包的 expireAt 为什么天然就是「今天 24 点」────────────
 * 不是我在这里算的，是后端**故意挑过字段**：`billing::credit_pack_of` 对
 * `isDaily` 的包取 `CycleEndTime`、对其余取 `DeductionEndTime`
 * （`core/billing/usage.rs`，那处注释写着「日额度包该读哪个到期字段」）。
 * 所以这里直接用 `expireAt`，**前端不重算一天结束的时刻** —— 重算就是第二份实现，
 * 且跨时区/跨夏令时迟早与后端对不上。
 *
 * ── 为什么不直接「取第一个命中的」────────────────────────────
 * 早先这里信任后端给的顺序（「两家都按到期升序发出 → 第一个就是最近到期」），
 * 那是**错的**，实测两类反例：
 *
 *   · **WorkBuddy 的排序首要键是套餐优先级，不是到期时间**。`billing::usage`
 *     的注释写着「先按套餐优先级，同优先级再按到期时间升序」——`plan_priority`
 *     一不同就 `return`，到期时间根本不参与比较。于是 `Free Plan Subscription`
 *     （高优先级）会排在 `Bonus Pack`（低优先级、但早 9 年到期）**前面**。
 *     只取第一个 → 选中 2035 年到期那笔，而把 10 天后就作废的那笔藏了
 *     （实测 sunny-00001：现取「Free Plan Subscription」剩 100 / 2035 年，
 *     该取「Bonus Pack」剩 250 / 10-25）。
 *   · **Trae 的排序是「缺失垫底 + 同名按名字」**（`credit_rows`），
 *     不是「所有包按 expireAt 全序」。
 *
 * 所以这里**自己排一次**：按 `expireAt` 升序（缺失视为 +∞ 垫底）。
 * 曾经担心「前端再排一次会让两处口径分叉」—— 那个顾虑针对的是**明细列表的
 * 展示顺序**（那里照后端，见 `creditRowsOf` 不排序），与此处「挑哪一个」是
 * 两个问题：「挑」只关心哪笔最先作废，与后端整表怎么排无关。
 *
 * ── 还要跳过「已经花光」的包（一级优先，不是硬排除）──────────
 * 「最近到期」问的是「**哪笔钱**要作废了」—— 已经花光的包没有钱可作废，
 * 它到期只是从列表里消失。若不跳过，一个 `remain=0`、今晚 24 点归零的包
 * 会把「还剩 2000 分、下月才到期」的那笔整个遮住（实测 sunnyzt001 一例：
 * 现值「每日签到有礼」剩 0，而真正该报的是「新用户专属福利」剩 1962）。
 *
 * 但**只在有"还有钱"的份时才跳过**：全都花光时仍报最先作废的那份（读数 0）——
 * 这一列问的是「什么时候作废」，全花完时显示 `—` 反而丢了信息，
 * 而「0」准确说出了「那一刻会作废 0 分」。判据直接复用 `CreditRow.spent`
 * （`remain ≤ 0`，或 `remain` 缺失时 `used ≥ total`；都读不到时不猜）。
 *
 * ── `now` 为什么要能传进来 ────────────────────────────────
 * 默认取 `Date.now()` 适合**渲染**（每次画都是「此刻」）。但排序比较器会在
 * **一次 `sort()` 里反复调用它**（n log n 次）：若某份包在这一轮排序的时间窗内
 * 跨过 `expireAt`，同一行会先后算出两个不同的值 → 比较器失去传递性 → 排序结果
 * 不可预测。所以 `accounts-sort.ts` 排序时先把 `now` 快照一次、整轮复用。
 */
export function nearestCreditOf(entry: UsageEntry, now: number = Date.now()): NearestCredit | null {
  // `now` 一路传下去：过滤「已过期」与判 `expired` 必须是同一个时刻，
  // 否则同一份包可能在「算最近到期」和「标已过期」两处得到相反结论
  const rows = creditRowsOf(entry, now)

  /**
   * 这一份还有钱可花吗 —— 直接复用 `CreditRow.spent` 已经算好的判据
   * （`remain ≤ 0`，或 `remain` 缺失时 `used ≥ total`；两者都读不到时不猜），
   * 两处各写一份迟早会漂。注意 `spent` 对日额度包恒为 false（它明天会回满），
   * 但下面第 ① 轮已经把 `daily` 排除了，所以这里不影响。
   */
  const hasMoney = (row: CreditRow): boolean => !row.spent

  /** 到期升序；缺失的 `expireAt` 垫底（与别处「缺失值恒排最后」同一取向） */
  const byExpire = (a: CreditRow, b: CreditRow): number =>
    (a.expireAt ?? Number.POSITIVE_INFINITY) - (b.expireAt ?? Number.POSITIVE_INFINITY)

  // ① 一次性包：跳过日额度与已过期的，再按到期升序
  const once = rows
    .filter(row => !row.daily && row.expireAt !== null && row.expireAt > now)
    .sort(byExpire)
  // ①-a 优先取「还有钱」的第一份 —— 已花光的包没有钱可作废，不该占着这一列
  const firstWithMoney = once.find(hasMoney)
  if (firstWithMoney) {
    return {
      expireAt: firstWithMoney.expireAt as number, name: firstWithMoney.name,
      remain: firstWithMoney.remain, unit: firstWithMoney.unit, daily: false,
    }
  }
  // ①-b 全都花光了：仍报最先作废的那一份（读数会显示 0）。这一列问的是
  //      「什么时候作废」，不是「哪份还有钱」—— 全花完时显示 `—` 反而丢了信息，
  //      而「0」准确地说出了「那一刻会作废 0 分」
  const firstAny = once[0]
  if (firstAny) {
    return {
      expireAt: firstAny.expireAt as number, name: firstAny.name,
      remain: firstAny.remain, unit: firstAny.unit, daily: false,
    }
  }

  // ② 兜底：日额度包（`expireAt` = 今天 24 点）。它「到期」是周期回满而非作废，
  //    所以**不套 `hasMoney`**：日包今天花光了，这一列仍该说「今晚 24 点重置」
  const daily = rows
    .filter(row => row.daily && row.expireAt !== null && row.expireAt > now)
    .sort(byExpire)[0]
  if (daily) {
    return { expireAt: daily.expireAt as number, name: daily.name, remain: daily.remain, unit: daily.unit, daily: true }
  }

  return null
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
        render={<button type='button' className={className} title={title}
          aria-label={t('积分明细')} />}
      >
        {variant === 'pool' ? children : <span className='val'>{children}</span>}
        <span className='caret' aria-hidden>▾</span>
      </PopoverTrigger>
      <PopoverContent align='end' sideOffset={6} className='w-[320px]' initialFocus={initialFocus}>
        {/* ⚠️ 这一层 div 不是装饰，是**滚动容器本身**：组件库给 PopoverContent 编出的
            `overflow-hidden` / `max-h-[min(70vh,560px)]` 都带 `!important`，把
            max-height / overflow-y 写在弹层自己身上会被它们压掉（明细一多就既滚不动
            又被裁掉）。写在弹层里的这一层没有对手，明细的滚动归它。
            `tabIndex={-1}` 是为了能被上面的 `initialFocus` 聚焦（见那段注释）。
            动这段结构前先看 page-accounts-table.css 里 .usage-pop-credit 的说明。 */}
        <div className='usage-pop-credit' ref={attachBox} tabIndex={-1}>
          {subscriptionLine ? <div className='usage-pop-sub'>{subscriptionLine}</div> : null}
          <div className='usage-pop-group'>{t('积分明细')}</div>
          {liveRows.map((row, index) => <CreditRowView key={`${row.name}-${index}`} row={row} />)}
          {spentRows.length > 0 ? (
            <div className='usage-pop-spent'>
              <button
                type='button'
                className='usage-pop-spent-head'
                aria-expanded={spentOpen}
                title={t('展开或收起（默认收起）')}
                onClick={() => setSpentOpen(value => !value)}
              >
                <span className='caret' aria-hidden>▾</span>
                <span>{t('已用完 / 已过期')}</span>
                <span className='n'>{t('{count} 份', { count: spentRows.length })}</span>
              </button>
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
