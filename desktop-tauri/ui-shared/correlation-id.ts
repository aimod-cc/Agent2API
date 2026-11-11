/**
 * 关联 id（test_id 等）的生成：优先 `crypto.randomUUID`，退化到自拼的 v4 形态。
 *
 * ── 为什么要有这个函数 ────────────────────────────────────────
 * `crypto.randomUUID` 只在**安全上下文**（HTTPS / localhost / WebView2）可用。
 * 面板走反代以普通 HTTP + 非 localhost 的地址打开时（内网 IP、自建域名），
 * 这个 API 是 `undefined`，裸调会抛 `TypeError`，让整只工具直接不可用。
 *
 * 内置助手有两只工具用它做关联 id：`test_model`（发起模型测试）与图片工具。
 * 其中 `test_model` 曾经裸调 `crypto.randomUUID()`，在非安全上下文下面板上
 * 「测试」整只工具报错、发不出请求 —— 而同一个面板的「测试」对话框
 * （`model-test-dialog.tsx`）早就用自拼兜底了，两条路径行为不一致。
 *
 * 把兜底收敛到这里，两条路径共用一份，避免以后再分叉。
 *
 * 退化实现拼的是**形态合法的 v4**（第 3 组以 `4` 开头、第 4 组以 `a` 开头），
 * 不是任意随机串 —— 上游若按 UUID 格式校验，也能过。
 */
export function newCorrelationId(): string {
  const uuid = globalThis.crypto?.randomUUID?.()
  if (uuid) return uuid
  const hex = (count: number): string =>
    Array.from({ length: count }, () => Math.floor(Math.random() * 16).toString(16)).join('')
  return `${hex(8)}-${hex(4)}-4${hex(3)}-a${hex(3)}-${hex(12)}`
}
