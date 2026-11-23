//! 原始账号JSON只改固定记忆键。原/新明文及兼容解码均写入外持有槽，Db锁内不丢整行。
//! Unknown保留原5字段；Official同schema严格11字段，execution仍unknown，无领取/金额/发送结论。

use rusqlite::{types::ValueRef, Connection};
use serde_json::{json, Map, Value};

use super::{
    envelope::Envelope, retained_json, RowStamp, state_error, storage, Error, StateError, StateFailure,
    StorageFailure, MAX_ENTRY_BYTES, MAX_STATE_BYTES, MAX_STATE_ENTRIES, MAX_VALUE_BYTES, SCHEMA, STATE_KEY,
};
use crate::server::core::{beijing, plugin_host::wasm::OfficialDailyFact};

#[derive(PartialEq, Eq)]
pub(super) struct Memory {
    pub(super) receipt_id: String,
    pub(super) snapshot_at: i64,
    pub(super) day: String,
    pub(super) official: Option<OfficialTask>,
}

#[derive(PartialEq, Eq)]
pub(super) struct OfficialTask {
    pub(super) discovery_group: String,
    pub(super) scene: String,
    pub(super) item_code: String,
    pub(super) selected_day: String,
    pub(super) selected_at: i64,
    pub(super) observed_at: i64,
}

impl Memory {
    pub(super) fn reserve() -> Result<Self, StateError> {
        let snapshot_at = crate::server::logging::now_ms();
        let day = date(snapshot_at)?;
        Ok(Self { receipt_id: storage::random_id()?, snapshot_at, day, official: None })
    }

    pub(super) fn local_copy(&self) -> Self {
        Self { receipt_id: self.receipt_id.clone(), snapshot_at: self.snapshot_at, day: self.day.clone(), official: None }
    }

    /// 参数只能是真实typed fact，不能用view/JSON/布尔补造肯定材料。
    pub(super) fn from_official(reserved: &Self, fact: &OfficialDailyFact) -> Result<Self, StateError> {
        let view = fact.view();
        for text in [view.discovery_group, view.scene, view.item_code] { identifier(text)?; }
        if reserved.official.is_some() || reserved.snapshot_at > view.selected_at
            || reserved.day != view.selected_day || date(view.selected_at)? != view.selected_day
            || view.observed_at < view.selected_at || date(view.observed_at)? != view.selected_day {
            return Err(state_error(StateFailure::InvalidClock));
        }
        Ok(Self {
            receipt_id: reserved.receipt_id.clone(), snapshot_at: reserved.snapshot_at, day: reserved.day.clone(),
            official: Some(OfficialTask {
                discovery_group: view.discovery_group.to_string(), scene: view.scene.to_string(),
                item_code: view.item_code.to_string(), selected_day: view.selected_day.to_string(),
                selected_at: view.selected_at, observed_at: view.observed_at,
            }),
        })
    }

    /// 先完整借用验证后才分配固定小对象；调用者立即放外持有槽，不在COMMIT失败时丢弃。
    pub(super) fn read(value: &Value) -> Result<Self, StateError> {
        let fields = validated(value)?;
        let receipt_id = text(fields, "receiptId")?;
        let snapshot_at = integer(fields, "snapshotAt")?;
        let day = text(fields, "day")?;
        let official = if fields["state"] == "official_confirmed" {
            Some(OfficialTask {
                discovery_group: text(fields, "discovery_group")?.to_string(),
                scene: text(fields, "scene")?.to_string(), item_code: text(fields, "item_code")?.to_string(),
                selected_day: text(fields, "selected_day")?.to_string(),
                selected_at: integer(fields, "selected_at")?, observed_at: integer(fields, "observed_at")?,
            })
        } else { None };
        Ok(Self { receipt_id: receipt_id.to_string(), snapshot_at, day: day.to_string(), official })
    }

    pub(super) fn value(&self) -> Value {
        match &self.official {
            None => json!({ "state": "unknown", "execution": "unknown", "receiptId": self.receipt_id,
                "snapshotAt": self.snapshot_at, "day": self.day }),
            Some(task) => json!({ "state": "official_confirmed", "execution": "unknown", "receiptId": self.receipt_id,
                "snapshotAt": self.snapshot_at, "day": self.day, "discovery_group": task.discovery_group,
                "scene": task.scene, "item_code": task.item_code, "selected_day": task.selected_day,
                "selected_at": task.selected_at, "observed_at": task.observed_at }),
        }
    }

    pub(super) fn matches_official(&self, fact: &OfficialDailyFact) -> bool {
        let Some(task) = &self.official else { return false; };
        let view = fact.view();
        task.discovery_group == view.discovery_group && task.scene == view.scene && task.item_code == view.item_code
            && task.selected_day == view.selected_day && task.selected_at == view.selected_at && task.observed_at == view.observed_at
    }
}

