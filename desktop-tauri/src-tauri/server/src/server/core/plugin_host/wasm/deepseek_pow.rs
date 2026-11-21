//! DeepSeek 网页版 DeepSeekHashV1 工作量证明的受控宿主计算。
//! 仅限 deepseek-web 在真实对话调用中使用，避免纯 Wasm 循环耗尽指令燃料。

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use agent2api_provider_protocol as proto;
use base64::Engine;
use serde_json::{json, Value};
use tokio::{sync::Semaphore, time::Instant};
use tokio_util::sync::CancellationToken;

use super::context::Context;

const MAX_DIFFICULTY: u64 = 2_000_000;
const CHECK_INTERVAL: u64 = 1024;
const MAX_SECONDS: u64 = 20;
const MAX_CONCURRENT_SOLVERS: usize = 4;

struct Challenge {
    path: String,
    target: String,
    expected: [u8; 32],
    difficulty: u64,
    salt: String,
    signature: String,
    expiry: i64,
}

struct Work {
    cancel: CancellationToken,
}

impl Drop for Work {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

fn http_error(status: u16, code: Option<&str>, message: impl Into<String>) -> proto::WireError {
    proto::WireError::with_code(
        message,
        format!("provider_http:{}", json!({ "status": status, "code": code })),
    )
}

fn check_abort(cancel: &CancellationToken, deadline: Instant) -> Result<(), proto::WireError> {
    if cancel.is_cancelled() {
        return Err(proto::WireError::with_code(
            "DeepSeek 网页验证计算已取消",
            proto::WireError::ABORTED,
        ));
    }
    if Instant::now() >= deadline {
        return Err(http_error(
            504,
            Some("deepseek_pow_timeout"),
            "DeepSeek 网页验证计算超时，未提交对应请求",
        ));
    }
    Ok(())
}

fn parse_challenge(params: &Value) -> Result<Challenge, proto::WireError> {
    let fields = params
        .as_object()
        .filter(|fields| {
            fields.len() == 2 && fields.contains_key("challenge") && fields.contains_key("path")
        })
        .ok_or_else(|| http_error(400, Some("bad_params"), "DeepSeek 验证参数无效"))?;
    let path = fields["path"]
        .as_str()
        .filter(|value| matches!(*value, "/api/v0/chat/completion" | "/api/v0/file/upload_file"))
        .ok_or_else(|| http_error(400, None, "DeepSeek 验证目标路径无效"))?;
    let challenge = &fields["challenge"];
    if !challenge.is_object() {
        return Err(http_error(502, None, "DeepSeek 没有返回有效的验证挑战"));
    }
    if challenge["algorithm"].as_str() != Some("DeepSeekHashV1") {
        return Err(http_error(502, None, "DeepSeek 网页验证算法已变化，请更新插件"));
    }
    let target = challenge["challenge"]
        .as_str()
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| http_error(502, None, "DeepSeek 验证摘要无效"))?;
    let mut expected = [0u8; 32];
    for (index, byte) in expected.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&target[index * 2..index * 2 + 2], 16)
            .map_err(|_| http_error(502, None, "DeepSeek 验证摘要无效"))?;
    }
    let difficulty = challenge["difficulty"]
        .as_u64()
        .filter(|value| *value > 0 && *value <= MAX_DIFFICULTY)
        .ok_or_else(|| http_error(502, None, "DeepSeek 验证难度过高或无效"))?;
    let salt = challenge["salt"]
        .as_str()
        .filter(|value| {
            !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
        })
        .ok_or_else(|| http_error(502, None, "DeepSeek 验证盐无效"))?;
    let signature = challenge["signature"]
        .as_str()
        .filter(|value| {
            !value.is_empty() && value.len() <= 8192 && !value.chars().any(char::is_control)
        })
        .ok_or_else(|| http_error(502, None, "DeepSeek 验证签名缺失或无效"))?;
    if challenge
        .get("target_path")
        .is_some_and(|value| value.as_str() != Some(path))
    {
        return Err(http_error(502, None, "DeepSeek 验证挑战不属于本次目标路径"));
    }
    let expiry = challenge["expire_at"]
        .as_i64()
        .filter(|value| *value > 0)
        .ok_or_else(|| http_error(502, None, "DeepSeek 验证缺少有效期"))?;
    Ok(Challenge {
        path: path.to_string(),
        target: target.to_string(),
        expected,
        difficulty,
        salt: salt.to_string(),
        signature: signature.to_string(),
        expiry,
    })
}

