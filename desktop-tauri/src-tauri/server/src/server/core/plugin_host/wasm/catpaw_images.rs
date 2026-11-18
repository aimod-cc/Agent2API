use agent2api_provider_protocol as proto;
use serde_json::{json,Value};

const MAX_URL: usize = 8 * 1024 * 1024;

pub(super) fn source(value: &Value) -> Result<Value,proto::WireError> {
    let object=value.as_object().ok_or_else(invalid)?;
    if object.keys().any(|key|!matches!(key.as_str(),"type"|"image_url"|"imageUrl")) || value["type"]!="image_url" {return Err(invalid());}
    let source=value.get("image_url").or_else(||value.get("imageUrl")).ok_or_else(invalid)?;
    if value.get("image_url").is_some()&&value.get("imageUrl").is_some(){return Err(invalid());}
    let url=source.as_str().or_else(||source["url"].as_str()).filter(|text|valid(text)).ok_or_else(invalid)?;
    if let Some(fields)=source.as_object(){if fields.keys().any(|key|!matches!(key.as_str(),"url"|"detail")){return Err(invalid());}}
    let detail=match source.get("detail") {None=>"auto",Some(Value::String(value))if matches!(value.as_str(),"auto"|"low"|"high")=>value,_=>return Err(invalid())};
    Ok(json!({"type":"image_url","imageUrl":{"url":url,"detail":detail}}))
}
pub(super) fn projected(value:&Value)->Result<(),proto::WireError>{
    if value.get("image_url").is_some()||source(value)?!=*value{return Err(invalid());}Ok(())
}
fn valid(url:&str)->bool{
    if url.is_empty()||url.len()>MAX_URL||url.chars().any(char::is_control){return false;}
    if let Some((prefix,body))=url.split_once(','){
        return matches!(prefix,"data:image/png;base64"|"data:image/jpeg;base64"|"data:image/jpg;base64"|"data:image/webp;base64")&&!body.is_empty()
            &&body.bytes().all(|byte|byte.is_ascii_alphanumeric()||b"+/=".contains(&byte));
    }
    url::Url::parse(url).ok().is_some_and(|url|url.scheme()=="https"&&url.port_or_known_default()==Some(443)&&url.username().is_empty()&&url.password().is_none()&&url.fragment().is_none())
}
fn invalid()->proto::WireError{proto::WireError::with_code("CatPaw 图片块与本次主程序准备结果不一致，未忽略图片或发出替代正文","catpaw_image_projection_invalid")}
