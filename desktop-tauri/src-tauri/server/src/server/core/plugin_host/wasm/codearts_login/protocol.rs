use agent2api_provider_protocol as proto;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD,Engine};
use p256::ecdsa::{SigningKey,Signature,signature::hazmat::PrehashSigner};
use serde_json::{json,Value};
use sha2::{Digest,Sha256};
use hmac::{Hmac,Mac};
use zeroize::Zeroizing;
use crate::server::core::egress::managed::FrozenHttpClient;
use super::invalid;

const TOKEN: &str = "https://sts.cn-north-4.myhuaweicloud.com/v1/oauth2/tokens";
const IDENTITY: &str = "https://sts.cn-north-4.myhuaweicloud.com/v5/caller-identity";
const BASE: &str = "https://snap-access.cn-north-4.myhuaweicloud.com";

pub(super) struct Login {
    pub(super) callback: String,
    verifier: Zeroizing<String>,
    key: SigningKey,
    ticket: String,
}
pub(super) struct Material {
    secret: Value,
    identity: String,
    public: Value,
}
impl Material {
    pub(super) fn account(&self) -> Value { json!({"identity":self.identity,"secret":self.secret,"public":self.public,"desktop":false}) }
}

pub(super) fn nonce() -> Result<String,proto::WireError> {
    let mut random = [0u8;32]; getrandom::getrandom(&mut random).map_err(|_| invalid())?;
    Ok(URL_SAFE_NO_PAD.encode(random))
}
impl Login {
    pub(super) fn new(port: u16) -> Result<Self,proto::WireError> {
        let verifier = Zeroizing::new(nonce()?);
        let mut bytes = [0u8;32]; getrandom::getrandom(&mut bytes).map_err(|_| invalid())?;
        let key = SigningKey::from_slice(&bytes).map_err(|_| invalid())?;
        use zeroize::Zeroize; bytes.zeroize();
        Ok(Self {callback:format!("http://127.0.0.1:{port}/oauth/callback"),verifier,key,ticket:nonce()?})
    }
    fn context(&self) -> Value {
        let point = self.key.verifying_key().to_encoded_point(false);
        let public = json!({"kty":"EC","crv":"P-256","x":URL_SAFE_NO_PAD.encode(point.x().map(|x| x.as_slice()).unwrap_or(&[])),
            "y":URL_SAFE_NO_PAD.encode(point.y().map(|y| y.as_slice()).unwrap_or(&[]))});
        let mut private = public.clone(); private["d"] = json!(URL_SAFE_NO_PAD.encode(self.key.to_bytes()));
        json!({"client_id":"vscode-codebot","pkce_pair":{"code_verifier":self.verifier.as_str(),
            "code_challenge":URL_SAFE_NO_PAD.encode(Sha256::digest(self.verifier.as_bytes())),"code_challenge_method":"SHA-256"},
            "dpop_key_pair":{"private_key_jwk":private,"public_key_jwk":public}})
    }
    pub(super) fn authorization(&self) -> String {
        let context = self.context();
        let port = url::Url::parse(&self.callback).ok().and_then(|url| url.port()).unwrap_or(0);
        let pairs = [ ("theme","2".to_string()),("locale","zh-cn".to_string()),("uri_scheme","vscode-codebot".to_string()),
            ("client_id","vscode-codebot".to_string()),("port",port.to_string()),("code_challenge",context["pkce_pair"]["code_challenge"].as_str().unwrap_or("").to_string()),
            ("code_challenge_method","SHA-256".to_string()),("ticket_id",self.ticket.clone()),("auth_callback_url",self.callback.clone()),
            ("plugin-name","snap_vscode".to_string()),("plugin-version","26.9.101".to_string())];
        let query = pairs.iter().map(|(name,value)|format!("{name}={}",encode(value))).collect::<Vec<_>>().join("&");
        format!("https://codearts.huaweicloud.com/portal/authorize?{query}")
    }
    fn proof(&self) -> Result<String,proto::WireError> {
        let context = self.context();
        let header = json!({"alg":"ES256","typ":"dpop+jwt","jwk":context["dpop_key_pair"]["public_key_jwk"]});
        let payload = json!({"htm":"POST","htu":TOKEN,"iat":crate::server::logging::now_ms()/1000,"jti":nonce()?});
        let input = format!("{}.{}",URL_SAFE_NO_PAD.encode(header.to_string()),URL_SAFE_NO_PAD.encode(payload.to_string()));
        let signature: Signature = self.key.sign_prehash(&Sha256::digest(input.as_bytes())).map_err(|_| invalid())?;
        Ok(format!("{input}.{}",URL_SAFE_NO_PAD.encode(signature.to_bytes())))
    }
    pub(super) async fn exchange(&self,client: &FrozenHttpClient,code: &str) -> Result<Material,proto::WireError> {
        if code.is_empty() || code.len() > 8192 || code.chars().any(char::is_control) { return Err(invalid()); }
        let form = {
            let mut form = url::form_urlencoded::Serializer::new(String::new());
            for (name,value) in [("client_id","vscode-codebot"),("grant_type","authorization_code"),("code",code),("code_verifier",self.verifier.as_str()),("redirect_uri",self.callback.as_str())] {form.append_pair(name,value);}
            form.finish()
        };
        let request = client.client().map_err(|_| invalid())?.post(TOKEN).header("Content-Type","application/x-www-form-urlencoded")
            .header("Accept","application/json").header("DPoP",self.proof()?).body(form).build().map_err(|_| invalid())?;
        let (_,value) = send(client,request).await?;
        let c = value.get("credentials").unwrap_or(&value);
        let mut secret = json!({"access_key_id":required(c,"access_key_id")?,"secret_access_key":required(c,"secret_access_key")?,
            "security_token":required(c,"security_token")?,"expires_at":required(c,"expiration")?,
            "refresh_token":value.get("refresh_token").or_else(||c.get("refresh_token")).and_then(Value::as_str).filter(|token| !token.is_empty()).ok_or_else(invalid)?,
            "oauth_context":self.context(),"login_type":"WEB"});
        let identity = fetch_identity(client,&secret).await?;
        secret["domain_id"] = json!(required(&identity,"account_id")?);
        secret["user_id"] = json!(required(&identity,"principal_id")?);
        let name = identity["principal_urn"].as_str().unwrap_or("CodeArts").rsplit(['/',':']).next().unwrap_or("CodeArts");
        secret["user_name"] = json!(name);
        let expires = chrono::DateTime::parse_from_rfc3339(required(&secret,"expires_at")?).map_err(|_| invalid())?.timestamp_millis();
        let refresh_expires = secret["refresh_token"].as_str().and_then(jwt_exp);
        let identity = format!("codearts-{}-{}",required(&secret,"domain_id")?,required(&secret,"user_id")?);
        let public = json!({"name":secret["user_name"],"userId":secret["user_id"],"domainId":secret["domain_id"],"expiresAt":expires,
            "accessTokenExpiresAt":expires,"refreshTokenExpiresAt":refresh_expires,"hasAccessToken":true,"hasRefreshToken":true,"canRefresh":true,"refreshStatus":"ready"});
        Ok(Material {public,secret,identity})
    }
    pub(super) async fn ticket(&self,client: &FrozenHttpClient,secret: &str) -> Result<Option<Material>,proto::WireError> {
        let url = format!("{BASE}/snap-manager/v1/login/ticket?ticket_id={}&secret={}",encode(&self.ticket),encode(secret));
        let request = client.client().map_err(|_| invalid())?.get(url).header("Accept","application/json").header("plugin-name","snap_vscode").header("plugin-version","26.9.101").build().map_err(|_| invalid())?;
        let (status,value) = send_allow_pending(client,request).await?;
        if matches!(status,404|202) { return Ok(None); }
        let c = &value["credential"];
        let secret = json!({"access_key_id":required(c,"access")?,"secret_access_key":required(c,"secret")?,"security_token":required(c,"securitytoken")?,
            "expires_at":required(c,"expires_at")?,"domain_id":required(&value,"domain_id")?,"user_id":required(&value,"user_id")?,
            "user_name":value["user_name"],"login_type":"WEB","refresh_token":""});
        let identity = format!("codearts-{}-{}",required(&secret,"domain_id")?,required(&secret,"user_id")?);
        let expires = chrono::DateTime::parse_from_rfc3339(required(&secret,"expires_at")?).map_err(|_| invalid())?.timestamp_millis();
        let public = json!({"name":secret["user_name"],"userId":secret["user_id"],"domainId":secret["domain_id"],"expiresAt":expires,
            "hasAccessToken":true,"hasRefreshToken":false,"canRefresh":false,"importWarning":"本次使用临时登录兜底，约一小时到期后需要重新登录"});
        Ok(Some(Material {public,secret,identity}))
    }
}