/// 在受控阻塞线程池中执行 DeepSeekHashV1 挑战计算并返回 Base64 证明。
pub(super) async fn solve(context: &Context, params: &Value) -> Result<Value, proto::WireError> {
    context.permission("crypto.deepseek-pow.v1")?;
    if context.shared.options.provider != "deepseek-web" {
        return Err(proto::WireError::with_code(
            "DeepSeek 网页验证计算能力不能跨提供商使用",
            "forbidden",
        ));
    }
    let call = context.conversation.as_ref().ok_or_else(|| {
        proto::WireError::with_code("DeepSeek 网页验证计算只能在真实对话调用中执行", "forbidden")
    })?;
    call.validate_context(&context.shared, &context.account)?;

    let challenge = parse_challenge(params)?;
    let deadline = context
        .call_deadline
        .min(Instant::now() + Duration::from_secs(MAX_SECONDS));
    let call_ended = call.ended_token();
    let request_cancel = context
        .sink
        .as_ref()
        .map(|sink| sink.cancel.clone())
        .unwrap_or_else(|| context.shared.cancel.child_token());

    static SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    let slots = SLOTS
        .get_or_init(|| Arc::new(Semaphore::new(MAX_CONCURRENT_SOLVERS)))
        .clone();
    let work = Work {
        cancel: context.shared.cancel.child_token(),
    };
    let cancel = work.cancel.clone();
    let run = async move {
        let permit = slots.acquire_owned().await.map_err(|_| {
            http_error(
                503,
                Some("deepseek_pow_unavailable"),
                "DeepSeek 网页验证计算队列已关闭",
            )
        })?;
        crate::server::core::blocking::run(move || {
            let _permit = permit;
            calculate(&challenge, deadline, &cancel)
        })
        .await
        .map_err(|_| {
            http_error(
                502,
                Some("deepseek_pow_unavailable"),
                "DeepSeek 网页验证计算任务已中断",
            )
        })?
    };
    let encoded = tokio::select! {
        biased;
        _ = context.shared.cancel.cancelled() => {
            return Err(proto::WireError::with_code("Wasm 插件已关闭", "plugin_unavailable"));
        }
        _ = call_ended.cancelled() => {
            return Err(proto::WireError::with_code("Wasm 对话调用已结束", proto::WireError::ABORTED));
        }
        _ = request_cancel.cancelled() => {
            return Err(proto::WireError::with_code("DeepSeek 网页验证计算已取消", proto::WireError::ABORTED));
        }
        result = tokio::time::timeout_at(deadline, run) => match result {
            Ok(solved) => solved?,
            Err(_) => return Err(http_error(504, Some("deepseek_pow_timeout"), "DeepSeek 网页验证计算超时，未提交对应请求")),
        }
    };
    drop(work);
    call.validate_context(&context.shared, &context.account)?;
    Ok(json!({ "response": encoded }))
}

