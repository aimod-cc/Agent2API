use agent2api_provider_protocol as proto;
use base64::Engine;
use image::{codecs::jpeg::JpegEncoder,imageops::FilterType,ImageReader,Limits};
use serde_json::{json,Value};
use std::io::Cursor;

pub(super) async fn prepare(body: &mut Value,options: &super::manifest::Options) -> Result<(),proto::WireError> {
    if options.provider != "catpaw" {return Ok(());}
    let policy = super::managed_task::OrdinaryPolicy::from_options(options)?;
    let mut downloaded = 0usize;
    let messages=body["messages"].as_array_mut().ok_or_else(invalid)?;
    let mut count=0usize;
    let mut total=0usize;
    for message in messages {
        let user = message["role"] == "user";
        let Some(parts)=message["content"].as_array_mut() else {continue;};
        for part in parts {
            if !matches!(part["type"].as_str(),Some("image_url"|"input_image"|"image")){continue;}
            if !user {return Err(invalid());}
            count+=1;if count>10{return Err(invalid());}
            let raw=if part["type"]=="image"&&part["source"]["type"]=="base64" {
                format!("data:{};base64,{}",part["source"]["media_type"].as_str().ok_or_else(invalid)?,part["source"]["data"].as_str().ok_or_else(invalid)?)
            } else {part["image_url"].as_str().or_else(||part["image_url"]["url"].as_str()).or_else(||part["image_url"].is_null().then(||part["url"].as_str()).flatten()).ok_or_else(invalid)?.to_string()};
            total+=raw.len();if total>32*1024*1024{return Err(invalid());}
            let detail=part["image_url"]["detail"].as_str().filter(|detail|matches!(*detail,"auto"|"low"|"high")).unwrap_or("auto").to_string();
            let raw=if raw.starts_with("https://") {
                let remaining = (32usize * 1024 * 1024).saturating_sub(downloaded);
                let image=super::resource_fetch::download(&raw,remaining.min(6*1024*1024),&mut downloaded,policy).await?;
                format!("data:{};base64,{}",image.mime,base64::engine::general_purpose::STANDARD.encode(image.bytes))
            } else { raw };
            let image=crate::server::core::blocking::run(move||normalize(&raw)).await.map_err(|_|invalid())??;
            *part=json!({"type":"image_url","image_url":{"url":image,"detail":detail}});
        }
    }
    Ok(())
}
fn normalize(raw: &str)->Result<String,proto::WireError>{
    let (header,encoded)=raw.split_once(',').ok_or_else(invalid)?;
    if !matches!(header,"data:image/png;base64"|"data:image/jpeg;base64"|"data:image/jpg;base64"|"data:image/webp;base64")||encoded.len()>8*1024*1024{return Err(invalid());}
    let bytes=base64::engine::general_purpose::STANDARD.decode(encoded).map_err(|_|invalid())?;
    let mut reader=ImageReader::new(Cursor::new(&bytes)).with_guessed_format().map_err(|_|invalid())?;
    let mut limits=Limits::default();limits.max_image_width=Some(8192);limits.max_image_height=Some(8192);limits.max_alloc=Some(128*1024*1024);reader.limits(limits);
    let image=reader.decode().map_err(|_|invalid())?;
    if encoded.len()<60*1024{return Ok(raw.to_string());}
    let mut dimension=1568u32;let mut quality=80u8;let mut best=Vec::new();
    for attempt in 0..8 {
        let image=if image.width()>dimension||image.height()>dimension{image.resize(dimension,dimension,FilterType::Lanczos3)}else{image.clone()};
        let rgba=image.to_rgba8();
        let rgb=image::RgbImage::from_fn(rgba.width(),rgba.height(),|x,y|{let p=rgba.get_pixel(x,y);let alpha=p[3] as u16;image::Rgb([
            ((p[0] as u16*alpha+255*(255-alpha))/255) as u8,((p[1] as u16*alpha+255*(255-alpha))/255) as u8,((p[2] as u16*alpha+255*(255-alpha))/255) as u8])});
        let mut output=Vec::new();JpegEncoder::new_with_quality(&mut output,quality).encode_image(&rgb).map_err(|_|invalid())?;
        best=output;if best.len()<=120*1024{break;}
        quality=quality.saturating_sub(15).max(55);if attempt>=1{dimension=(dimension*3/4).max(448);}
    }
    if best.len()>120*1024{return Err(invalid());}
    if best.len()>=bytes.len() && encoded.len()<=120*1024{return Ok(raw.to_string());}
    Ok(format!("data:image/jpeg;base64,{}",base64::engine::general_purpose::STANDARD.encode(best)))
}
fn invalid()->proto::WireError{proto::WireError::with_code("CatPaw 图片需为有效 PNG、JPEG 或 WebP 内联数据或 HTTPS 公网图片，数量、尺寸或大小超过限制","unsupported_parameter")}
