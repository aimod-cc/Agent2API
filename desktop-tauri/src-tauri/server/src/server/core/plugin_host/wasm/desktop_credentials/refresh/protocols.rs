use agent2api_provider_protocol as proto;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::{json, Value};
use md5::{Digest, Md5};

use super::{Job, changed, super::invalid};
use crate::server::core::{egress::managed::FrozenHttpClient, proxies::ResolvedProxy};

fn text(value:&Value,keys:&[&str])->String{keys.iter().find_map(|key|value[*key].as_str().filter(|value|!value.is_empty()).map(str::to_string)).unwrap_or_default()}
fn claims(token:&str)->Value{token.trim_start_matches("workos:").split('.').nth(1).and_then(|value|URL_SAFE_NO_PAD.decode(value.trim_end_matches('=')).ok()).and_then(|bytes|serde_json::from_slice(&bytes).ok()).unwrap_or(Value::Null)}
fn uid(value:&Value)->String{match &value["user_id"]{Value::String(value)=>value.clone(),Value::Number(value)=>value.to_string(),_=>String::new()}}
fn expires(data:&Value,access:&str)->Option<i64>{
    let now=crate::server::logging::now_ms();
    data["expiresIn"].as_i64().or_else(||data["expires_in"].as_i64()).filter(|value|*value>0).and_then(|value|value.checked_mul(1000)).and_then(|value|now.checked_add(value))
        .or_else(||claims(access)["exp"].as_i64().and_then(|value|value.checked_mul(1000)))
        .or_else(||data["expiresAt"].as_i64())
}

async fn request(job:&Job,url:&str,headers:Value,body:Vec<u8>)->Result<(u16,Value),proto::WireError>{
    job.current()?;
    job.shared.options.check_url(url,false)?;
    let proxy=ResolvedProxy::from_json(&job.proxy).map_err(|_|invalid())?;
    let client=FrozenHttpClient::from_selected(proxy).await.map_err(|_|invalid())?;
    tokio::time::timeout_at(job.deadline,async{
        let mut request=client.client().map_err(|_|invalid())?.post(url).body(body);
        for (name,value) in headers.as_object().into_iter().flatten(){request=request.header(name,value.as_str().ok_or_else(invalid)?);}
        if !job.user_agent.is_empty(){request=request.header("User-Agent",&job.user_agent);}
        let mut response=request.send().await.map_err(|_|invalid())?;
        if response.url().as_str()!=url||response.headers().contains_key("location"){return Err(invalid());}
        let status=response.status().as_u16();
        let mut bytes=zeroize::Zeroizing::new(Vec::new());
        while let Some(chunk)=response.chunk().await.map_err(|_|invalid())?{
            if bytes.len().saturating_add(chunk.len())>256*1024{return Err(invalid());}
            bytes.extend_from_slice(&chunk);
        }
        let value=serde_json::from_slice(&bytes).map_err(|_|invalid())?;
        Ok((status,value))
    }).await.map_err(|_|invalid())?
}

pub(super) async fn refresh(job:&Job,old:&Value)->Result<Value,proto::WireError>{
    let refresh=text(old,&["refresh","refreshToken","refresh_token"]);
    let access=text(old,&["access","accessToken","token"]);
    if refresh.is_empty(){return Err(invalid());}
    if job.provider=="kiro"{
        let method=text(old,&["method"]);let region=text(old,&["region"]);
        let (url,headers,body)=match method.as_str(){
            "social"=>(format!("https://prod.{}.auth.desktop.kiro.dev/refreshToken",if region.is_empty(){"us-east-1"}else{&region}),
                json!({"Content-Type":"application/json","Accept":"application/json","User-Agent":"Kiro-Desktop/0.2.13"}),json!({"refreshToken":refresh}).to_string().into_bytes()),
            "idc"=>(format!("https://oidc.{}.amazonaws.com/token",if region.is_empty(){"us-east-1"}else{&region}),
                json!({"Content-Type":"application/json","Accept":"application/json"}),json!({"grantType":"refresh_token","refreshToken":refresh,"clientId":old["clientId"],"clientSecret":old["clientSecret"]}).to_string().into_bytes()),
            "external-idp"=>{
                return super::super::super::external_refresh::kiro(job,old).await;
            }
            _=>return Err(invalid()),
        };
        let (status,value)=request(job,&url,headers,body).await?;
        if !(200..300).contains(&status)||value.get("error").is_some_and(|value|!value.is_null()){return Err(invalid());}
        let next_access=text(&value,&["accessToken","access_token"]);let next_refresh=text(&value,&["refreshToken","refresh_token"]);
        if next_access.is_empty(){return Err(invalid());}
        let mut next=old.clone();next["access"]=json!(next_access);next["refresh"]=json!(if next_refresh.is_empty(){refresh}else{next_refresh});
        next["expires"]=json!(expires(&value,&next_access));
        return Ok(next);
    }
    if job.provider.starts_with("autoclaw"){
        let origin=if job.provider=="autoclaw-intl"{"https://autoglm-api.autoglm.ai"}else{"https://autoglm-acceleration-api.zhipuai.cn"};
        let now=(crate::server::logging::now_ms()/1000).to_string();
        let sign=format!("{:x}",Md5::digest(format!("100003&{now}&38d2391985e2369a5fb8227d8e6cd5e5")));
        let headers=json!({"Content-Type":"application/json","Accept":"*/*","Authorization":format!("Bearer {access}"),
            "X-Version":"1.18.5","X-Product":"autoclaw","X-Client-Type":"pc","X-Harness-Type":"zcode","X-Tm":"win","X-Lang":"zh-CN","X-Channel":"official",
            "X-Auth-Appid":"100003","X-Auth-TimeStamp":now,"X-Auth-Sign":sign});
        let body=json!({"refresh_token":refresh,"source_id":"autoclaw","device_id":old["deviceId"]});
        let (mut status,mut value)=request(job,&format!("{origin}/userapi/v1/refresh"),headers.clone(),body.to_string().into_bytes()).await?;
        if value["code"]==400002{
            (status,value)=request(job,&format!("{origin}/userapi/v1/agent-refresh"),headers,body.to_string().into_bytes()).await?;
        }
        if !(200..300).contains(&status)||value["code"]!=0{return Err(invalid());}
        let data=&value["data"];let next_access=text(data,&["access_token"]);let next_refresh=text(data,&["refresh_token"]);
        if next_access.is_empty()||uid(&claims(&next_access))!=uid(&claims(&access)){return Err(changed());}
        let mut next=old.clone();next["token"]=json!(next_access);next["accessToken"]=json!(next_access);next["refreshToken"]=json!(if next_refresh.is_empty(){refresh}else{next_refresh});next["expiresAt"]=json!(expires(data,&next_access));
        return Ok(next);
    }
    let (status,value)=request(job,"https://api.cline.bot/api/v1/auth/refresh",json!({"Content-Type":"application/json","Accept":"application/json"}),json!({"refreshToken":refresh,"grantType":"refresh_token"}).to_string().into_bytes()).await?;
    if matches!(status, 401 | 403) {
        return Err(proto::WireError::with_code("Cline 本机续期凭据已被拒绝，请在官方客户端重新登录", "refresh_token_expired"));
    }
    if !(200..300).contains(&status){return Err(invalid());}
    let raw=serde_json::to_vec(&value).map_err(|_|invalid())?;
    let account=text(old,&["account"]);
    let (next, _) = super::cline::replacement(&raw, &account, &json!({"name": account}), Some(&refresh))?;
    Ok(next.0.clone())
}