fn calculate(
    challenge: &Challenge,
    deadline: Instant,
    cancel: &CancellationToken,
) -> Result<String, proto::WireError> {
    // expire_at 逐字用于挑战前缀；不猜单位，不把它改写成另一时间戳。
    let prefix = format!("{}_{}_", challenge.salt, challenge.expiry);
    let mut candidate = Vec::with_capacity(prefix.len() + 20);
    candidate.extend_from_slice(prefix.as_bytes());
    for answer in 0..=challenge.difficulty {
        if answer % CHECK_INTERVAL == 0 {
            check_abort(cancel, deadline)?;
        }
        candidate.truncate(prefix.len());
        decimal(answer, &mut candidate);
        if hash(&candidate) == challenge.expected {
            check_abort(cancel, deadline)?;
            let value = json!({
                "algorithm": "DeepSeekHashV1",
                "challenge": challenge.target,
                "salt": challenge.salt,
                "answer": answer,
                "signature": challenge.signature,
                "target_path": challenge.path,
            });
            return Ok(base64::engine::general_purpose::STANDARD.encode(value.to_string()));
        }
    }
    Err(http_error(
        502,
        Some("deepseek_pow_unsolved"),
        "DeepSeek 网页验证没有找到有效答案，未提交对应请求",
    ))
}

fn decimal(mut value: u64, output: &mut Vec<u8>) {
    let mut digits = [0u8; 20];
    let mut at = digits.len();
    loop {
        at -= 1;
        digits[at] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    output.extend_from_slice(&digits[at..]);
}

const ROUND: [u64; 24] = [
    0x1,
    0x8082,
    0x800000000000808a,
    0x8000000080008000,
    0x808b,
    0x80000001,
    0x8000000080008081,
    0x8000000000008009,
    0x8a,
    0x88,
    0x80008009,
    0x8000000a,
    0x8000808b,
    0x800000000000008b,
    0x8000000000008089,
    0x8000000000008003,
    0x8000000000008002,
    0x8000000000000080,
    0x800a,
    0x800000008000000a,
    0x8000000080008081,
    0x8000000000008080,
    0x80000001,
    0x8000000080008008,
];
const ROTATION: [[u32; 5]; 5] = [
    [0, 36, 3, 41, 18],
    [1, 44, 10, 45, 2],
    [62, 6, 43, 15, 61],
    [28, 55, 25, 21, 56],
    [27, 20, 39, 8, 14],
];

// 原生 DeepSeekHashV1 使用 23 轮 Keccak-256（跳过第 0 轮常量）。
fn permutation(state: &mut [u64; 25]) {
    for round in &ROUND[1..] {
        let mut c = [0u64; 5];
        for x in 0..5 {
            for y in 0..5 {
                c[x] ^= state[x + 5 * y];
            }
        }
        for x in 0..5 {
            let d = c[(x + 4) % 5] ^ c[(x + 1) % 5].rotate_left(1);
            for y in 0..5 {
                state[x + 5 * y] ^= d;
            }
        }
        let mut b = [0u64; 25];
        for x in 0..5 {
            for y in 0..5 {
                b[y + 5 * ((2 * x + 3 * y) % 5)] = state[x + 5 * y].rotate_left(ROTATION[x][y]);
            }
        }
        for x in 0..5 {
            for y in 0..5 {
                state[x + 5 * y] =
                    b[x + 5 * y] ^ ((!b[(x + 1) % 5 + 5 * y]) & b[(x + 2) % 5 + 5 * y]);
            }
        }
        state[0] ^= round;
    }
}

fn absorb(state: &mut [u64; 25], block: &[u8]) {
    for (index, lane) in block.chunks_exact(8).enumerate() {
        let mut value = [0u8; 8];
        value.copy_from_slice(lane);
        state[index] ^= u64::from_le_bytes(value);
    }
    permutation(state);
}

fn hash(data: &[u8]) -> [u8; 32] {
    const RATE: usize = 136;
    let mut state = [0u64; 25];
    let mut blocks = data.chunks_exact(RATE);
    for block in &mut blocks {
        absorb(&mut state, block);
    }
    let rest = blocks.remainder();
    let mut last = [0u8; RATE];
    last[..rest.len()].copy_from_slice(rest);
    last[rest.len()] = 0x06;
    last[RATE - 1] ^= 0x80;
    absorb(&mut state, &last);
    let mut output = [0u8; 32];
    for (index, lane) in output.chunks_exact_mut(8).enumerate() {
        lane.copy_from_slice(&state[index].to_le_bytes());
    }
    output
}
