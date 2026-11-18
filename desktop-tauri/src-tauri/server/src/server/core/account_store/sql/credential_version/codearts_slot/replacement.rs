//! 已消费 Pending 的完整新凭据候选。只校验保存契约，不证明上游成功或首次授权。
//! 不保留旧刷新令牌作兜底，不接受私钥、oauth_context、任意公开字段或整条账号覆盖。

use serde_json::{json, Map, Value};

use super::super::{bounded, projection::Change, Error, StoredAccount};
use super::SlotFailure;

const ALIASES: &[&[&str]] = &[
    &["access_key_id", "accessKeyId", "accessToken"],
    &["secret_access_key", "secretAccessKey"],
    &["security_token", "securityToken"],
    &["refresh_token", "refreshToken"],
    &["expiration", "expires_at"],
];

pub(super) struct Replacement {
    change: Change,
}

impl Replacement {
    pub(super) fn new(credentials: &Value, now: i64) -> Result<Self, SlotFailure> {
        bounded::check(credentials, bounded::MAX_SECRET_BYTES)?;
        let fields = credentials
            .as_object()
            .ok_or(SlotFailure::IncompleteReplacement)?;
        if now <= 0
            || fields.keys().any(|key| {
                key != "expiresAt"
                    && !ALIASES
                        .iter()
                        .any(|aliases| aliases.contains(&key.as_str()))
            })
        {
            return Err(SlotFailure::IncompleteReplacement);
        }
        let access = required(fields, ALIASES[0], 32 * 1024)?;
        let secret = required(fields, ALIASES[1], 32 * 1024)?;
        let security = required(fields, ALIASES[2], 32 * 1024)?;
        let refresh = required(fields, ALIASES[3], 32 * 1024)?;
        let expiration = required(fields, ALIASES[4], 1024)?;
        if [access, security]
            .iter()
            .any(|text| text.bytes().any(|byte| !(0x21..=0x7e).contains(&byte)))
        {
            return Err(SlotFailure::IncompleteReplacement);
        }
        let expires = chrono::DateTime::parse_from_rfc3339(expiration)
            .map_err(|_| SlotFailure::IncompleteReplacement)?
            .timestamp_millis();
        if expires <= now
            || fields
                .get("expiresAt")
                .is_some_and(|value| value.as_i64() != Some(expires))
        {
            return Err(SlotFailure::IncompleteReplacement);
        }
        // 每一项均从本次响应取得；没有上游强制轮换证据，不要求字符串一定不同旧值。
        let normalized = json!({
            "access_key_id": access,
            "secret_access_key": secret,
            "security_token": security,
            "refresh_token": refresh,
            "expires_at": expiration,
            "expiresAt": expires,
        });
        let change = Change::new(&normalized, &json!({ "expiresAt": expires }))?;
        Ok(Self { change })
    }

    pub(super) fn apply(self, record: &mut StoredAccount) -> Result<(), Error> {
        self.change.apply(record)
    }
}

fn required<'a>(
    fields: &'a Map<String, Value>,
    aliases: &[&str],
    maximum: usize,
) -> Result<&'a str, SlotFailure> {
    let mut found = None;
    for name in aliases {
        if let Some(value) = fields.get(*name) {
            let text = value.as_str().ok_or(SlotFailure::IncompleteReplacement)?;
            // 在 trim 之前拒绝原文控制字符和越界；不把空白/坏别名吞成“缺字段”。
            if text.is_empty()
                || text.len() > maximum
                || text.chars().any(char::is_control)
                || text.trim() != text
                || found.is_some_and(|previous| previous != text)
            {
                return Err(SlotFailure::IncompleteReplacement);
            }
            found = Some(text);
        }
    }
    found.ok_or(SlotFailure::IncompleteReplacement)
}
