//! 通用对称加解密：AES-256-GCM + `enc1:` 信封，按 `SecretScope` 分密钥域。
//!
//! ── 为什么抽成通用模块 ──────────────────────────────────────
//! 这套东西（32 字节随机密钥存**库外**、`enc1:<base64(nonce||密文+tag)>` 信封、
//! GCM 的 Postfix tag 拼法、解密失败时提示用户「重存一次即可自愈」）原先长在
//! `core::update::token` 里，只服务 GitHub 令牌。账号表（`accounts`）也要按同样的
//! 理由加密落库：配置库会被整库导出 / 备份 / 拷机迁移，明文凭证躺在里面等于
//! 「拷走一个文件就带走全部账号」。与其把那套实现复制第二份（两份会各自演进、
//! 迟早出现「一处改了另一处没改」的漏口），不如把**与业务无关**的那一半抽到这里：
//!   · 归本模块：算法、信封格式、密钥文件的加载/生成/进程内缓存、错误文案；
//!   · 留业务侧：明文怎么校验、解不开时要不要回落环境变量、对外 API 说什么
//!     （这些因令牌与账号而异，见 `update/token.rs` 的模块头）。
//!
//! ── 两个密钥域（`SecretScope`）──────────────────────────────
//!   `UpdateToken` → 密钥文件 `update-token.key`
//!   `Accounts`    → 密钥文件 `account.key`
//! 两个域**各有各的密钥、各有各的进程内缓存**，绝不共用：共用一把钥匙意味着拿到
//! 一处即解开另一处（令牌泄露 == 账号泄露），换钥匙时也没法只换一边。缓存分开是
//! 硬要求 —— 单槽缓存会让后加载的域拿着前一个域的钥匙去解密文，得到的是
//! 「解密失败」这种最难查的串钥匙故障。
//! 命名刻意错开：`core::key_scope.rs` 里的 `KeyScope` 是「请求命中的网关 API Key
//! 带来的提供商/模型限制」，与加密毫无关系，故这里叫 `SecretScope`。
//!
//! ── 密钥文件在 `config_dir`，与库同目录（强度边界要说清）──
//! 密钥文件与 `agent2api.db` 同在 `paths::config_dir()` 下。（`token.rs` 旧文里写的
//! 是 `config::config_dir()`，那只是转发 `paths::config_dir` 的一层薄壳，指向同一个
//! 目录；这里统一直接调 `paths::config_dir()`，少绕一层。）它挡的是**库被单独拿走**
//! 这一类泄露（导出 / 备份 / 拷库迁移），挡不住拿到整台机器或整个 `config_dir` 的人。
//! 这是本项目「零外部依赖的本地单机网关」定位决定的强度上限：真正的平台密钥库 /
//! KMS 要么要求平台专属代码、要么引入外部服务，都不是本模块的目标。这里只保证
//! 「单独打开数据库文件看不到凭证」。
//!
//! ── `AGENT2API_ACCOUNT_KEY` 兜底为什么必须存在 ──────────────
//! 密钥文件**丢了就再也解不开**已存的密文 —— 这是设计而非缺陷：能解开的密钥就
//! 等于明文。桌面形态下用户重存一次即可自愈，但 Docker 部署没有「用户重存」这个
//! 动作：容器重建时若 `config_dir` 没挂持久卷，密钥文件随容器一起消失，账号数据
//! 就成了永远解不开的死数据。因此账号域额外认一个环境变量 `AGENT2API_ACCOUNT_KEY`
//! （值 = 32 字节密钥的 **64 位十六进制**，便于写进 compose/.env，不必塞二进制）：
//! 把它与数据一起带在容器外，容器随便重建。
//! 未设置 / 长度不是 64 / 含非十六进制字符 → **回落密钥文件**（它是兜底而非唯一
//! 来源，配错不该让整个模块不可用），但留一条 verbose，免得「设了 env 却没用上」
//! 在生产里没人发现。
//! `UpdateToken` **不认**这个 env：GitHub 令牌本来就另有 `GITHUB_TOKEN` 环境变量
//! （见 `update/token.rs` 的 `env_token`），那是「令牌本体」的兜底，与「加密密钥」
//! 语义完全不同；挤进同一个变量名只会让「令牌到底从哪来」无从判断。
//!
//! ── 明文闸门：`open` 仍然校验 UTF-8 ──────────────────────────
//! 旧实现 `token.rs::decrypt` 返回 `String`，非法 UTF-8 给的是「解密结果不是合法
//! UTF-8 文本」，而 `status_json` 会把这句原样展示给用户 —— 它属于对外行为。
//! 所以闸门跟着实现一起搬进来：`open` 返回 `Vec<u8>`（对账号这类未来用途，字节比
//! 字符串更通用），但在返回前校验一次 UTF-8，保证所有既有调用点的错误路径逐字不变。
//! 本模块存储的都是文本凭证（令牌、账号 JSON），这道闸门不会挡掉合法数据。

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;

