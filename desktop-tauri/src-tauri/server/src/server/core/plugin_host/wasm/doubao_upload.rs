use std::{collections::BTreeMap, io::Cursor, time::Duration};

use agent2api_provider_protocol as proto;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{checkin_call::doubao_daily::credentials, context::Context};
use crate::server::core::{account_store::{credential_version::RowProvider, plugin_accounts}, blocking, egress::managed::FrozenHttpClient};

const MAX_IMAGE: usize = 20 * 1024 * 1024;

pub(super) async fn upload(context: &Context, params: &Value) -> Result<Value, proto::WireError> {
    context.permission("http.doubao-turn.v1")?;
    context.permission("resources.v1")?;
    if context.shared.options.provider != "doubaowork"
        || params.as_object().is_none_or(|fields| fields.len() != 1 || !fields.contains_key("resource")) {
        return Err(invalid());
    }
    let call = context.conversation.as_ref().ok_or_else(invalid)?;
    let path = call.selected_path(&context.shared, context.account()?)?;
    let bytes = context.blobs.raw(params["resource"].as_str().ok_or_else(invalid)?)?;
    if bytes.is_empty() || bytes.len() > MAX_IMAGE { return Err(invalid()); }
    let format = image::guess_format(&bytes).map_err(|_| invalid())?;
    let extension = match format {
        image::ImageFormat::Png => ".png", image::ImageFormat::Jpeg => ".jpg", image::ImageFormat::WebP => ".webp", _ => return Err(invalid()),
    };
    let (width, height) = image::ImageReader::with_format(Cursor::new(&bytes), format).into_dimensions().map_err(|_| invalid())?;
    if width == 0 || height == 0 || width > 16_384 || height > 16_384 { return Err(invalid()); }
    let store = plugin_accounts::shared().ok_or_else(invalid)?;
    let account = context.account()?.to_string();
    let row = blocking::run(move || store.prepare_row_source(RowProvider::DoubaoWork, &account)).await.map_err(|_| invalid())?.map_err(|_| invalid())?;
    let credentials = credentials::from_original(&row, crate::server::logging::now_ms()).map_err(|_| invalid())?;
    let client = FrozenHttpClient::from_path(path).await.map_err(|_| invalid())?;
    call.validate_context(&context.shared, &context.account)?;
    let deadline = context.call_deadline.min(tokio::time::Instant::now() + Duration::from_secs(180));
    checkpoint(&row, deadline).await?;
    tokio::time::timeout_at(deadline, async {
        let request = client.client().map_err(|_| invalid())?.post(credentials.url("/alice/resource/prepare_upload"))
            .headers(credentials.headers(false).map_err(|_| invalid())?)
            .json(&json!({"tenant_id":"5","scene_id":"4","resource_type":2})).build().map_err(|_| invalid())?;
        let prepared = send(&client, request).await?;
        call.validate_context(&context.shared, &context.account)?;
        checkpoint(&row, deadline).await?;
        if prepared["code"].as_i64().is_some_and(|code| code != 0) || prepared["error_code"].as_i64().is_some_and(|code| code != 0) { return Err(invalid()); }
        let prep = &prepared["data"];
        let host = required(prep, "upload_host")?;
        let endpoint = official(host)?;
        let auth = &prep["upload_auth_token"];
        let token = required(auth, "session_token")?;
        let service = required(prep, "service_id")?;
        let region = prep["region"].as_str().unwrap_or("cn-north-1");
        if region.len() > 64 || !region.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-') { return Err(invalid()); }
        let query = BTreeMap::from([("Action", "ApplyImageUpload".into()), ("Version", "2018-08-01".into()),
            ("ServiceId", service.into()), ("FileSize", bytes.len().to_string()), ("FileExtension", extension.into())]);
        let time = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let (query, signature) = sign(auth, "GET", host, &query, "", region, &time)?;
        let request = client.client().map_err(|_| invalid())?.get(format!("{endpoint}/?{query}"))
            .header("x-amz-date", &time).header("x-amz-security-token", token).header("Authorization", signature).build().map_err(|_| invalid())?;
        let applied = send(&client, request).await?;
        call.validate_context(&context.shared, &context.account)?;
        checkpoint(&row, deadline).await?;
        let address = &applied["Result"]["UploadAddress"];
        let store = &address["StoreInfos"][0];
        let host = address["UploadHosts"][0].as_str().ok_or_else(invalid)?;
        let uri = required(store, "StoreUri")?;
        check_uri(uri)?;
        let mut crc = flate2::Crc::new(); crc.update(&bytes);
        call.validate_context(&context.shared, &context.account)?;
        let request = client.client().map_err(|_| invalid())?.post(format!("{}/upload/v1/{uri}", official(host)?))
            .header("Authorization", required(store, "Auth")?).header("Content-CRC32", format!("{:08x}", crc.sum()))
            .header("Content-Type", "application/octet-stream").body(bytes).build().map_err(|_| invalid())?;
        let uploaded = send(&client, request).await?;
        call.validate_context(&context.shared, &context.account)?;
        checkpoint(&row, deadline).await?;
        if uploaded["code"].as_i64() != Some(2000) { return Err(invalid()); }
        let body = json!({"SessionKey":required(address,"SessionKey")?}).to_string();
        let time = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let query = BTreeMap::from([("Action", "CommitImageUpload".into()), ("Version", "2018-08-01".into()), ("ServiceId", service.into())]);
        let (query, signature) = sign(auth, "POST", required(prep,"upload_host")?, &query, &body, region, &time)?;
        let request = client.client().map_err(|_| invalid())?.post(format!("{endpoint}/?{query}"))
            .header("x-amz-date", &time).header("x-amz-security-token", token).header("Authorization", signature)
            .header("Content-Type", "application/json").body(body).build().map_err(|_| invalid())?;
        let committed = send(&client, request).await?;
        let result = &committed["Result"]["Results"][0];
        if result["UriStatus"].as_i64() != Some(2000) { return Err(invalid()); }
        let uri = required(result, "Uri")?; check_uri(uri)?;
        call.validate_context(&context.shared, &context.account)?;
        checkpoint(&row, deadline).await?;
        Ok(json!({"type":1,"identifier":crate::server::core::protocol::random_id("image"),
            "image":{"name":format!("image{extension}"),"uri":uri,"image_ori":{"url":"","width":width,"height":height,"format":"","url_formats":{}}},
            "parse_state":0,"review_state":1,"upload_status":1,"progress":100,"src":""}))
    }).await.map_err(|_| invalid())?
}