async fn fetch_identity(client: &FrozenHttpClient,secret: &Value) -> Result<Value,proto::WireError> {
    let date = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let headers = [ ("accept","application/json"), ("content-type","application/json"), ("x-sdk-date",date.as_str()), ("x-security-token",required(secret,"security_token")?) ];
    let canonical = headers.iter().map(|(key,value)|format!("{key}:{value}\n")).collect::<String>();
    let names = headers.iter().map(|(key,_)| *key).collect::<Vec<_>>().join(";");
    let request = format!("GET\n/v5/caller-identity/\n\n{canonical}\n{names}\n{:x}",Sha256::digest(b""));
    let input = format!("SDK-HMAC-SHA256\n{date}\n{:x}",Sha256::digest(request));
    let mut mac = Hmac::<Sha256>::new_from_slice(required(secret,"secret_access_key")?.as_bytes()).map_err(|_| invalid())?; mac.update(input.as_bytes());
    let auth = format!("SDK-HMAC-SHA256 Access={}, SignedHeaders={names}, Signature={:x}",required(secret,"access_key_id")?,mac.finalize().into_bytes());
    let mut request = client.client().map_err(|_| invalid())?.get(IDENTITY).header("Authorization",auth);
    for (name,value) in headers {request = request.header(name,value);}
    send(client,request.build().map_err(|_| invalid())?).await.map(|(_,value)|value)
}
async fn send(client: &FrozenHttpClient,request: reqwest::Request) -> Result<(u16,Value),proto::WireError> {
    let (status,value) = send_allow_pending(client,request).await?;
    if !(200..300).contains(&status) || value.get("error").is_some() { return Err(invalid()); }
    Ok((status,value))
}
async fn send_allow_pending(client: &FrozenHttpClient,request: reqwest::Request) -> Result<(u16,Value),proto::WireError> {
    tokio::time::timeout(std::time::Duration::from_secs(30),async {
        let mut response = client.client().map_err(|_| invalid())?.execute(request).await.map_err(|_| invalid())?;
        let status = response.status().as_u16();
        if matches!(status,404|202) { return Ok((status,Value::Null)); }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| invalid())? { if bytes.len()+chunk.len()>128*1024{return Err(invalid());}bytes.extend_from_slice(&chunk); }
        let value = super::super::managed_task::declaration::parse_object(&bytes,128*1024).map_err(|_| invalid())?;
        Ok((status,value))
    }).await.map_err(|_| invalid())?
}
fn required<'a>(value: &'a Value,key: &str) -> Result<&'a str,proto::WireError> { value[key].as_str().filter(|value| !value.is_empty() && value.len()<=8192 && !value.chars().any(char::is_control)).ok_or_else(invalid) }
fn encode(value: &str) -> String { url::form_urlencoded::byte_serialize(value.as_bytes()).collect() }
fn jwt_exp(token: &str) -> Option<i64> {
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    let sig = parts.next()?;
    if payload.is_empty() || sig.is_empty() || parts.next().is_some() || token.len() > 8192 { return None; }
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    claims["exp"].as_i64()?.checked_mul(1000).filter(|at| *at > 0)
}