use crate::server::logging;

/// 密文信封的版本前缀（换算法时靠它识别旧格式，比如将来改用平台密钥库）
const ENVELOPE_PREFIX: &str = "enc1:";

/// 这段文本是不是本模块产出的密文信封（`enc1:` 开头）。
///
/// 供调用方判「某个位置存的是密文还是历史明文」——例如账号表 `data` 列升级前的
/// 行是明文 JSON、升级后是信封（见 `core::account_store::sql`）。为什么要暴露这个
/// 函数而不是让调用方各自 `starts_with("enc1:")`：那串前缀是**本模块的格式契约**，
/// 复制到外面就有了第二份「什么算信封」的事实来源，将来换格式（如 `enc2:`）必然
/// 漏改一处。判断口径只此一份（本函数），前缀也只此一份（上面的常量）。
pub fn is_envelope(text: &str) -> bool {
    text.starts_with(ENVELOPE_PREFIX)
}

/// GCM nonce 长度（96 bit，标准值）
const NONCE_LEN: usize = 12;

/// GCM 认证标签长度（128 bit）。只用于长度下限校验 —— tag 附在密文尾部，
/// 整段交给 aes-gcm 的 Postfix 语义处理，不自己切（见 `open` 的注释）
const TAG_LEN: usize = 16;

/// 密钥长度（AES-256）
const KEY_LEN: usize = 32;

/// 账号域密钥的环境变量名（值 = 32 字节密钥的 64 位十六进制，见模块头）
const ACCOUNT_KEY_ENV: &str = "AGENT2API_ACCOUNT_KEY";

/// 密钥域：决定用哪把密钥、哪个密钥文件、哪份进程内缓存。
///
/// 加一个域 = 加一个枚举值 + 给 `key_file` / `recovery_hint` 补一条分支，
/// 其余实现（信封、算法、缓存）原样复用。
#[derive(Clone, Copy)]
pub enum SecretScope {
    /// GitHub 令牌（`core::update::token`）。密钥文件名是历史事实，不许改
    UpdateToken,
    /// 账号表（`accounts`）。预留：支持 `AGENT2API_ACCOUNT_KEY` 兜底
    Accounts,
    /// 云同步的「同步密码」（`core::cloud_sync`）。
    ///
    /// ── 为什么必须是独立密钥域 ──────────────────────────────────
    /// 同步密码要用来加解密**远端那份 `account.key`**，而本机的账号域密钥本身
    /// 也在同步范围里。若拿账号域密钥去封同步密码，下载并替换 `account.key`
    /// 之后，本机保存的那份同步密码就再也解不开了 —— 用户会看到「密码明明
    /// 没改，却说密码错」。独立一把 `cloud-sync.key` 只属于这台机器、不参与
    /// 同步，这条死循环才不存在。
    CloudSync,
}

