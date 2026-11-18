//! 仅 DeepSeek 显式启用的 Chat 内联图片入场，不处理图片生成或公网下载。
//! 原 Call、Shared 和 params 封闭按值传递；JSON 资源标识不构成入场凭据。
//! 解码只在原 Runtime 并发名额内进行，失败/取消直接丢弃本批资源。

mod format;
mod gif;
mod plan;

use std::io::{self, Write};
use std::sync::{Arc, Weak};

use agent2api_provider_protocol as proto;
use base64::Engine;
use serde::Serialize;
use serde_json::Value;
use tokio::time::Instant;

use super::context::{Context, Shared, Sink};
use super::conversation::{ConversationCall, ExecutionGuard};
use super::resources::Resources;

pub(super) const CAPABILITY: &str = "resources.inbound-images.v1";
pub(super) const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
pub(super) const MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;
pub(super) const MAX_IMAGES: usize = 50;
// 原 Context 共 16 个 blob；保留一个供顺序公网下载/临时处理，不抬高原有数量上限。
pub(super) const MAX_INLINE_IMAGES: usize = 15;
const BASE64_CHUNK: usize = 64 * 1024;

/// 只由真实 RPC Call 在绑定 Shared 后创建；不提供字段、克隆或反序列化入口。
pub(super) struct Prepared {
    call: ConversationCall,
    shared: Weak<Shared>,
    params: Value,
}

pub(super) fn prepare(
    call: ConversationCall,
    shared: &Arc<Shared>,
    params: Value,
) -> Result<Prepared, proto::WireError> {
    call.validate_params(shared, &params)?;
    Ok(Prepared {
        call,
        shared: Arc::downgrade(shared),
        params,
    })
}

impl Prepared {
    pub(super) fn scheduling(&self) -> (&str, &str, usize) {
        (self.params["account"].as_str().unwrap_or(""), self.call.capacity_caller(),
            crate::server::core::runtime_capacity::value_bytes(&self.params))
    }

    pub(super) fn matches(&self, guard: &ExecutionGuard) -> bool {
        guard.matches(&self.call)
    }

    /// 必须由原 invoke_inner 在取得原并发名额之后、任何 account.get/ABI 序列化之前调用。
    /// 没有外部 params 参数，不能将验证过的 Call 与另一份请求正文拼接。
    pub(super) async fn into_context(
        self,
        shared: &Arc<Shared>,
        sink: Sink,
        deadline: Instant,
    ) -> Result<(Context, Value), proto::WireError> {
        let Self {
            call,
            shared: source,
            mut params,
        } = self;
        if !Weak::ptr_eq(&source, &Arc::downgrade(shared)) {
            return Err(unavailable());
        }
        call.validate_params(shared, &params)?;
        let enabled = shared.options.allows(CAPABILITY);
        let deepseek = shared.options.provider == "deepseek-web";
        if enabled && (!deepseek || !shared.options.allows("resources.v1")) {
            return Err(proto::WireError::with_code(
                "内联图片资源仅接入显式声明资源能力的 DeepSeek 网页对话",
                "forbidden",
            ));
        }
        let account = params["account"]
            .as_str()
            .ok_or_else(unavailable)?
            .to_string();
        let cancel = sink.cancel.clone();
        let work = async {
            checkpoint(&call, shared, &account, &cancel, deadline)?;
            let mut resources = Resources::default();
            if deepseek {
                // 未启用新能力也检查保留块；不能把客户端伪造标识交给新旧插件。
                let pending = plan::extract(&mut params, enabled).await?;
                if enabled {
                    // 先把所有可计算预算和移除 base64 后的完整信封核对完，再分配图片。
                    envelope_budget(&params)?;
                    // 整批先做无大分配的完整 base64/GIF 检查；任何坏图都先于首个资源登记。
                    for image in &pending {
                        validate_encoded(image, &call, shared, &account, &cancel, deadline).await?;
                    }
                    for image in pending {
                        checkpoint(&call, shared, &account, &cancel, deadline)?;
                        let decoded =
                            decode(&image, &call, shared, &account, &cancel, deadline).await?;
                        let position = image.position();
                        // 当前图片 base64 不再需要；封存前释放这一份已移出的原内容块。
                        drop(image);
                        let block = resources.seal_inbound(decoded)?;
                        plan::replace(&mut params, position, block)?;
                        tokio::task::yield_now().await;
                    }
                }
            }
            if shared.options.provider == "catpaw" {
                super::inline_images::prepare(&mut params["body"], &shared.options).await?;
                checkpoint(&call, shared, &account, &cancel, deadline)?;
            }
            Ok::<_, proto::WireError>(resources)
        };
        let resources = tokio::select! {
            biased;
            _ = call.ended() => return Err(unavailable()),
            _ = shared.cancel.cancelled() => return Err(unavailable()),
            _ = cancel.cancelled() => return Err(cancelled()),
            result = tokio::time::timeout_at(deadline, work) => result.map_err(|_| timed_out())??,
        };
        checkpoint(&call, shared, &account, &cancel, deadline)?;
        call.validate_params(shared, &params)?;
        // 只给本次 Context 安装原字节和原 Call。不存在可跨调用查找的图片表。
        let catpaw_material = if shared.options.provider == "catpaw" {
            Some(Arc::new(super::conversation::CatPawMaterial::from_call_body(&call, &params["body"])?))
        } else {
            None
        };
        let mut context = Context::new(shared.clone(), &params, Some(sink), deadline);
        context.blobs = resources;
        context.catpaw_material = catpaw_material;
        context.conversation = Some(call);
        Ok((context, params))
    }
}

