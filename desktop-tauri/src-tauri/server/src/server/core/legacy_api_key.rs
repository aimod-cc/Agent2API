//! 顶层旧字段 `apiKey`（1.x 单 Key 兼容形态）的**原地加密迁移**。
//!
//! ── 为什么单独一个文件（而不是塞进 `core::api_keys`）────────────────
//! 这块逻辑只有十几行，但它的取舍（原地而不搬进数组、幂等靠数据自证、失败不阻断
//! 启动）与 `api_keys.rs` 里的读侧（兼容分支的降级态处置）是一对，要对着读才改得
//! 安心；而 `core::api_keys` 已经贴着本项目的单文件行数约定（见仓库纪律），再往里
//! 塞一整套「写入侧」的论证就会把它撑爆 —— 与其硬塞，不如让「旧字段的写入侧」
//! 独立成一个能被直接 grep 到的名字。读完侧（明文兼容、信封解密、解不开的降级态）
//! 仍看 `core::api_keys` 的 `decode_key` / `from_value`：那里有完整论证。
//!
//! ── 它补的是哪一块 ──────────────────────────────────────────
//! `apiKeys` 数组里每把 `key` 从本轮改造起落盘即是 `enc1:` 密文，升级前写进去的
//! 历史明文由 `api_keys::encrypt_legacy_keys` 收敛。但**还有一把更旧的 Key**：
//! 顶层字段 `apiKey`（用户从未动过 Key 列表时留下的形态，见 `api_keys` 的模块头）。
//! 它此前一直是明文，而它就在同一份 `kv` 里 —— 「拷走库文件」照样把网关口令带走，
//! 所以也要收敛，收敛方式就是本模块。
//!
//! ── 为什么是**原地**加密，而不是搬进 `apiKeys` 数组 ─────────────
//! 搬进去是一次**形态变更**：`apiKeys` 一出现，`entries_from` 与写侧就都走数组那
//! 条路，「写侧一旦动过列表才落成 `apiKeys` 并删掉 `apiKey`」这条既有语义会被无声
//! 提前触发 —— 用户什么都没做，配置形状却变了。后果不只是「看着奇怪」：
//! 「`apiKey` 还在 ⇒ 用户没动过 Key 列表」这条一目了然的推断就此失效，排查配置问题
//! 时会把人引向错误的方向。原地加密只换值、字段名与形态都不变，读侧照旧兼容，
//! 界面上看不出任何变化（掩码文案也一字不差，见 `config::build` 的取值口径）。
//!
//! ── 幂等：靠数据自证，不靠任何标记键 ─────────────────────────
//! 判据只有两条，且都长在数据自己身上（见 `pending_plaintext`）：值是非空字符串、
//! 且不是 `enc1:` 信封。加密是可重复的纯变换，迁完自然就没有明文了 —— 再跑一遍
//! 在第一段就早退，**一个字节都不写**（连配置快照都不必回写）。刻意不加 kv 标记键
//! （与 `encrypt_legacy_keys` / `encrypt_legacy_accounts` 同一纪律）：标记会多出一份
//! 「这条跑过没有」的事实来源，还可能和值本身不一致（标记在、值却仍是明文 ——
//! 上一轮加密成功但落库失败时就是这样），而数据自证没有这个不一致的余地。
//!
//! **「已经是信封」也包括解不开的信封**（密钥文件被删/被换）：那种值绝不能再
//! `seal` 一次 —— 落盘的会变成「信封套信封」，即便用户之后找回密钥文件，解出来的
//! 也只是中间那层密文，真 Key 永远回不来（一次静默、不可逆的数据损坏）。
//! 读侧的对应处置见 `api_keys::decode_key`：原样保留、不置空、不放行。
//!
//! ── 失败不阻断启动 ─────────────────────────────────────────
//! 密钥不可用（`config_dir` 不可写等）或库不可用时只记一条日志并返回 0：
//! 读侧兼容明文，本次运行这把 Key 照常可用，下次启动重试 —— 与
//! `encrypt_legacy_keys` 同一形态、同一理由（启动路径上不该因为一次加密失败
//! 让整个网关起不来）。

use serde_json::{Map, Value};

use crate::server::config;
use crate::server::core::api_keys::KEY_API_KEY;
use crate::server::core::crypto::{self, SecretScope};
use crate::server::logging;