impl SecretScope {
    /// 密钥文件名（落在 `paths::config_dir()` 下，与库同目录）。
    ///
    /// `update-token.key` **必须逐字保持**：改名字等于换了密钥文件，已发布用户
    /// 机器上的 GitHub 令牌会全部解不开，而用户不会想到「升级一次就丢了令牌」。
    fn key_file(self) -> &'static str {
        match self {
            SecretScope::UpdateToken => "update-token.key",
            SecretScope::Accounts => "account.key",
            SecretScope::CloudSync => "cloud-sync.key",
        }
    }

    /// 解密失败时提示用户「怎么自愈」的后半句（前半句各域相同，见 `open`）。
    ///
    /// 按域区分而不是写死一句话：这句最终会经 `status_json` 显示到界面上，
    /// 把令牌的提示照抄给账号会让用户去重存一个不存在的东西。
    fn recovery_hint(self) -> &'static str {
        match self {
            SecretScope::UpdateToken => "重存一次令牌即可自愈",
            SecretScope::Accounts => "重新保存一次账号即可自愈",
            SecretScope::CloudSync => "在「设置 → 数据 → 云同步」里重新输入同步密码即可自愈",
        }
    }
}

// ─── 密钥（加载 / 首次生成，按域缓存在进程内）───────────────

/// 取某个域的缓存槽。
///
/// 为什么每个域一个 `OnceLock`：缓存必须**按域隔离**（串钥匙的后果见模块头），
/// 又要是「进程内一次性初始化」的静态量。两个独立静态变量比一张 `HashMap`
/// 更省：域是编译期已知的枚举，没有动态增删的场景。
fn cache_for(scope: SecretScope) -> &'static std::sync::Mutex<Option<[u8; KEY_LEN]>> {
    static UPDATE_TOKEN: std::sync::OnceLock<std::sync::Mutex<Option<[u8; KEY_LEN]>>> =
        std::sync::OnceLock::new();
    static ACCOUNTS: std::sync::OnceLock<std::sync::Mutex<Option<[u8; KEY_LEN]>>> =
        std::sync::OnceLock::new();
    static CLOUD_SYNC: std::sync::OnceLock<std::sync::Mutex<Option<[u8; KEY_LEN]>>> =
        std::sync::OnceLock::new();
    match scope {
        SecretScope::UpdateToken => UPDATE_TOKEN.get_or_init(|| std::sync::Mutex::new(None)),
        SecretScope::Accounts => ACCOUNTS.get_or_init(|| std::sync::Mutex::new(None)),
        SecretScope::CloudSync => CLOUD_SYNC.get_or_init(|| std::sync::Mutex::new(None)),
    }
}

/// 取某个域的加密密钥（没有就现场生成一份新的）。所有失败都以中文错误上抛。
///
/// 为什么用 `Mutex<Option<…>>` 而不是把密钥直接 `OnceLock` 进去：密钥要
/// **不存在就现场生成**，生成失败（目录不可写等）不能把失败缓存成永久状态 ——
/// 下次保存还能重试；同时首次并发的两个保存要串行（各自生成不同的密钥、互相
/// 覆盖文件，先存的那份密文就再也解不开了）。
fn secret_key(scope: SecretScope) -> Result<[u8; KEY_LEN], String> {
    let cell = cache_for(scope);
    let Ok(mut guard) = cell.lock() else {
        return Err("密钥锁中毒".to_string());
    };
    if let Some(key) = *guard {
        return Ok(key);
    }
    let key = resolve_key(scope)?;
    *guard = Some(key);
    Ok(key)
}

/// 定出这个域当前该用哪把密钥：环境变量兜底（只有账号域有）优先于密钥文件。
///
/// 环境变量与密钥文件**都是「同一把密钥」的两种携带方式**，不是两把：Docker 场景
/// 里 env 就是那份密钥文件的替身。所以这里先读 env、读不到才去碰文件；env 一旦
/// 有效就不再落盘（不生成也不需要密钥文件）——容器里凭空多出一个密钥文件反而是
/// 又一个要跟着容器生死的东西。
fn resolve_key(scope: SecretScope) -> Result<[u8; KEY_LEN], String> {
    if let Some(key) = env_key(scope) {
        return Ok(key);
    }
    load_or_create_key(scope)
}

