mod receipt;
mod local_gate;

use std::{collections::{BTreeMap, BTreeSet}, sync::{Arc, Mutex}, time::Duration};

use agent2api_provider_protocol as proto;
use base64::Engine;
use serde_json::{json, Value};

use super::{checkin_call::doubao_daily::credentials::{self, Credentials}, context::Context};
use crate::server::core::{account_store::{credential_version::{RowProvider, RowSource}, plugin_accounts}, blocking, egress::managed::FrozenHttpClient, plugin_host::script::{self, HttpDeadline, HttpHandle}};

pub(super) struct Turn {
    client: FrozenHttpClient,
    row: Arc<RowSource>,
    credentials: Arc<Credentials>,
    state: Arc<Mutex<receipt::State>>,
    readers: BTreeMap<String, receipt::Reader>,
    finished: bool,
}

impl Context {
    pub(super) async fn doubao_http(&mut self, params: &Value, deadline: HttpDeadline) -> Result<Option<(Value,HttpHandle)>,proto::WireError> {
        if self.shared.options.provider != "doubaowork" {return Ok(None);}
        let url = url::Url::parse(params["url"].as_str().ok_or_else(invalid)?).map_err(|_|invalid())?;
        if url.origin().ascii_serialization() != "https://www.doubao.com" || !matches!(url.path(),"/chat/completion"|"/chat/async/chunk_stream") {return Ok(None);}
        self.permission("http.doubao-turn.v1")?;
        if params["method"] != "POST" {return Err(invalid());}
        let (bytes, pinned_body) = if let Some(resource) = params.get("body_resource").filter(|value| !value.is_null()) {
            if params.get("body").is_some_and(|value| !value.is_null()) { return Err(invalid()); }
            self.permission("resources.v1")?;
            let raw = self.blobs.raw(resource.as_str().ok_or_else(invalid)?)?;
            if raw.len() > 8 * 1024 * 1024 { return Err(invalid()); }
            (raw.clone(), Some(script::RequestBody::Bytes(raw)))
        } else {
            let encoded = params["body"].as_str().filter(|bytes|bytes.len()<=12*1024*1024).ok_or_else(invalid)?;
            let raw = base64::engine::general_purpose::STANDARD.decode(encoded).map_err(|_|invalid())?;
            (bytes::Bytes::from(raw), None)
        };
        let body: Value = super::managed_task::declaration::parse_object(&bytes,8*1024*1024).map_err(|_|invalid())?;
        if url.path() == "/chat/completion" {
            if self.doubao_turn.is_some() {return Err(invalid());}
            let call = self.conversation.as_ref().ok_or_else(invalid)?;
            let path = call.selected_path(&self.shared,self.account()?)?;
            let store = plugin_accounts::shared().ok_or_else(invalid)?;
            let id = self.account()?.to_string();
            let row = Arc::new(blocking::run(move ||store.prepare_row_source(RowProvider::DoubaoWork,&id)).await.map_err(|_|invalid())?.map_err(|_|invalid())?);
            let credentials = Arc::new(credentials::from_original(&row,crate::server::logging::now_ms()).map_err(|_|invalid())?);
            credentials.matches_turn_request(&url, &params["headers"]).map_err(|_| invalid())?;
            let client = FrozenHttpClient::from_path(path).await.map_err(|_|invalid())?;
            call.validate_context(&self.shared,&self.account)?;
            local_gate::check(&row,self.call_deadline).await?;
            let local = body["messages"][0]["local_message_id"].as_str().filter(|value|identifier(value)).ok_or_else(invalid)?.to_string();
            self.guard_http_submission(params)?;
            self.doubao_turn = Some(Turn {client,row,credentials,state:Arc::new(Mutex::new(receipt::State::new(local))),readers:BTreeMap::new(),finished:false});
        } else {
            self.conversation.as_ref().ok_or_else(invalid)?.validate_context(&self.shared,&self.account)?;
            let task = body["task_id"].as_str().filter(|id|identifier(id)).ok_or_else(invalid)?;
            let turn = self.doubao_turn.as_mut().ok_or_else(invalid)?;
            turn.credentials.matches_turn_request(&url, &params["headers"]).map_err(|_| invalid())?;
            {
                let mut state = turn.state.lock().map_err(|_|invalid())?;
                if !state.tasks.contains_key(task) || !state.opened_tasks.insert(task.to_string()) {return Err(invalid());}
            }
            local_gate::check(&turn.row,self.call_deadline).await?;
        }
        let turn = self.doubao_turn.as_mut().ok_or_else(invalid)?;
        let client = turn.client.client().map_err(|_|invalid())?.clone();
        let task = (url.path()=="/chat/async/chunk_stream").then(||body["task_id"].as_str().unwrap_or("").to_string());
        let (result,handle) = script::open_pinned_body(params,pinned_body,client,deadline.cap_at(self.call_deadline)).await?;
        if result["status"] == 200 && result["headers"]["content-type"].as_str().is_some_and(|value| value.starts_with("text/event-stream")) {
            turn.readers.insert(handle.id().to_string(),receipt::Reader::new(task));
        }
        Ok(Some((result,handle)))
    }
}