/// 把配置里的顶层 `apiKey` **原地**加密成 `enc1:` 信封（幂等，可重复调用）。
///
/// 返回迁移条数：`1` = 本次已加密并落库，`0` = 不需要迁移或失败（失败已在日志里
/// 说明，调用方不必再判断）。调用点是 `account_bootstrap::run`（紧邻
/// `api_keys::encrypt_legacy_keys`）—— 它必须等**配置快照就位**：本函数读
/// `config::current().raw()`，启动路径里 `config::init` 早于它、升级路径里调用方
/// 先做过 `config::reload()`（见 `api::upgrade_api`），两条路都成立。
///
/// 与 `encrypt_legacy_accounts` / `encrypt_legacy_keys` 的差别只有一处：后两者
/// 回写的是「整表 / 整个数组」，本函数回写的是**一个顶层键**，所以它走
/// `config::update_raw_field` 这个既有的「改单个键」入口（取舍见函数体里的注释）。
pub fn encrypt_legacy_api_key() -> usize {
    // 判据与待加密的值取自**同一份**快照（与 `encrypt_legacy_keys` 同一纪律）：
    // 分两次取会在「两次取值之间有人改了配置」时拿到自相矛盾的中间态 ——
    // 判据说该迁、真去写的时候值已经变了。快照本来就是克隆，不必为同一份 raw
    // 克隆两遍。
    let raw = config::current().raw().clone();
    let Some(pending) = pending_plaintext(&raw) else {
        // 没配 / 空值 / 已经是信封：走这条早退路径，一个字节都不写
        return 0;
    };
    let envelope = match crypto::seal(pending.as_bytes(), SecretScope::Accounts) {
        Ok(envelope) => envelope,
        Err(error) => {
            logging::log(
                "[Keys]",
                &format!(
                    "⚠️  旧字段 apiKey 加密未完成（下次启动会自动重试，本次 Key 照常可用）: {error}"
                ),
            );
            return 0;
        }
    };
    // 走 `config::update_raw_field`（config 层「改单个顶层键并落盘」的既有入口）：
    // 它一个动作里做完三件事 —— 落库、替换内存快照的 raw 底稿、同步
    // `RuntimeConfig::api_key`（掩码取值的那份）。少做任何一件都会留下自相矛盾的
    // 状态：内存里已是信封而磁盘还是明文（下次启动重来一遍）、或 raw 已换而
    // `api_key` 还是旧的（界面掩码显示错）。顺带让「改单个配置键的正确姿势」
    // 只有这一处实现，将来配置层换了写法也不会漏掉这里。
    if !config::update_raw_field(KEY_API_KEY, Value::String(envelope)) {
        // 落库失败时 `update_raw_field` 内部已经记过一条 `[Config]` 保存失败；
        // 这里再补一条 `[Keys]` 口径的：用户要知道的是「这把 Key 现在什么状态、
        // 我该做什么」，而那条错误只说了「保存失败」。
        logging::log(
            "[Keys]",
            "⚠️  旧字段 apiKey 已加密但未落盘（库不可用；下次启动会自动重试）",
        );
        return 0;
    }
    logging::log(
        "[Keys]",
        "🔒 已加密 1 把网关 Key（旧字段 apiKey；字段名与形态保持不变）",
    );
    1
}

/// 需要迁移的原始明文（`None` = 不需要迁移，一个字节都不写）。
///
/// 两条判据都在数据自己身上（见模块头的幂等论证）：
///   - **非空字符串**：非字符串 / 空串 / 只有空白都不算一把 Key —— 与
///     `api_keys::entries_from` 的兼容分支口径一致（那里同样在解密前拒绝空值）。
///     空串绝不能被加密：落盘后信封非空，读侧就会把它当成一把 Key，而它的明文是
///     空 —— 正好命中「带空 token 的请求」，等于把网关放开（这条危险的完整论证
///     在 `ApiKeyEntry::from_value` 的「不置空」那一条）。
///   - **不是 `enc1:` 信封**：判据的唯一来源是 `crypto::is_envelope`（不在这里
///     自己 `starts_with`，将来换格式才不会漏改一处）。已经是信封（含解不开的
///     那种）就什么都不做 —— 这是幂等的全部依据。
fn pending_plaintext(raw: &Map<String, Value>) -> Option<String> {
    let text = raw.get(KEY_API_KEY)?.as_str()?.trim();
    if text.is_empty() || crypto::is_envelope(text) {
        return None;
    }
    Some(text.to_string())
}