async fn checkpoint(row: &crate::server::core::account_store::credential_version::RowSource,
    deadline: tokio::time::Instant) -> Result<(), proto::WireError> {
    super::doubao_local_gate::wait(deadline, invalid, || {
        Ok(row.try_final_check(|| Ok::<(), proto::WireError>(())))
    }).await
}

fn sign(auth: &Value, method: &str, host: &str, query: &BTreeMap<&str, String>, body: &str, region: &str, time: &str) -> Result<(String,String), proto::WireError> {
    let date = &time[..8];
    let query = query.iter().map(|(name,value)| format!("{name}={}", encode(value))).collect::<Vec<_>>().join("&");
    let headers = format!("host:{host}\nx-amz-date:{time}\nx-amz-security-token:{}\n", required(auth,"session_token")?);
    let names = "host;x-amz-date;x-amz-security-token";
    let canonical = format!("{method}\n/\n{query}\n{headers}\n{names}\n{:x}", Sha256::digest(body));
    let scope = format!("{date}/{region}/imagex/aws4_request");
    let text = format!("AWS4-HMAC-SHA256\n{time}\n{scope}\n{:x}", Sha256::digest(canonical));
    let key = mac(&mac(&mac(&mac(format!("AWS4{}",required(auth,"secret_key")?).as_bytes(),date)?,region)?,"imagex")?,"aws4_request")?;
    let signature = mac(&key,&text)?.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    Ok((query,format!("AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={names}, Signature={signature}",required(auth,"access_key")?)))
}
fn mac(key: &[u8], value: &str) -> Result<Vec<u8>,proto::WireError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_|invalid())?; mac.update(value.as_bytes()); Ok(mac.finalize().into_bytes().to_vec())
}
fn encode(value: &str) -> String {
    value.bytes().map(|byte| if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) { char::from(byte).to_string() } else {format!("%{byte:02X}")}).collect()
}
fn official(host: &str) -> Result<String,proto::WireError> {
    let url = url::Url::parse(&format!("https://{host}/")).map_err(|_|invalid())?;
    if url.host_str()!=Some(host) || url.path()!="/" || url.port().is_some() || !url.username().is_empty() || url.password().is_some()
        || ![".bytedanceapi.com",".bytevcloudapi.com",".volcengineapi.com",".byteimg.com",".ibyteimg.com",".ibytedtos.com",".imagex.cn",".douyinpic.com"].iter().any(|suffix|host.ends_with(suffix)) {return Err(invalid());}
    Ok(format!("https://{host}"))
}
fn check_uri(uri: &str) -> Result<(),proto::WireError> {
    if uri.len()>2048 || uri.starts_with('/') || !uri.bytes().all(|byte|byte.is_ascii_alphanumeric() || b"/_-.".contains(&byte))
        || uri.split('/').any(|part|matches!(part,"."|".."|"")) {return Err(invalid());} Ok(())
}
fn required<'a>(value: &'a Value,key: &str) -> Result<&'a str,proto::WireError> {
    value[key].as_str().filter(|text|!text.is_empty() && text.len()<=8192 && !text.chars().any(char::is_control)).ok_or_else(invalid)
}
async fn send(client: &FrozenHttpClient, request: reqwest::Request) -> Result<Value,proto::WireError> {
    let mut response = client.client().map_err(|_|invalid())?.execute(request).await.map_err(|_|invalid())?;
    if !response.status().is_success() || response.headers().contains_key("location") || response.content_length().is_some_and(|bytes|bytes>256*1024) {return Err(invalid());}
    let mut bytes=Vec::new();
    while let Some(chunk)=response.chunk().await.map_err(|_|invalid())? {if bytes.len()+chunk.len()>256*1024{return Err(invalid());}bytes.extend_from_slice(&chunk);}
    super::managed_task::declaration::parse_object(&bytes,256*1024).map_err(|_|invalid())
}
fn invalid() -> proto::WireError {proto::WireError::with_code("豆包工作图片上传未确认，未自动重传；仅支持不超过 20 MiB 的 PNG、JPEG 和 WebP 图片","doubaowork_image_upload_unconfirmed")}