fn validated(value: &Value) -> Result<&Map<String, Value>, StateError> {
    retained_json::check(value, MAX_VALUE_BYTES)?;
    let fields = value.as_object().ok_or_else(|| state_error(StateFailure::InvalidEntry))?;
    let official = match fields.get("state").and_then(Value::as_str) {
        Some("unknown") => false,
        Some("official_confirmed") => true,
        _ => return Err(state_error(StateFailure::InvalidEntry)),
    };
    let expected_fields = if official { 11 } else { 5 };
    if fields.len() != expected_fields || text(fields, "execution")? != "unknown" {
        return Err(state_error(StateFailure::InvalidEntry));
    }
    let receipt = text(fields, "receiptId")?;
    if receipt.len() != 64 || !receipt.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || !receipt.bytes().any(|byte| byte != b'0') {
        return Err(state_error(StateFailure::InvalidEntry));
    }
    let at = integer(fields, "snapshotAt")?;
    let day = text(fields, "day")?;
    if date(at)? != day { return Err(state_error(StateFailure::InvalidEntry)); }
    if official {
        for name in ["discovery_group", "scene", "item_code"] { identifier(text(fields, name)?)?; }
        let selected_day = text(fields, "selected_day")?;
        let selected_at = integer(fields, "selected_at")?;
        let observed_at = integer(fields, "observed_at")?;
        if at > selected_at || selected_at > observed_at || day != selected_day
            || date(selected_at)? != selected_day || date(observed_at)? != selected_day {
            return Err(state_error(StateFailure::InvalidClock));
        }
    }
    Ok(fields)
}

fn text<'a>(fields: &'a Map<String, Value>, name: &str) -> Result<&'a str, StateError> {
    fields.get(name).and_then(Value::as_str).ok_or_else(|| state_error(StateFailure::InvalidEntry))
}
fn integer(fields: &Map<String, Value>, name: &str) -> Result<i64, StateError> {
    fields.get(name).and_then(Value::as_i64).ok_or_else(|| state_error(StateFailure::InvalidEntry))
}
fn identifier(text: &str) -> Result<(), StateError> {
    if text.is_empty() || text.len() > 256 || text.trim() != text || text.contains("://")
        || text.chars().any(|character| character.is_control() || character.is_whitespace()) {
        return Err(state_error(StateFailure::InvalidEntry));
    }
    Ok(())
}
fn date(at: i64) -> Result<String, StateError> {
    if at <= 0 || at > 253_402_300_799_999 { return Err(state_error(StateFailure::InvalidClock)); }
    beijing::date_key_of_ms(at).filter(|day| day.len() == 10).ok_or_else(|| state_error(StateFailure::InvalidClock))
}

pub(super) struct FixedState {
    pub(super) present: bool,
    pub(super) schema: u32,
    pub(super) revision: u64,
    pub(super) memory: Option<Memory>,
}

fn container(raw: &Value) -> Result<Option<&Map<String, Value>>, StateError> {
    let root = raw.as_object().ok_or(Error::InvalidRecord)?;
    match root.get("pluginState") {
        None => Ok(None),
        Some(value @ Value::Object(fields)) => {
            if fields.len() > MAX_STATE_ENTRIES { return Err(Error::LimitExceeded.into()); }
            retained_json::check(value, MAX_STATE_BYTES)?;
            for (key, entry) in fields {
                if key.is_empty() || key.len() > 160 || key.chars().any(char::is_control) {
                    return Err(state_error(StateFailure::InvalidContainer));
                }
                entry_header(entry)?;
            }
            Ok(Some(fields))
        }
        Some(_) => Err(state_error(StateFailure::InvalidContainer)),
    }
}

pub(super) fn entry_header(value: &Value) -> Result<(u64, u32), StateError> {
    let fields = value.as_object().ok_or_else(|| state_error(StateFailure::InvalidEntry))?;
    if fields.len() != 3 || !fields.contains_key("value") { return Err(state_error(StateFailure::InvalidEntry)); }
    let revision = fields.get("revision").and_then(Value::as_u64).filter(|revision| *revision > 0)
        .ok_or_else(|| state_error(StateFailure::InvalidEntry))?;
    let schema = fields.get("schema").and_then(Value::as_u64).and_then(|schema| u32::try_from(schema).ok())
        .filter(|schema| *schema > 0).ok_or_else(|| state_error(StateFailure::InvalidEntry))?;
    retained_json::check(&fields["value"], MAX_ENTRY_BYTES)?;
    Ok((revision, schema))
}

