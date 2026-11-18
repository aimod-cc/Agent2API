use std::sync::{Arc, OnceLock};
use std::time::Duration;

use agent2api_provider_protocol as proto;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::{sync::Semaphore, time::Instant};
use tokio_util::sync::CancellationToken;

use super::context::Context;

const MAX_HASHES: usize = 8_000_000;

struct Work {
    cancel: CancellationToken,
}
impl Drop for Work {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

pub(super) async fn solve(context: &Context, params: &Value) -> Result<Value, proto::WireError> {
    context.permission("crypto.monkeycode-cap.v1")?;
    if !matches!(context.shared.options.provider.as_str(), "monkeycode" | "monkeycode-intl")
        || params.as_object().is_none_or(|fields| fields.len() != 1 || !fields.contains_key("challenge"))
    {
        return Err(invalid());
    }
    let call = context.checkin_call.as_ref().ok_or_else(invalid)?;
    call.observe_context(context)?;
    let challenge = &params["challenge"];
    let count = challenge["challenge"]["c"].as_u64().filter(|value| (1..=100).contains(value)).ok_or_else(invalid)?;
    let size = challenge["challenge"]["s"].as_u64().filter(|value| (1..=64).contains(value)).ok_or_else(invalid)? as usize;
    let difficulty = challenge["challenge"]["d"].as_u64().filter(|value| (1..=4).contains(value)).ok_or_else(invalid)? as usize;
    let token = challenge["token"].as_str().filter(|value| !value.is_empty() && value.len() <= 256
        && value.bytes().all(|byte| byte.is_ascii_hexdigit())).ok_or_else(invalid)?.to_string();
    let now = crate::server::logging::now_ms();
    let lifetime = challenge["expires"].as_i64().and_then(|expires| expires.checked_sub(now))
        .filter(|lifetime| (1..=600_000).contains(lifetime)).ok_or_else(invalid)?;
    let deadline = context.call_deadline.min(Instant::now() + Duration::from_millis(lifetime as u64))
        .min(Instant::now() + Duration::from_secs(60));
    static SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    let slot = SLOTS.get_or_init(|| Arc::new(Semaphore::new(2))).clone()
        .try_acquire_owned().map_err(|_| invalid())?;
    let work = Work { cancel: context.shared.cancel.child_token() };
    let cancel = work.cancel.clone();
    let task = crate::server::core::blocking::run(move || {
        let _slot = slot;
        calculate(&token, count, size, difficulty, deadline, &cancel)
    });
    let solutions = tokio::select! {
        biased;
        _ = context.shared.cancel.cancelled() => return Err(invalid()),
        result = tokio::time::timeout_at(deadline, task) => result.map_err(|_| invalid())?.map_err(|_| invalid())??,
    };
    call.observe_context(context)?;
    Ok(json!({ "solutions": solutions }))
}

fn calculate(token: &str, count: u64, size: usize, difficulty: usize, deadline: Instant,
    cancel: &CancellationToken) -> Result<Vec<u64>, proto::WireError> {
    let mut solutions = Vec::new();
    let mut hashes = 0usize;
    for index in 1..=count {
        let seed = format!("{token}{index}");
        let salt = prng(&seed, size);
        let target = prng(&format!("{seed}d"), difficulty).bytes().map(|byte| match byte {
            b'0'..=b'9' => byte - b'0',
            _ => byte - b'a' + 10,
        }).collect::<Vec<_>>();
        let mut prefix = Sha256::new();
        prefix.update(salt.as_bytes());
        let mut solution = None;
        for nonce in 0..4_000_000u64 {
            hashes += 1;
            if hashes > MAX_HASHES || hashes % 4096 == 1 && (cancel.is_cancelled() || Instant::now() >= deadline) {
                return Err(invalid());
            }
            let mut hash = prefix.clone();
            hash.update(nonce.to_string().as_bytes());
            let digest = hash.finalize();
            if target.iter().enumerate().all(|(index, nibble)| {
                let actual = if index % 2 == 0 { digest[index / 2] >> 4 } else { digest[index / 2] & 15 };
                actual == *nibble
            }) {
                solution = Some(nonce);
                break;
            }
        }
        solutions.push(solution.ok_or_else(invalid)?);
    }
    if cancel.is_cancelled() || Instant::now() >= deadline {
        return Err(invalid());
    }
    Ok(solutions)
}

fn prng(seed: &str, size: usize) -> String {
    let mut state = 2_166_136_261u32;
    for byte in seed.bytes() {
        state = (state ^ byte as u32).wrapping_mul(16_777_619);
    }
    let mut output = String::new();
    while output.len() < size {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        output.push_str(&format!("{state:08x}"));
    }
    output.truncate(size);
    output
}

fn invalid() -> proto::WireError {
    proto::WireError::with_code("MonkeyCode 官方计算验证无效、过期或超过本次预算，请稍后重试；没有提交签到", "monkeycode_cap_unavailable")
}