/// 只有成功解码/校验路径能构造。Resources 只按值消费，不能从插件 JSON 构造。
pub(super) struct SealedImage {
    mime: &'static str,
    bytes: Vec<u8>,
}

impl SealedImage {
    pub(super) fn into_parts(self) -> (&'static str, Vec<u8>) {
        (self.mime, self.bytes)
    }
}

async fn validate_encoded(
    image: &plan::PendingImage,
    call: &ConversationCall,
    shared: &Arc<Shared>,
    account: &str,
    cancel: &tokio_util::sync::CancellationToken,
    deadline: Instant,
) -> Result<(), proto::WireError> {
    let encoded = image.encoded()?;
    let mut size = 0usize;
    let mut gif = (image.format() == format::Format::Gif).then(gif::Validator::new);
    for (index, chunk) in encoded.as_bytes().chunks(BASE64_CHUNK).enumerate() {
        checkpoint(call, shared, account, cancel, deadline)?;
        if (index + 1) * BASE64_CHUNK < encoded.len() && chunk.contains(&b'=') {
            return Err(invalid("DeepSeek 内联图片 base64 中间出现结束填充"));
        }
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(chunk)
            .map_err(|_| invalid("DeepSeek 内联图片 base64 无效"))?;
        size = size
            .checked_add(decoded.len())
            .filter(|size| *size <= image.size())
            .ok_or_else(|| invalid("DeepSeek 内联图片实际字节数与编码长度不一致"))?;
        if index == 0 {
            image.format().check_magic(&decoded)?;
        }
        if let Some(validator) = &mut gif {
            validator.push(&decoded)?;
        }
        drop(decoded);
        tokio::task::yield_now().await;
    }
    if size != image.size() || size == 0 {
        return Err(invalid("DeepSeek 内联图片实际字节数与编码长度不一致"));
    }
    if let Some(validator) = gif {
        validator.finish()?;
    }
    checkpoint(call, shared, account, cancel, deadline)
}