pub(super) fn fixed_state(raw: &Value) -> Result<FixedState, StateError> {
    let Some(entry) = container(raw)?.and_then(|fields| fields.get(STATE_KEY)) else {
        return Ok(FixedState { present: false, schema: SCHEMA, revision: 0, memory: None });
    };
    let (revision, schema) = entry_header(entry)?;
    if schema != SCHEMA { return Err(state_error(StateFailure::UnsupportedSchema)); }
    let memory = Memory::read(&entry["value"])?;
    Ok(FixedState { present: true, schema, revision, memory: Some(memory) })
}

/// entry在锁外准备，旧条目移入外持有displaced；不在锁内drop被替换的固定记忆。
pub(super) fn put_fixed(
    raw: &mut Value, entry: &mut Option<Value>, displaced: &mut Option<Value>, displaced_container: &mut Option<Value>,
) -> Result<(), StateError> {
    if displaced.is_some() || displaced_container.is_some() { return Err(state_error(StateFailure::InvalidEntry)); }
    let value = entry.as_ref().ok_or_else(|| state_error(StateFailure::InvalidEntry))?;
    let (_, schema) = entry_header(value)?;
    if schema != SCHEMA { return Err(state_error(StateFailure::UnsupportedSchema)); }
    validated(&value["value"])?;
    container(raw)?;
    let root = raw.as_object_mut().ok_or(Error::InvalidRecord)?;
    if !root.contains_key("pluginState") {
        *displaced_container = root.insert("pluginState".into(), Value::Object(Map::new()));
    }
    let fields = root.get_mut("pluginState").and_then(Value::as_object_mut)
        .ok_or_else(|| state_error(StateFailure::InvalidContainer))?;
    if !fields.contains_key(STATE_KEY) && fields.len() >= MAX_STATE_ENTRIES { return Err(Error::LimitExceeded.into()); }
    let value = entry.take().ok_or_else(|| state_error(StateFailure::InvalidEntry))?;
    *displaced = fields.insert(STATE_KEY.to_string(), value);
    container(raw)?;
    Ok(())
}

/// 仅撤回本轮暂存的待确认条目；被移出的旧条目与空容器均移至外持有槽锁外释放。
pub(super) fn clear_fixed(
    raw: &mut Value, displaced: &mut Option<Value>, displaced_container: &mut Option<Value>,
) -> Result<(), StateError> {
    if displaced.is_some() || displaced_container.is_some() { return Err(state_error(StateFailure::InvalidEntry)); }
    container(raw)?;
    let root = raw.as_object_mut().ok_or(Error::InvalidRecord)?;
    let Some(fields) = root.get_mut("pluginState").and_then(Value::as_object_mut) else {
        return Err(state_error(StateFailure::StateChanged));
    };
    *displaced = fields.remove(STATE_KEY);
    if displaced.is_none() { return Err(state_error(StateFailure::StateChanged)); }
    if fields.is_empty() { *displaced_container = root.remove("pluginState"); }
    container(raw)?;
    Ok(())
}

#[derive(Default)]
pub(super) struct RawSlot {
    pub(super) value: Value,
    envelope: Envelope,
    pending_key: Option<String>,
    displaced_key: Option<String>,
    decoder: Option<retained_json::Decoder>,
    plugin: Value,
    plugin_decoder: Option<retained_json::Decoder>,
    public: Value,
    public_decoder: Option<retained_json::Decoder>,
}

impl RawSlot {
    pub(super) fn verify(&self, conn: &Connection, stamp: &RowStamp) -> Result<(), StateError> {
        let plugin = match self.value.get("pluginSecret") {
            Some(Value::String(_)) => Some(&self.plugin), other => other,
        };
        let public = match self.value.get("pluginPublic") {
            Some(Value::String(_)) => Some(&self.public), other => other,
        };
        retained_json::check_loaded(&self.value, plugin, public)?;
        stamp.verify_doubao_loaded(conn, &self.value, plugin, public).map_err(StateError::Account)
    }
}

