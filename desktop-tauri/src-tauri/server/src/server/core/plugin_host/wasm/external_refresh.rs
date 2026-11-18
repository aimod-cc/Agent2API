//! 外部身份提供商续期只使用当前账号保存的端点和材料，不开放任意HTTP调用。

use agent2api_provider_protocol as proto;
use serde_json::{json, Value};

use super::{context::Context, desktop_credentials::{invalid, refresh::Job}};

fn text(value:&Value,keys:&[&str])->String{keys.iter().find_map(|key|value[*key].as_str().filter(|value|!value.is_empty()).map(str::to_string)).unwrap_or_default()}
fn endpoint(secret:&Value)->Result<url::Url,proto::WireError>{
    let raw=text(secret,&["tokenURL","tokenUrl","tokenEndpoint","token_endpoint"]);
    let url=url::Url::parse(&raw).map_err(|_|invalid())?;
    if url.scheme()!="https"||url.port_or_known_default()!=Some(443)||!url.username().is_empty()
        ||url.password().is_some()||url.query().is_some()||url.fragment().is_some()
        ||url.host_str().is_none_or(|host|host.parse::<std::net::IpAddr>().is_ok()||!host.contains('.')){return Err(invalid());}
    Ok(url)
}
async fn exchange(secret:&Value,proxy:&Value,ua:&str,deadline:tokio::time::Instant)->Result<Value,proto::WireError>{
    let url=endpoint(secret)?;
    let client_id=text(secret,&["clientId","client_id"]);let refresh=text(secret,&["refresh","refreshToken","refresh_token"]);
    if client_id.is_empty()||refresh.is_empty(){return Err(invalid());}
    let addresses=super::account_origin::network::addresses(&url).await?;
    let proxy=crate::server::core::proxies::ResolvedProxy::from_json(proxy).map_err(|_|invalid())?;
    let client=crate::server::core::egress::resolved_client(proxy.as_ref(),false,url.host_str().ok_or_else(invalid)?,&addresses).map_err(|_|invalid())?;
    tokio::time::timeout_at(deadline,async{
        let mut response=client.post(url.clone()).header("Accept","application/json").header("User-Agent",if ua.is_empty(){"kiro-cli"}else{ua})
            .form(&[("grant_type","refresh_token"),("client_id",client_id.as_str()),("refresh_token",refresh.as_str())]).send().await.map_err(|_|invalid())?;
        if !response.status().is_success()||response.url()!=&url||response.headers().contains_key("location"){return Err(invalid());}
        let mut bytes=zeroize::Zeroizing::new(Vec::new());
        while let Some(chunk)=response.chunk().await.map_err(|_|invalid())?{if bytes.len().saturating_add(chunk.len())>128*1024{return Err(invalid());}bytes.extend_from_slice(&chunk);}
        let value:Value=super::managed_task::declaration::parse_object(&bytes,128*1024).map_err(|_|invalid())?;
        let access=text(&value,&["access_token"]);let next=text(&value,&["refresh_token"]);
        if access.is_empty()||access.len()>8192||access.chars().any(char::is_control){return Err(invalid());}
        let expiry=value["expires_in"].as_i64().filter(|value|*value>0).and_then(|value|value.checked_mul(1000)).and_then(|value|crate::server::logging::now_ms().checked_add(value));
        Ok(json!({"accessToken":access,"refreshToken":if next.is_empty(){refresh}else{next},"expiresAt":expiry}))
    }).await.map_err(|_|invalid())?
}

pub(super) async fn kiro(job:&Job,secret:&Value)->Result<Value,proto::WireError>{
    job.current()?;
    let next=exchange(secret,&job.proxy,&job.user_agent,job.deadline).await?;
    job.current()?;
    let mut result=secret.clone();result["access"]=next["accessToken"].clone();result["refresh"]=next["refreshToken"].clone();result["expires"]=next["expiresAt"].clone();
    Ok(result)
}

pub(super) async fn settle(context: &mut Context, expect: &Value, secret: &Value) -> Result<(), proto::WireError> {
    if context.shared.options.provider != "kiro" || expect["method"] != "external-idp" { return Ok(()); }
    let id = context.account()?.to_string();
    let store = crate::server::core::account_store::plugin_accounts::shared().ok_or_else(invalid)?;
    let current = store.plugin_account(&id).ok_or_else(invalid)?;
    if current["desktop"] == true { return Ok(()); }
    if current["secret"] != *secret { return Err(invalid()); }
    let reserved = store.plugin_state_strict("kiro", &id, "host.kiro-external-refresh.v1").map_err(|_| invalid())?;
    if reserved["value"].is_null() { return Ok(()); }
    use sha2::{Digest, Sha256};
    let expected = format!("{:x}", Sha256::digest(format!("{}:{}", endpoint(expect)?, text(expect, &["refresh", "refreshToken"]))));
    if reserved["value"]["stamp"] != expected { return Ok(()); }
    if store.plugin_state_compare_exchange_strict("kiro", &id, "host.kiro-external-refresh.v1",
        reserved["revision"].as_u64().ok_or_else(invalid)?, 1, Value::Null).map_err(|_| invalid())?["updated"] != true {
        return Err(invalid());
    }
    Ok(())
}

pub(super) async fn invoke(context:&mut Context,params:&Value)->Result<Value,proto::WireError>{
    context.permission("credential.kiro-external-refresh.v1")?;
    if context.shared.options.provider!="kiro"||params.as_object().is_none_or(|fields|!fields.is_empty()){return Err(invalid());}
    let id=context.account()?.to_string();
    let mut record=context.shared.callbacks.store_get("kiro".into(),id.clone()).await?;
    let original_secret = record["secret"].clone();
    super::legacy_secret::project(&mut record);
    let secret=&record["secret"];
    if secret["method"]!="external-idp"||record["desktop"]==true{return Err(invalid());}
    let endpoint=endpoint(secret)?;
    let stamp=format!("{}:{}",endpoint,text(secret,&["refresh","refreshToken"]));
    use sha2::{Digest,Sha256};let stamp=format!("{:x}",Sha256::digest(stamp));
    let store=crate::server::core::account_store::plugin_accounts::shared().ok_or_else(invalid)?;
    let before=store.plugin_state_strict("kiro",&id,"host.kiro-external-refresh.v1").map_err(|_|invalid())?;
    if before["value"]["stamp"]==stamp{return Err(proto::WireError::with_code("上次外部授权续期结果未确认，请重新登录；不会重复消费原凭据","kiro_external_refresh_unconfirmed"));}
    if store.plugin_state_compare_exchange_strict("kiro",&id,"host.kiro-external-refresh.v1",before["revision"].as_u64().ok_or_else(invalid)?,1,json!({"stamp":stamp})).map_err(|_|invalid())?["updated"]!=true{return Err(invalid());}
    let next=exchange(secret,&context.proxy,context.user_agent.as_deref().unwrap_or(""),context.call_deadline).await?;
    let current=context.shared.callbacks.store_get("kiro".into(),id).await?;
    if current["secret"]!=original_secret||current["identity"]!=record["identity"]{return Err(invalid());}
    Ok(next)
}