async fn decode(
    image: &plan::PendingImage,
    call: &ConversationCall,
    shared: &Arc<Shared>,
    account: &str,
    cancel: &tokio_util::sync::CancellationToken,
    deadline: Instant,
) -> Result<SealedImage, proto::WireError> {
    checkpoint(call, shared, account, cancel, deadline)?;
    let encoded = image.encoded()?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(image.size())
        .map_err(|_| limit("DeepSeek 图片内存分配失败，未进入对话"))?;
    let mut gif = (image.format() == format::Format::Gif).then(gif::Validator::new);
    for (index, chunk) in encoded.as_bytes().chunks(BASE64_CHUNK).enumerate() {
        checkpoint(call, shared, account, cancel, deadline)?;
        // 64 KiB 是 4 的倍数；只有最后一个片段可以出现 base64 结束填充。
        if (index + 1) * BASE64_CHUNK < encoded.len() && chunk.contains(&b'=') {
            return Err(invalid("DeepSeek 内联图片 base64 中间出现结束填充"));
        }
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(chunk)
            .map_err(|_| invalid("DeepSeek 内联图片 base64 无效"))?;
        if bytes
            .len()
            .checked_add(decoded.len())
            .is_none_or(|size| size > image.size())
        {
            return Err(invalid("DeepSeek 内联图片实际字节数与编码长度不一致"));
        }
        if index == 0 {
            image.format().check_magic(&decoded)?;
        }
        if let Some(validator) = &mut gif {
            validator.push(&decoded)?;
        }
        bytes.extend_from_slice(&decoded);
        // 不 spawn_blocking；取消会直接 drop 此 future 和它独占的图片缓冲区。
        tokio::task::yield_now().await;
    }
    if bytes.len() != image.size() || bytes.is_empty() {
        return Err(invalid("DeepSeek 内联图片实际字节数与编码长度不一致"));
    }
    if let Some(validator) = gif {
        validator.finish()?;
    }
    checkpoint(call, shared, account, cancel, deadline)?;
    Ok(SealedImage {
        mime: image.format().mime(),
        bytes,
    })
}

fn checkpoint(
    call: &ConversationCall,
    shared: &Arc<Shared>,
    account: &str,
    cancel: &tokio_util::sync::CancellationToken,
    deadline: Instant,
) -> Result<(), proto::WireError> {
    if cancel.is_cancelled() {
        return Err(cancelled());
    }
    if shared.cancel.is_cancelled() {
        return Err(unavailable());
    }
    if Instant::now() >= deadline {
        return Err(timed_out());
    }
    // 原 Call 同代检查，不创建/冻结 ReplayGuard；是否已投递仍由原发送入口维护。
    // account 只从 Prepared 内已通过 validate_params 的原参数取得，不接受外部另传。
    call.validate_context(shared, account)
}

#[derive(Serialize)]
struct Envelope<'a> {
    method: &'static str,
    params: &'a Value,
}

struct BudgetWriter {
    bytes: usize,
}

impl Write for BudgetWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .filter(|size| *size <= super::MAX_MESSAGE)
            .ok_or_else(|| io::Error::other("inbound_message_limit"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn envelope_budget(params: &Value) -> Result<(), proto::WireError> {
    let envelope = Envelope {
        method: proto::methods::gateway::CONVERSATION,
        params,
    };
    serde_json::to_writer(BudgetWriter { bytes: 0 }, &envelope).map_err(|_| {
        limit("DeepSeek 图片资源化后其余请求内容与调用信封仍超过 16 MiB，请缩短非图片内容")
    })
}

fn invalid(message: &str) -> proto::WireError {
    proto::WireError::with_code(message, "bad_params")
}

fn limit(message: &str) -> proto::WireError {
    proto::WireError::with_code(message, "plugin_resource_limit")
}

fn unavailable() -> proto::WireError {
    proto::WireError::with_code(
        "图片准备所属对话已结束或执行体已切换",
        "request_call_revoked",
    )
}

fn cancelled() -> proto::WireError {
    proto::WireError::with_code("图片准备已取消", proto::WireError::ABORTED)
}

fn timed_out() -> proto::WireError {
    proto::WireError::with_code("图片准备超过本次对话期限", "timeout")
}