/// 环境变量里的密钥（只有账号域认，见模块头）。未设置 / 非法一律 None。
fn env_key(scope: SecretScope) -> Option<[u8; KEY_LEN]> {
    // `UpdateToken` 刻意没有 env 兜底：令牌本体另有 GITHUB_TOKEN（见模块头）
    let SecretScope::Accounts = scope else {
        return None;
    };
    let Some(raw) = std::env::var(ACCOUNT_KEY_ENV).ok() else {
        return None;
    };
    match decode_hex_key(&raw) {
        Some(key) => Some(key),
        None => {
            // 静默回落密钥文件，但必须留痕：env 是「密钥丢了也能救回来」的唯一
            // 手段，配错了却没人知道，等于这个兜底不存在
            logging::verbose(
                "[Crypto]",
                &format!(
                    "{ACCOUNT_KEY_ENV} 不是 64 位十六进制（实际 {} 个字符），本次回落密钥文件",
                    raw.trim().len()
                ),
            );
            None
        }
    }
}

/// 64 个十六进制字符 → 32 字节密钥（大小写均可）。非法返回 None。
///
/// 手写而不是引 `hex` crate：只有这一处用到，为 20 行代码加一个依赖不划算
/// （本项目「不新增第三方依赖」的约定，见 `Cargo.toml` 的依赖清单注释）。
fn decode_hex_key(value: &str) -> Option<[u8; KEY_LEN]> {
    let text = value.trim();
    if text.len() != KEY_LEN * 2 {
        return None;
    }
    let bytes = text.as_bytes();
    let mut key = [0u8; KEY_LEN];
    for (index, slot) in key.iter_mut().enumerate() {
        let high = hex_nibble(bytes[index * 2])?;
        let low = hex_nibble(bytes[index * 2 + 1])?;
        *slot = (high << 4) | low;
    }
    Some(key)
}

/// 单个十六进制字符 → 4 bit
fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// 读该域的密钥文件；不存在则生成并写出（Unix 上收紧到 0600）。
fn load_or_create_key(scope: SecretScope) -> Result<[u8; KEY_LEN], String> {
    let path = crate::paths::config_dir().join(scope.key_file());
    match std::fs::read(&path) {
        Ok(bytes) => match <[u8; KEY_LEN]>::try_from(&bytes[..]) {
            Ok(key) => Ok(key),
            Err(_) => Err(format!(
                "密钥文件 {} 长度异常（期望 {KEY_LEN} 字节，实际 {} 字节）",
                path.display(),
                bytes.len()
            )),
        },
        // 不存在才生成新密钥；其余错误（权限等）如实上抛 —— 覆盖式重建会把
        // 已保存的密文变成永远解不开的死数据
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => create_key_file(&path),
        Err(error) => Err(format!("读取密钥文件 {} 失败: {error}", path.display())),
    }
}

