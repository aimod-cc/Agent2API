//! Rule expressions adapted from yetone/magpie, MIT (see licenses/magpie-MIT).
//! Source: d847b79690cb12951ceefbb53f51993abac76b43, internal/redact/{redact,more,crypto}.go.
//! RE2 ASCII shorthand was made explicit for Rust regex; guards are in guards.rs and wallet.rs.
use super::{Capture, Category, Guard, IdRule, Spec};

#[rustfmt::skip]
pub(super) const BUILTINS: &[Spec] = &[
    // redact.go:171 (PRIVATE_KEY)
    Spec { category: Category::Secrets, pattern: r#"-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY(?: BLOCK)?-----(?s:.)+?-----END (?:[A-Z0-9]+ )*PRIVATE KEY(?: BLOCK)?-----"#, capture: Capture::First, markers: &["PRIVATE KEY"], id_rule: IdRule::Credential, ..Spec::DEFAULT },
    // redact.go:172 (API_KEY)
    Spec { category: Category::Secrets, pattern: r#"sk-(?:ant-|proj-|or-|svcacct-|admin-)?[A-Za-z0-9_-]{20,}"#, capture: Capture::First, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-", check: Guard::NotASlug, markers: &["sk-"], id_rule: IdRule::Credential, ..Spec::DEFAULT },
    // redact.go:174 (API_KEY)
    Spec { category: Category::Secrets, pattern: r#"[a-z]{2,4}_sk_[A-Za-z0-9_-]{20,}"#, capture: Capture::First, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-", check: Guard::SecretValue, markers: &["_sk_"], id_rule: IdRule::Credential, ..Spec::DEFAULT },
    // redact.go:177 (API_KEY)
    Spec { category: Category::Secrets, pattern: r#"ark-[A-Za-z0-9][A-Za-z0-9_-]{15,}"#, capture: Capture::First, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-", check: Guard::SecretValue, markers: &["ark-"], id_rule: IdRule::Credential, ..Spec::DEFAULT },
    // redact.go:178 (API_KEY)
    Spec { category: Category::Secrets, pattern: r#"(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{40,}|glpat-[A-Za-z0-9_-]{20,})"#, capture: Capture::First, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-", markers: &["gh", "github_pat_", "glpat-"], id_rule: IdRule::Credential, ..Spec::DEFAULT },
    // redact.go:179 (API_KEY)
    Spec { category: Category::Secrets, pattern: r#"AIza[0-9A-Za-z_-]{35}"#, capture: Capture::First, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-", markers: &["AIza"], id_rule: IdRule::Credential, ..Spec::DEFAULT },
    // redact.go:180 (API_KEY)
    Spec { category: Category::Secrets, pattern: r#"xox[abposr]-[0-9A-Za-z-]{10,}"#, capture: Capture::First, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-", markers: &["xox"], id_rule: IdRule::Credential, ..Spec::DEFAULT },
    // redact.go:181 (API_KEY)
    Spec { category: Category::Secrets, pattern: r#"(?:sk|rk)_live_[0-9A-Za-z]{20,}"#, capture: Capture::First, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-", markers: &["_live_"], id_rule: IdRule::Credential, ..Spec::DEFAULT },
    // redact.go:182 (API_KEY)
    Spec { category: Category::Secrets, pattern: r#"(?:AKIA|ASIA)[0-9A-Z]{16}"#, capture: Capture::First, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789", markers: &["AKIA", "ASIA"], id_rule: IdRule::Credential, ..Spec::DEFAULT },
    // redact.go:183 (API_KEY)
    Spec { category: Category::Secrets, pattern: r#"(?:hf_[A-Za-z0-9]{30,}|gsk_[A-Za-z0-9]{40,}|xai-[A-Za-z0-9]{40,}|npm_[A-Za-z0-9]{36}|pypi-[A-Za-z0-9_-]{50,}|dop_v1_[a-f0-9]{64}|SG\.[A-Za-z0-9_-]{22}\.[A-Za-z0-9_-]{43})"#, capture: Capture::First, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-", markers: &["hf_", "gsk_", "xai-", "npm_", "pypi-", "dop_v1_", "SG."], id_rule: IdRule::Credential, ..Spec::DEFAULT },
    // redact.go:194 (API_KEY)
    Spec { category: Category::Secrets, pattern: r#"cf(?:ut|at|k|oat)_[A-Za-z0-9_-]{40,}"#, capture: Capture::First, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-", markers: &["cfut_", "cfat_", "cfk_", "cfoat_"], id_rule: IdRule::Credential, ..Spec::DEFAULT },
    // redact.go:195 (API_KEY)
    Spec { category: Category::Secrets, pattern: r#"v1\.0-[A-Za-z0-9]{16,}-[A-Za-z0-9]{40,}"#, capture: Capture::First, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-", markers: &["v1.0-"], id_rule: IdRule::Credential, ..Spec::DEFAULT },
    // redact.go:196 (TOKEN)
    Spec { category: Category::Secrets, pattern: r#"eyJhIjoi[A-Za-z0-9+/_-]{40,}={0,2}"#, capture: Capture::First, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-", markers: &["eyJhIjoi"], id_rule: IdRule::Credential, ..Spec::DEFAULT },
    // redact.go:199 (SECRET)
    Spec { category: Category::Secrets, pattern: r#"(?i)x-auth-(?:user-service-)?key["']?[ \t]*[:=][ \t]*["']?([A-Za-z0-9._-]{16,})"#, capture: Capture::First, check: Guard::NotAVariable, fold: &["x-auth-"], id_rule: IdRule::Assignment, ..Spec::DEFAULT },
    // redact.go:204 (TOKEN)
    Spec { category: Category::Secrets, pattern: r#"eyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}"#, capture: Capture::First, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-", markers: &["eyJ"], id_rule: IdRule::Credential, ..Spec::DEFAULT },
    // redact.go:207 (PASSWORD)
    Spec { category: Category::Secrets, pattern: r#"[A-Za-z][A-Za-z0-9+.-]*://[^ \t\n\r\x0C:@/?#"'<>]*:([^ \t\n\r\x0C@/?#"'<>]{3,})@"#, capture: Capture::First, check: Guard::NotAVariable, context: Guard::HiddenInURL, markers: &["://"], id_rule: IdRule::Credential, ..Spec::DEFAULT },
    // redact.go:209 (SECRET)
    Spec { category: Category::Secrets, pattern: r#"(?i)[A-Za-z0-9_.-]*(?:password|passwd|passphrase|secret|token|api[_-]?key|access[_-]?key|private[_-]?key|priv[_-]?key|credential)[A-Za-z0-9_.-]*(?:\\?["'])?[ \t]*[:=][ \t]*(?:\\?["'])?([A-Za-z0-9_\-./+=~!@#%^&*]{8,})"#, capture: Capture::First, check: Guard::SecretValue, context: Guard::FieldAt, field: true, id_rule: IdRule::Assignment, ..Spec::DEFAULT },
    // redact.go:217 (SECRET)
    Spec { category: Category::Secrets, pattern: r#"(?:^|[^A-Za-z0-9_.-])[A-Za-z0-9_.-]*_(?:KEY|PASS)(?:\\?["'])?[ \t]*[:=][ \t]*(?:\\?["'])?([A-Za-z0-9_\-./+=~!@#%^&*]{8,})"#, capture: Capture::First, check: Guard::SecretValue, context: Guard::NamedSecretAt, field: true, id_rule: IdRule::Assignment, ..Spec::DEFAULT },
    // redact.go:221 (SECRET)
    Spec { category: Category::Secrets, pattern: r#"(?im)^[ \t]*(?:authorization|cookie)[ \t]*:[ \t]*([^ \t\n\r\x0C][^\r\n]{7,})"#, capture: Capture::First, check: Guard::HeaderValue, id_rule: IdRule::Assignment, ..Spec::DEFAULT },
    // redact.go:226 (SECRET)
    Spec { category: Category::Secrets, pattern: r#"(?i)(?:^|[^A-Za-z0-9_.-])(?:\\?["'])?key(?:\\?["'])?[ \t]*[:=][ \t]*(?:\\?["'])?([A-Za-z0-9_\-./+=~!@#%^&*]{16,})"#, capture: Capture::First, check: Guard::SecretValue, context: Guard::FieldAt, field: true, id_rule: IdRule::Assignment, ..Spec::DEFAULT },
    // redact.go:233 (PASSWORD)
    Spec { category: Category::Secrets, pattern: r#"(?i)(?:密码|密碼|口令|PIN码|PIN碼|PIN 码|安全码|安全碼|パスワード|暗証番号|비밀번호|암호|비번)[ \t]*(?:[:：=][ \t]*|[为為是][ \t]*[:：]?[ \t]*|は[ \t]*|는[ \t]*)?(?:\\?["'“「])?([A-Za-z0-9_\-./+=~!@#%^&*]{3,63}[A-Za-z0-9_\-/+=~!@#%^&*])"#, capture: Capture::First, check: Guard::NotAVariable, field: true, id_rule: IdRule::Credential, ..Spec::DEFAULT },
    // redact.go:238 (SECRET)
    Spec { category: Category::Secrets, pattern: r#"(?i)(?:密钥|密鑰|秘钥|秘鑰|私钥|私鑰|金钥|金鑰|令牌|权杖|權杖|授权码|授權碼|秘密鍵|トークン|シークレット|APIキー|アクセスキー|토큰|비밀키|API 키|API키)[ \t]*(?:[:：=][ \t]*|[为為是][ \t]*[:：]?[ \t]*|は[ \t]*|는[ \t]*)?(?:\\?["'“「])?([A-Za-z0-9_\-./+=~!@#%^&*]{3,63}[A-Za-z0-9_\-/+=~!@#%^&*])"#, capture: Capture::First, context: Guard::KeyAt, field: true, id_rule: IdRule::Assignment, ..Spec::DEFAULT },
    // redact.go:242 (SECRET)
    Spec { category: Category::Secrets, pattern: r#"(?i)(?:password|passwd|passphrase|secret|token|api[_-]?key|access[_-]?key|private[_-]?key|priv[_-]?key|credential|pwd|api[ ]key|access[ ]key|secret[ ]key|key|(?-u:\b)pin(?:[ ]?code)?)(?:\\?["'])?[ \t]*(?:[为為是][ \t]*[:：]?|：)[ \t]*(?:\\?["'“「])?([A-Za-z0-9_\-./+=~!@#%^&*]{3,}[A-Za-z0-9_\-/+=~!@#%^&*])"#, capture: Capture::First, context: Guard::KeyAt, field: true, fold: &["为", "為", "是", "："], id_rule: IdRule::Assignment, ..Spec::DEFAULT },
    // redact.go:246 (EMAIL)
    Spec { category: Category::Personal, pattern: r#"[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}"#, capture: Capture::First, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_%+-", check: Guard::RealEmail, context: Guard::EmailAt, markers: &["@"], ..Spec::DEFAULT },
    // redact.go:247 (ID_CARD)
    Spec { category: Category::Personal, pattern: r#"[1-9][0-9]{5}(?:18|19|20)[0-9]{2}(?:0[1-9]|1[0-2])(?:0[1-9]|[12][0-9]|3[01])[0-9]{3}[0-9Xx]"#, capture: Capture::First, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789", check: Guard::ChineseID, ..Spec::DEFAULT },
    // redact.go:248 (PHONE)
    Spec { category: Category::Personal, pattern: r#"(?:\+?86[- ]?)?1[3-9][0-9]{9}"#, capture: Capture::First, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789", context: Guard::PhoneAt, ..Spec::DEFAULT },
    // redact.go:249 (BANK_CARD)
    Spec { category: Category::Personal, pattern: r#"[3-6][0-9]{3}(?:[ -]?[0-9]{4}){2,3}(?:[ -]?[0-9]{1,3})?"#, capture: Capture::First, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789", check: Guard::Luhn, ..Spec::DEFAULT },
    // more.go:85 (SSN)
    Spec { category: Category::Ssn, pattern: r#"[0-9]{3}-[0-9]{2}-[0-9]{4}"#, capture: Capture::First, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-", check: Guard::SsnOK, markers: &["-"], ..Spec::DEFAULT },
    // more.go:86 (SSN)
    Spec { category: Category::Ssn, pattern: r#"(?i:(?-u:\b)ssn|(?-u:\b)social[ \t]+security(?:[ \t]+(?:number|no\.?|#))?)["']?[ \t]*(?:[:#=：][ \t]*)?(?:(?i:is)[ \t]+)?["']?([0-9]{9}|[0-9]{3} [0-9]{2} [0-9]{4})"#, capture: Capture::First, bound: "0123456789", check: Guard::SsnOK, fold: &["ssn", "social"], ..Spec::DEFAULT },
    // more.go:89 (PASSPORT)
    Spec { category: Category::Passport, pattern: r#"(?i:(?-u:\b)passport(?:[ \t]*(?:no\.?|number|num|#))?|(?-u:\b)reisepass(?:nummer)?)["']?[ \t]*(?:[:#=：][ \t]*)?(?:(?i:is|was)[ \t]+)?["']?([A-Z0-9]{6,9})|(?:护照(?:号码|号)?|護照(?:號碼|号码|番号)?|パスポート(?:番号)?)[ \t]*(?:[:：=][ \t]*)?(?:是[ \t]*|は[ \t]*)?["']?([A-Z0-9]{6,9})"#, capture: Capture::Any, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789", check: Guard::PassportOK, fold: &["passport", "reisepass", "护照", "護照", "パスポート"], ..Spec::DEFAULT },
    // more.go:95 (IBAN)
    Spec { category: Category::Iban, pattern: r#"[A-Z]{2}[0-9]{2}(?:[A-Z0-9]{11,30}|(?: [A-Z0-9]{4}){2,7}(?: [A-Z0-9]{1,3})?)"#, capture: Capture::First, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789", repair: Guard::IbanAt, ..Spec::DEFAULT },
    // more.go:98 (BIRTHDAY)
    Spec { category: Category::Birthday, pattern: r#"(?:(?i:(?-u:\b)date[ \t]+of[ \t]+birth|(?-u:\b)birth[ \t_-]?date|(?-u:\b)birthday|(?-u:\b)dob(?-u:\b)|(?-u:\b)d\.o\.b\.?|(?-u:\b)born(?:[ \t]+on)?|(?-u:\b)geburtsdatum|(?-u:\b)geboren(?:[ \t]+am)?)|生日|出生日期|出生年月日|出生于|出生於|出生|誕生日|生年月日)["']?[ \t]*(?:[:#=：][ \t]*)?(?:(?i:is|was)[ \t]+|是[ \t]*|为[ \t]*|為[ \t]*|は[ \t]*)?["']?([0-9]{4}[-/.][0-9]{1,2}[-/.][0-9]{1,2}|[0-9]{4}[ \t]*年[ \t]*[0-9]{1,2}[ \t]*月[ \t]*[0-9]{1,2}[ \t]*日?|[0-9]{1,2}[-/.][0-9]{1,2}[-/.][0-9]{4}|(?i:jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)[a-z]*\.?[ \t]+[0-9]{1,2}(?:st|nd|rd|th)?,?[ \t]+[0-9]{4}|[0-9]{1,2}(?:st|nd|rd|th)?[ \t]+(?i:jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)[a-z]*\.?,?[ \t]+[0-9]{4})"#, capture: Capture::First, bound: "0123456789", check: Guard::DateOK, fold: &["birth", "dob", "d.o.b", "born", "geburt", "geboren", "生日", "出生", "誕生日", "生年月日"], ..Spec::DEFAULT },
    // more.go:107 (MAC)
    Spec { category: Category::Mac, pattern: r#"[0-9A-Fa-f]{2}(?::[0-9A-Fa-f]{2}){5}|[0-9A-Fa-f]{2}(?:-[0-9A-Fa-f]{2}){5}"#, capture: Capture::First, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789:-", check: Guard::MacOK, ..Spec::DEFAULT },
    // more.go:111 (SERIAL)
    Spec { category: Category::Serial, pattern: r#"(?i:(?-u:\b)serial[ \t_-]?(?:number|num|no\.?|#)?|(?-u:\b)s/n|(?-u:\b)sn)(?:[ \t]*\([^)\n]{1,20}\))?["']?[ \t]*[:#=：][ \t]*["']?([A-Za-z0-9][A-Za-z0-9-]{4,39})|(?:序列号|序列號|シリアル番号|(?i:seriennummer))[ \t]*[:：=]?[ \t]*["']?([A-Za-z0-9][A-Za-z0-9-]{4,39})"#, capture: Capture::Any, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-", check: Guard::SerialOK, fold: &["serial", "s/n", "sn", "序列", "シリアル", "seriennummer"], ..Spec::DEFAULT },
    // more.go:118 (USERNAME)
    Spec { category: Category::Home, pattern: r#"(?:^|[^A-Za-z0-9_.~/\\-])(?:file://)?(?:/mnt/[a-z])?/(?:Users|home)/([^ \t\n\r\x0C/\\:*?"'<>|,;()\[\]{}`$%]{1,64})"#, capture: Capture::First, repair: Guard::HomeAt, fold: &["/users/", "/home/"], ..Spec::DEFAULT },
    // more.go:121 (USERNAME)
    Spec { category: Category::Home, pattern: r#"(?:^|[^A-Za-z0-9])[A-Za-z]:(?:\\{1,4}|/)(?i:users|documents and settings)(?:\\{1,4}|/)(?:([A-Za-z0-9._-]+(?: [A-Za-z0-9._-]+)+)(?:\\|/)|([^ \t\n\r\x0C/\\:*?"'<>|,;()\[\]{}`$%]{1,64}))"#, capture: Capture::Any, repair: Guard::HomeAt, fold: &["users", "documents and settings"], ..Spec::DEFAULT },
    // more.go:130 (USERNAME, HOSTNAME)
    Spec { category: Category::Userhost, pattern: r#"(?m)(?:^|[^A-Za-z0-9._%+-])([a-z_][a-z0-9_.-]{0,31})@([A-Za-z0-9][A-Za-z0-9-]{0,62}(?:\.[A-Za-z0-9-]{1,63})*)(?::[~/]|:[ \t]*$|[ \t]+[~/]|[ \t]*[$%#>][ \t]|[ \t]*[$%#>]$)"#, capture: Capture::Groups(&[Guard::UserOK, Guard::HostOK]), context: Guard::NotPackagePin, markers: &["@"], ..Spec::DEFAULT },
    // more.go:132 (USERNAME, HOSTNAME)
    Spec { category: Category::Userhost, pattern: r#"(?i:(?-u:\b)(?:ssh|scp|sftp|rsync|mosh|ssh-copy-id))(?-u:\b)[^\n@]{0,80}?[ \t"']([a-z_][a-z0-9_.-]{0,31})@([A-Za-z0-9][A-Za-z0-9-]{0,62}(?:\.[A-Za-z0-9-]{1,63})*)"#, capture: Capture::Groups(&[Guard::UserOK, Guard::HostOK]), context: Guard::NotPackagePin, markers: &["@"], ..Spec::DEFAULT },
    // more.go:136 (HOSTNAME)
    Spec { category: Category::Userhost, pattern: r#"(?m)^[ \t]*["']?(?:(?i:host[ \t_-]?name|computer[ \t_-]?name|machine[ \t_-]?name|device[ \t_-]?name)|主机名|主機名|計算機名|コンピュータ名|コンピューター名)["']?[ \t]*[:=：][ \t]*(?:["']([A-Za-z0-9][A-Za-z0-9-]{0,62}(?:\.[A-Za-z0-9-]{1,63})*)["'][ \t]*[,;]?|([A-Za-z0-9][A-Za-z0-9-]{0,62}(?:\.[A-Za-z0-9-]{1,63})*))[ \t]*$|^[ \t]+HostName[ \t]+([A-Za-z0-9][A-Za-z0-9-]{0,62}(?:\.[A-Za-z0-9-]{1,63})*)[ \t]*$"#, capture: Capture::Any, check: Guard::LineHostOK, fold: &["name", "主机", "主機", "計算機", "コンピュータ"], ..Spec::DEFAULT },
    // more.go:142 (USERNAME)
    Spec { category: Category::Userhost, pattern: r#"(?m)^[ \t]*["']?(?:(?i:user[ \t_-]?name|login[ \t_-]?name|user|login|logname)|用户名|用戶名|ユーザー名)["']?[ \t]*[:=：][ \t]*(?:["']([A-Za-z0-9_][A-Za-z0-9._-]{0,31})["'][ \t]*[,;]?|([A-Za-z0-9_][A-Za-z0-9._-]{0,31}))[ \t]*$|^[ \t]+User[ \t]+([A-Za-z0-9_][A-Za-z0-9._-]{0,31})[ \t]*$"#, capture: Capture::Any, check: Guard::LineUserOK, context: Guard::NotLineIdent, fold: &["user", "login", "logname", "用户", "用戶", "ユーザー"], ..Spec::DEFAULT },
    // more.go:149 (IP)
    Spec { category: Category::Ip, pattern: r#"(?:25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9]?[0-9])(?:\.(?:25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9]?[0-9])){3}"#, capture: Capture::First, context: Guard::Ipv4At, markers: &["."], ..Spec::DEFAULT },
    // more.go:151 (IP)
    Spec { category: Category::Ip, pattern: r#"[0-9A-Fa-f]{1,4}(?::[0-9A-Fa-f]{0,4}){2,7}"#, capture: Capture::First, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789:_", check: Guard::Ipv6OK, markers: &[":"], ..Spec::DEFAULT },
    // more.go:156 (BUCKET, BUCKET)
    Spec { category: Category::Bucket, pattern: r#"(?i:(?-u:\b)(?:s3a?|s3n|gs|oss|cosn?|obs|r2|ks3|bos|tos|az|azure|wasbs?|abfss?)://)([A-Za-z0-9](?:[A-Za-z0-9._-]{0,61}[A-Za-z0-9])?)(?:@([a-z0-9]{3,24})\.(?:blob|dfs)\.core\.windows\.net)?"#, capture: Capture::Groups(&[Guard::BucketOK, Guard::BucketOK]), markers: &["://"], ..Spec::DEFAULT },
    // more.go:158 (BUCKET)
    Spec { category: Category::Bucket, pattern: r#"(?:^|[^A-Za-z0-9.-])([a-z0-9](?:[a-z0-9.-]{0,61}[a-z0-9])?)\.(?:s3(?:[.-](?:dualstack\.)?[a-z0-9-]+)?\.amazonaws\.com(?:\.cn)?|s3-accelerate\.amazonaws\.com|storage\.googleapis\.com|oss-[a-z0-9-]+\.aliyuncs\.com|cos\.[a-z0-9-]+\.myqcloud\.com|obs\.[a-z0-9-]+\.myhuaweicloud\.com|r2\.cloudflarestorage\.com|(?:blob|dfs)\.core\.windows\.net|[a-z0-9-]+\.digitaloceanspaces\.com|tos-[a-z0-9-]+\.volces\.com)"#, capture: Capture::First, check: Guard::BucketOK, fold: &["amazonaws.com", "googleapis.com", "cloud.google.com", "aliyuncs.com", "myqcloud.com", "windows.net", "cloudflarestorage.com", "digitaloceanspaces.com", "myhuaweicloud.com", "volces.com"], ..Spec::DEFAULT },
    // more.go:162 (BUCKET)
    Spec { category: Category::Bucket, pattern: r#"(?:^|[^A-Za-z0-9.-])(?:s3(?:[.-](?:dualstack\.)?[a-z0-9-]+)?\.amazonaws\.com(?:\.cn)?|storage\.googleapis\.com|storage\.cloud\.google\.com|[a-z0-9]{3,24}\.blob\.core\.windows\.net|oss-[a-z0-9-]+\.aliyuncs\.com|[a-z0-9-]+\.digitaloceanspaces\.com)/([A-Za-z0-9](?:[A-Za-z0-9._-]{0,61}[A-Za-z0-9])?)"#, capture: Capture::First, check: Guard::BucketOK, fold: &["amazonaws.com", "googleapis.com", "cloud.google.com", "aliyuncs.com", "myqcloud.com", "windows.net", "cloudflarestorage.com", "digitaloceanspaces.com", "myhuaweicloud.com", "volces.com"], ..Spec::DEFAULT },
    // crypto.go:50 (WALLET_KEY)
    Spec { category: Category::Secrets, pattern: r#"(?i)0x[0-9a-f]{56,64}|[0-9a-f]{64}"#, capture: Capture::First, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789", context: Guard::WalletKeyAt, fold: &["priv", "secret", "signer", "signing", "deployer", "wallet", "pk", "私钥", "私鑰", "秘密鍵", "개인키", "비밀키"], id_rule: IdRule::Credential, ..Spec::DEFAULT },
    // crypto.go:55 (WIF_KEY)
    Spec { category: Category::Secrets, pattern: r#"[1-9A-HJ-NP-Za-km-z]{51,}"#, capture: Capture::First, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789", check: Guard::WifKey, ..Spec::DEFAULT },
    // crypto.go:61 (BIP32_KEY)
    Spec { category: Category::Secrets, pattern: r#"[xyztuvYZUV]prv[1-9A-HJ-NP-Za-km-z]{107}"#, capture: Capture::First, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789", check: Guard::Bip32Key, markers: &["prv"], ..Spec::DEFAULT },
    // crypto.go:66 (SOLANA_KEY)
    Spec { category: Category::Secrets, pattern: r#"[1-9A-HJ-NP-Za-km-z]{85,}"#, capture: Capture::First, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789", check: Guard::SolanaKey, ..Spec::DEFAULT },
    // crypto.go:71 (SOLANA_KEYPAIR)
    Spec { category: Category::Secrets, pattern: r#"\[[ \t\n\r\x0C]*[0-9]{1,3}(?:[ \t\n\r\x0C]*,[ \t\n\r\x0C]*[0-9]{1,3}){63}[ \t\n\r\x0C]*\]"#, capture: Capture::First, check: Guard::SolanaKeypair, markers: &["["], ..Spec::DEFAULT },
    // crypto.go:76 (NEAR_KEY)
    Spec { category: Category::Secrets, pattern: r#"ed25519:[1-9A-HJ-NP-Za-km-z]{86,88}"#, capture: Capture::First, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789", markers: &["ed25519:"], ..Spec::DEFAULT },
    // crypto.go:80 (SUI_KEY)
    Spec { category: Category::Secrets, pattern: r#"suiprivkey1[0-9a-z]{58}"#, capture: Capture::First, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789", markers: &["suiprivkey1"], ..Spec::DEFAULT },
    // crypto.go:84 (APTOS_KEY)
    Spec { category: Category::Secrets, pattern: r#"(?:ed25519|secp256k1)-priv-0x[0-9a-fA-F]{64}"#, capture: Capture::First, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789", markers: &["-priv-0x"], ..Spec::DEFAULT },
    // crypto.go:89 (STELLAR_KEY)
    Spec { category: Category::Secrets, pattern: r#"S[A-Z2-7]{55}"#, capture: Capture::First, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789", check: Guard::StellarSeed, ..Spec::DEFAULT },
    // crypto.go:93 (NOSTR_KEY)
    Spec { category: Category::Secrets, pattern: r#"nsec1[02-9ac-hj-np-z]{58}"#, capture: Capture::First, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789", markers: &["nsec1"], ..Spec::DEFAULT },
    // crypto.go:97 (AGE_KEY)
    Spec { category: Category::Secrets, pattern: r#"AGE-SECRET-KEY-1[0-9A-Z]{58}"#, capture: Capture::First, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789", markers: &["AGE-SECRET-KEY-"], ..Spec::DEFAULT },
    // crypto.go:101 (ZCASH_KEY)
    Spec { category: Category::Secrets, pattern: r#"secret-extended-key-(?:main|test)1[0-9a-z]{150,320}"#, capture: Capture::First, bound: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789", markers: &["secret-extended-key-"], ..Spec::DEFAULT },
    // crypto.go:107 (SEED_PHRASE)
    Spec { category: Category::Secrets, pattern: r#"(?i)(?:seed|mnemonic|phrase|recovery|助记词|助記詞|シード|ニーモニック|시드|니모닉)[^.\n]{0,24}?(?:\n[ \t]*)?(?-u:\b)([a-z]{3,8}(?:[ \t]+[a-z]{3,8}){11,40})(?-u:\b)"#, capture: Capture::First, repair: Guard::SeedPhraseAt, fold: &["seed", "mnemonic", "phrase", "recovery", "助记词", "助記詞", "シード", "ニーモニック", "시드", "니모닉"], ..Spec::DEFAULT },
];