pub(super) fn observe(turn: &mut Turn,id: &str,value: &Value) -> Result<(),proto::WireError> {
    let Some(reader)=turn.readers.get_mut(id) else {return Ok(());};
    if value["done"]==true {reader.finish()?;return Ok(());}
    let bytes=base64::engine::general_purpose::STANDARD.decode(value["data"].as_str().ok_or_else(invalid)?).map_err(|_|invalid())?;
    let mut state=turn.state.lock().map_err(|_|invalid())?;
    reader.push(&bytes,&mut state)
}

pub(super) async fn invoke(context: &mut Context,method: &str,params: &Value) -> Result<Value,proto::WireError> {
    context.permission("http.doubao-turn.v1")?;
    if context.shared.options.provider!="doubaowork" || params.as_object().is_none_or(|fields|!fields.is_empty()) {return Err(invalid());}
    context.conversation.as_ref().ok_or_else(invalid)?.validate_context(&context.shared,&context.account)?;
    let turn=context.doubao_turn.as_mut().ok_or_else(invalid)?;
    local_gate::check(&turn.row,context.call_deadline).await?;
    let value=turn.state.lock().map_err(|_|invalid())?.view();
    if method=="doubao.turn.finish" {
        if !turn.state.lock().map_err(|_|invalid())?.complete() {return Err(invalid());}
        local_gate::check(&turn.row,context.call_deadline).await?;
        turn.finished=true;
    }
    Ok(value)
}

impl Drop for Turn {
    fn drop(&mut self) {
        if self.finished {return;}
        let client=self.client.clone();let row=self.row.clone();let credentials=self.credentials.clone();
        let state=self.state.clone();
        crate::spawn_task(async move {
            let receipt=match state.lock(){Ok(state)=>state.view(),Err(_)=>return};
            if receipt["conversation_id"].as_str().is_none_or(|id|id.is_empty()) {return;}
            if local_gate::check(&row,tokio::time::Instant::now()+Duration::from_secs(2)).await.is_err() {return;}
            let stop=async {
                let request=client.client().map_err(|_|invalid())?.post(credentials.url("/im/message/break_stream_msg"))
                    .headers(credentials.headers(false).map_err(|_|invalid())?)
                    .json(&json!({"cmd":2240,"uplink_body":{"break_stream_msg_uplink_body":{"reply_msg_id":receipt["question_id"],"message_id":"","conversation_id":receipt["conversation_id"],"conversation_type":receipt["conversation_type"],"break_reason":7}},
                        "sequence_id":crate::server::core::protocol::random_id("stop"),"channel":2,"version":"1"})).build().map_err(|_|invalid())?;
                let _=send(&client,request).await;
                let threads: BTreeSet<_>=receipt["tasks"].as_array().into_iter().flatten().filter_map(|task|task["thread_id"].as_str())
                    .filter(|id|id.bytes().all(|byte|byte.is_ascii_digit()) && id.len()>=12 && id.len()<=24).map(str::to_string).collect();
                for thread in threads {
                    if local_gate::check(&row,tokio::time::Instant::now()+Duration::from_secs(2)).await.is_err(){break;}
                    let request=client.client().map_err(|_|invalid())?.post(credentials.url("/alice/generaltask/terminate"))
                        .headers(credentials.headers(false).map_err(|_|invalid())?).body(format!("{{\"thread_id\":{thread}}}")).build().map_err(|_|invalid())?;
                    let _=send(&client,request).await;
                }
                Ok::<(),proto::WireError>(())
            };
            if tokio::time::timeout(Duration::from_secs(20),stop).await.is_err() {
                crate::server::logging::log("[DoubaoWork]","本次远端停止未在限时内确认，没有重发对话或改用其他账号");
            }
        });
    }
}
async fn send(client:&FrozenHttpClient,request:reqwest::Request)->Result<(),proto::WireError>{
    let response=client.client().map_err(|_|invalid())?.execute(request).await.map_err(|_|invalid())?;
    if !response.status().is_success()||response.headers().contains_key("location"){return Err(invalid());}Ok(())
}
fn identifier(id:&str)->bool{!id.is_empty()&&id.len()<=256&&id.bytes().all(|byte|byte.is_ascii_alphanumeric()||b"_-".contains(&byte))}
fn invalid()->proto::WireError{proto::WireError::with_code("豆包工作原对话或任务回执未确认，未重新提交或停止其他任务","doubaowork_turn_unconfirmed")}