/// 生成 32 字节随机密钥并写盘
fn create_key_file(path: &std::path::Path) -> Result<[u8; KEY_LEN], String> {
    let mut key = [0u8; KEY_LEN];
    getrandom::getrandom(&mut key).map_err(|error| format!("系统随机源不可用: {error}"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|error| format!("创建配置目录失败: {error}"))?;
    }
    std::fs::write(path, key).map_err(|error| format!("写入密钥文件 {} 失败: {error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(key)
}

// ─── 密钥指纹（只读，绝不触发生成）───────────────────────────

/// 只读地取该域**当下实际生效**的密钥；不存在 / 不可读 / 长度异常 → `None`。
///
/// 与 `secret_key`（取或建）的本质区别：本函数**没有任何写盘动作**，所以可以用在
/// 「只是看看现在用的是哪把钥匙」的纯读场景（如整库导出时写指纹）。绝不能复用
/// `secret_key`：那条路径在密钥文件不存在时会**现场生成**一份新密钥 —— 一个纯读
/// 的导出动作顺手在用户机器上凭空造出 `account.key`，等于悄悄改变了本机的加密
/// 状态（下次真正保存账号时用的就是这把「导出时生出来的」钥匙，语义完全错位）。
///
/// 解析顺序必须与 `resolve_key` 逐字对齐（缓存 → 环境变量 → 密钥文件），否则指纹
/// 会代表一把**不是**当下加解密实际在用的钥匙，比对结果就成了误导。
fn existing_key(scope: SecretScope) -> Option<[u8; KEY_LEN]> {
    // 1) 进程内已解析过：它就是本进程 seal/open 正在用的钥匙，优先级最高
    if let Ok(guard) = cache_for(scope).lock() {
        if let Some(key) = *guard {
            return Some(key);
        }
    }
    // 2) 环境变量兜底（只有账号域认，见模块头）；与 resolve_key 同一优先级
    if let Some(key) = env_key(scope) {
        return Some(key);
    }
    // 3) 密钥文件：只读；「不存在」这一支**不生成**（与 load_or_create_key 的分岔点）
    let path = crate::paths::config_dir().join(scope.key_file());
    let bytes = std::fs::read(&path).ok()?;
    <[u8; KEY_LEN]>::try_from(&bytes[..]).ok()
}

/// 该域现有密钥的**指纹**（`sha256(密钥原文)` 的十六进制前 16 位）；密钥不存在
/// 或不可读 → `None`。
///
/// 为什么是哈希而不是密钥本身：指纹要写进**整库备份的文件头**，而备份是明文 SQL、
/// 会被拷到别的机器 / 云端。若把密钥写进去，等于把钥匙和锁一起交出去（备份文件
/// 从此成了明文凭证）。单向哈希只回答「是不是同一把钥匙」，反推不出密钥；取前
/// 16 位（64 bit）足以区分「换过密钥」，又短到适合放进一行注释。
///
/// 调用方是 `db::backup`：导出时写进文件头，导入时与本机指纹比对，不一致就明确
/// 提示「这份备份是用另一把密钥加密的」，而不是让用户对着空账号列表发懵。
pub fn key_fingerprint(scope: SecretScope) -> Option<String> {
    let key = existing_key(scope)?;
    Some(short_fingerprint(&key))
}

/// `sha256(bytes)` 的十六进制前 16 位 —— 本模块的指纹口径，只此一份。
fn short_fingerprint(bytes: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    let digest = Sha256::digest(bytes);
    let mut text = String::with_capacity(16);
    for byte in digest.iter().take(8) {
        use std::fmt::Write as _;
        let _ = write!(text, "{byte:02x}");
    }
    text
}

/// 公开的指纹口径：任意字节 → 前 16 位十六进制。
///
/// 云同步要用它给**远端那份 `account.key.enc` 里封装的密钥**算指纹（与
/// [`key_fingerprint`] 对本地密钥算的是同一口径），才能回答「远端那把钥匙是不是
/// 我现在这把」。不暴露这个函数的话调用方会自己写一遍 sha256 截断 —— 两处口径
/// 一旦漂移，指纹比对就会给出错误的「一致 / 不一致」结论。
pub fn fingerprint_of(bytes: &[u8]) -> String {
    short_fingerprint(bytes)
}

// ─── 缓存重置（云同步专用）───────────────────────────────────

/// 丢掉某个域的进程内密钥缓存，下一次 `seal` / `open` 重新从磁盘（或环境变量）解析。
///
/// ── 唯一调用方是云同步下载 ────────────────────────────────────
/// 下载会**替换 `account.key`**（把远端那份装到本机）。而 `secret_key` 把密钥缓存在
/// 进程内，不重置的话本次运行剩下的时间里账号表用的还是**旧密钥** —— 新写进去的
/// 密文用旧钥匙封、下一次启动用新钥匙开，账号全部解不开。这是云同步最容易踩的
/// 一个坑，所以重置入口放在 crypto 自己这里（而不是让调用方去碰私有缓存）。
///
/// `Accounts` 之外的域不会被替换，重置它们只是多一次读文件，但**不做**：调用方
/// 只该传真的换过密钥的那个域，保留「重置 = 我确实换了文件」的语义。
pub fn reset_cache(scope: SecretScope) {
    if let Ok(mut guard) = cache_for(scope).lock() {
        *guard = None;
    }
}

/// 该域密钥文件的**路径**（不读、不生成）。云同步据此做备份 / 替换。
pub fn key_file_path(scope: SecretScope) -> std::path::PathBuf {
    crate::paths::config_dir().join(scope.key_file())
}

/// 只读地取该域密钥的原始 32 字节（不生成新密钥，见 [`existing_key`]）。
///
/// 云同步上传时用它取 `account.key` 的字节去加密上云；本机还没有密钥
/// （从未保存过账号）时返回 `None`，那种情况不需要同步密钥文件。
pub fn existing_key_bytes(scope: SecretScope) -> Option<[u8; KEY_LEN]> {
    existing_key(scope)
}

// ─── 加密 / 解密（对外）─────────────────────────────────────

/// 加密明文 → `enc1:<base64(nonce||密文+tag)>`。
///
/// nonce 每次现生成（GCM 的 nonce 重用是灾难性的：同一密钥下两条明文用同一 nonce
/// 会互相泄底），随机性来自系统熵源；不用计数器 —— 进程重启后计数没有持久化，
/// 随机 nonce 才是这里站得住的选择。
pub fn seal(plaintext: &[u8], scope: SecretScope) -> Result<String, String> {
    use aes_gcm::aead::{consts::U12, Aead as _};
    use aes_gcm::{Aes256Gcm, KeyInit as _, Nonce};

    let key = secret_key(scope)?;
    let cipher =
        Aes256Gcm::new_from_slice(&key).map_err(|_| "密钥长度不受 AES-256 支持".to_string())?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    getrandom::getrandom(&mut nonce_bytes).map_err(|error| format!("系统随机源不可用: {error}"))?;
    let nonce =
        Nonce::<U12>::try_from(nonce_bytes.as_slice()).map_err(|_| "生成 nonce 失败".to_string())?;
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|_| "AES-GCM 加密失败".to_string())?;
    let mut blob = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    blob.extend_from_slice(&nonce_bytes);
    blob.extend_from_slice(&ciphertext);
    Ok(format!("{ENVELOPE_PREFIX}{}", BASE64.encode(&blob)))
}

/// 解开一段 `enc1:` 信封 → 明文字节（已校验是合法 UTF-8，见模块头）。
///
/// 失败原因都是「能照着做」的提示：格式不对 / base64 坏 / 长度不够 / 密钥不匹配。
/// 密钥不匹配那句会把**具体密钥文件名**带出去 —— 用户要知道该去找哪个文件，
/// 这句会经 `status_json` 显示到界面上。
pub fn open(envelope: &str, scope: SecretScope) -> Result<Vec<u8>, String> {
    use aes_gcm::aead::{consts::U12, Aead as _};
    use aes_gcm::{Aes256Gcm, KeyInit as _, Nonce};

    let Some(encoded) = envelope.trim().strip_prefix(ENVELOPE_PREFIX) else {
        return Err("密文格式不受支持（不是 enc1 信封）".to_string());
    };
    let blob = BASE64
        .decode(encoded)
        .map_err(|_| "密文不是合法 base64".to_string())?;
    // 长度下限：nonce + GCM tag（16）。aes-gcm 的 tag 附在密文尾部，
    // 这里整段交给 decrypt（Postfix 语义，与 autoclaw/crypto.rs 同一套）
    if blob.len() <= NONCE_LEN + TAG_LEN {
        return Err("密文长度异常".to_string());
    }
    // 密钥在长度校验之后才取：坏信封不该触发「现场生成密钥文件」这类副作用
    let key = secret_key(scope)?;
    let cipher =
        Aes256Gcm::new_from_slice(&key).map_err(|_| "密钥长度不受 AES-256 支持".to_string())?;
    let nonce = Nonce::<U12>::try_from(&blob[..NONCE_LEN])
        .map_err(|_| "密文 nonce 长度异常".to_string())?;
    let plaintext = cipher.decrypt(&nonce, &blob[NONCE_LEN..]).map_err(|_| {
        format!(
            "AES-GCM 解密失败（密钥文件 {} 是否被删除或更换？{}）",
            scope.key_file(),
            scope.recovery_hint()
        )
    })?;
    // UTF-8 闸门：旧实现 `token.rs::decrypt` 的这条文案会被界面原样展示，
    // 属于对外行为，不能因为返回值换成字节就漏掉（见模块头）
    std::str::from_utf8(&plaintext).map_err(|_| "解密结果不是合法 UTF-8 文本".to_string())?;
    Ok(plaintext)
}
