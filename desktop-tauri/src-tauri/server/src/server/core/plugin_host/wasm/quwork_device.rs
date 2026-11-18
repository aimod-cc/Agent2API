use agent2api_provider_protocol as proto;
use serde_json::{json, Value};

use super::context::Context;

pub(super) const HELPER: &str = "desktop.device.quwork.v1";

pub(super) fn candidate(provider:&str,id:&str)->Option<Value>{
    if provider!="quwork"||id!=HELPER{return None;}
    Some(json!({"pack":"agent2api-host","version":"quwork-device-v1","provider":provider,"id":id,
        "operations":["read"],"digest":"quwork-device-v1"}))
}

pub(super) async fn invoke(context:&Context,params:&Value)->Result<Value,proto::WireError>{
    context.permission("native.helpers.v1")?;
    if context.shared.options.provider!="quwork"||!context.shared.options.manifest.helpers.iter().any(|id|id==HELPER)
        ||params["operation"]!="read"||params["payload"].as_object().is_none_or(|fields|!fields.is_empty()){
        return Err(super::error("QuWork设备资料读取参数无效"));
    }
    super::native::authorized_builtin("quwork",HELPER)?;
    let id=context.account()?;
    let before=context.shared.callbacks.store_get("quwork".into(),id.into()).await?;
    let result=crate::server::core::blocking::run(||{
        let path=super::desktop_credentials::home()?.join(".qurwork-desktop-dsh/.user-device/device-identity.json");
        let value=super::desktop_credentials::object(&path,16384)?;
        let identifier=value["identifier"].as_str().filter(|value|value.len()==64&&value.bytes().all(|byte|byte.is_ascii_hexdigit())).ok_or_else(super::desktop_credentials::invalid)?;
        Ok::<_,proto::WireError>(json!({"deviceIdentifier":identifier}))
    }).await.map_err(|_|super::error("读取QuWork设备资料已中断"))??;
    if context.shared.callbacks.store_get("quwork".into(),id.into()).await?["secret"]!=before["secret"]{return Err(super::error("QuWork账号已变化"));}
    super::native::authorized_builtin("quwork",HELPER)?;
    Ok(result)
}