/// 只读真实原行，部分明文/JSON/兼容解码在错误时也留在调用方锁外所有者中。
pub(super) fn load_raw_into(
    conn: &Connection, key: &[u8; 32], stamp: &RowStamp, slot: &mut RawSlot,
) -> Result<(), StateError> {
    storage::ensure_no_triggers(conn)?;
    let mut statement = conn.prepare(
        "SELECT CASE WHEN typeof(data)='text' AND octet_length(data)<=?2 THEN data END,
            octet_length(data), typeof(data), id, provider
         FROM main.accounts WHERE id=?1 LIMIT 1"
    ).map_err(|error| storage::sql_error(error, StorageFailure::Read))?;
    let result: Result<(), StateError> = (|| {
        let mut rows = statement.query(rusqlite::params![stamp.account(), super::bounded::MAX_ROW_BYTES as i64])
            .map_err(|error| storage::sql_error(error, StorageFailure::Read))?;
        let row = rows.next().map_err(|error| storage::sql_error(error, StorageFailure::Read))?
            .ok_or(Error::Conflict(super::super::Conflict::MissingAccount))?;
        let text = |index: usize, limit: usize| -> Result<&str, StateError> {
            let ValueRef::Text(bytes) = row.get_ref(index).map_err(|error| storage::sql_error(error, StorageFailure::Read))?
                else { return Err(Error::InvalidRecord.into()); };
            if bytes.len() > limit { return Err(Error::LimitExceeded.into()); }
            std::str::from_utf8(bytes).map_err(|_| Error::InvalidRecord.into())
        };
        if text(2, 16)? != "text" || text(3, 512)? != stamp.account() || text(4, 128)? != stamp.provider().id() {
            return Err(Error::InvalidRecord.into());
        }
        let ValueRef::Integer(size) = row.get_ref(1).map_err(|error| storage::sql_error(error, StorageFailure::Read))?
            else { return Err(Error::InvalidRecord.into()); };
        if size <= 0 { return Err(Error::InvalidRecord.into()); }
        if size > super::bounded::MAX_ROW_BYTES as i64 { return Err(Error::LimitExceeded.into()); }
        let encoded = text(0, super::bounded::MAX_ROW_BYTES)?;
        super::envelope::open_into(encoded, key, &mut slot.envelope)?;
        retained_json::parse_into(&slot.envelope.plaintext, &mut slot.value,
            &mut slot.pending_key, &mut slot.displaced_key, &mut slot.decoder, super::bounded::MAX_PLAIN_BYTES)?;
        if !slot.value.is_object() || slot.value["id"].as_str() != Some(stamp.account())
            || slot.value["provider"].as_str() != Some(stamp.provider().id()) { return Err(Error::InvalidRecord.into()); }
        for (name, decoded, decoder, maximum) in [
            ("pluginSecret", &mut slot.plugin, &mut slot.plugin_decoder, super::bounded::MAX_SECRET_BYTES),
            ("pluginPublic", &mut slot.public, &mut slot.public_decoder, super::bounded::MAX_PUBLIC_BYTES),
        ] {
            if let Some(Value::String(raw)) = slot.value.get(name) {
                retained_json::parse_into(raw.as_bytes(), decoded, &mut slot.pending_key,
                    &mut slot.displaced_key, decoder, maximum)?;
                if !decoded.is_object() { return Err(Error::InvalidRecord.into()); }
            }
        }
        if rows.next().map_err(|error| storage::sql_error(error, StorageFailure::Read))?.is_some() {
            return Err(Error::InvalidRecord.into());
        }
        Ok(())
    })();
    let finalized = statement.finalize().map_err(|error| storage::sql_error(error, StorageFailure::Read));
    result?;
    finalized?;
    Ok(())
}

/// 只有固定原账号UPDATE，无INSERT/版本推进；输出明文及信封缓冲均由外层保留。
pub(super) fn update_raw(
    conn: &Connection, key: &[u8; 32], stamp: &RowStamp, raw: &Value, output: &mut Envelope,
) -> Result<(), StateError> {
    if conn.is_autocommit() { return Err(Error::Storage(StorageFailure::Begin).into()); }
    if raw["id"].as_str() != Some(stamp.account()) || raw["provider"].as_str() != Some(stamp.provider().id()) {
        return Err(Error::InvalidRecord.into());
    }
    storage::ensure_no_triggers(conn)?;
    retained_json::encode_into(raw, &mut output.plaintext, super::bounded::MAX_PLAIN_BYTES)?;
    super::envelope::seal_into(key, output)?;
    if output.encoded.len() > super::bounded::MAX_ROW_BYTES { return Err(Error::LimitExceeded.into()); }
    let mut statement = conn.prepare("UPDATE main.accounts SET data=?3 WHERE id=?1 AND provider=?2")
        .map_err(|error| storage::sql_error(error, StorageFailure::Write))?;
    let changed = statement.execute(rusqlite::params![stamp.account(), stamp.provider().id(), output.encoded.as_str()])
        .map_err(|error| storage::sql_error(error, StorageFailure::Write));
    let finalized = statement.finalize().map_err(|error| storage::sql_error(error, StorageFailure::Write));
    if changed? != 1 { return Err(Error::Conflict(super::super::Conflict::MissingAccount).into()); }
    finalized?;
    Ok(())
}
