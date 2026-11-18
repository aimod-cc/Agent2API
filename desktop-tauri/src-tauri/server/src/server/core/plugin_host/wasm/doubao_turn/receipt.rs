use std::collections::{BTreeMap,BTreeSet};

use agent2api_provider_protocol as proto;
use serde_json::{json,Value};
use super::{identifier,invalid};

pub(super) struct State {
    local: String,
    conversation: Option<String>,
    question: Option<String>,
    conversation_type: Value,
    root_done: bool,
    failed: bool,
    pub(super) tasks: BTreeMap<String,Task>,
    pub(super) opened_tasks: BTreeSet<String>,
}
pub(super) struct Task {thread:String,append_scene:Value,done:bool}

impl State {
    pub(super) fn new(local:String)->Self{Self{local,conversation:None,question:None,conversation_type:Value::Null,root_done:false,failed:false,tasks:BTreeMap::new(),opened_tasks:BTreeSet::new()}}
    pub(super) fn complete(&self)->bool{!self.failed&&self.conversation.is_some()&&self.root_done&&self.tasks.values().all(|task|task.done)}
    pub(super) fn view(&self)->Value{json!({"local_message_id":self.local,"conversation_id":self.conversation,"question_id":self.question,"conversation_type":self.conversation_type,
        "complete":self.complete(),"tasks":self.tasks.iter().map(|(id,task)|json!({"task_id":id,"thread_id":task.thread,"append_scene":task.append_scene,"completed":task.done})).collect::<Vec<_>>()})}
    fn frame(&mut self,event:&str,value:&Value,task:Option<&str>)->Result<(),proto::WireError>{
        if event.contains("ERR")||matches!(event,"error"|"gateway-error")
            || ["error_code", "code"].iter().any(|key| scalar(&value[*key]).is_some_and(|code| code != "0" && !code.is_empty()))
            || value.get("error").is_some_and(|error| !error.is_null() && error != false) {self.failed=true;}
        if event=="SSE_ACK" && task.is_none(){
            let meta=&value["ack_client_meta"];
            if let Some(local)=meta["local_message_id"].as_str(){if local!=self.local{return Err(invalid());}}
            let conversation=scalar(&meta["conversation_id"]).filter(|id|identifier(id)).ok_or_else(invalid)?;
            if self.conversation.as_deref().is_some_and(|old|old!=conversation){return Err(invalid());}
            self.conversation=Some(conversation);
            if !meta["conversation_type"].is_null(){self.conversation_type=meta["conversation_type"].clone();}
            if let Some(question)=scalar(&value["query_list"][0]["question_id"]){
                if !identifier(&question)||self.question.as_deref().is_some_and(|old|old!=question){return Err(invalid());}self.question=Some(question);
            }
        }
        if event=="FETCH_STREAM"&&value["fetch_type"].as_i64()==Some(2){
            let id=scalar(&value["fetch_key"]).filter(|id|identifier(id)).ok_or_else(invalid)?;
            let thread=scalar(&value["thread_id"]).unwrap_or_default();
            if !thread.is_empty()&&!identifier(&thread){return Err(invalid());}
            if let Some(old)=self.tasks.get(&id){if old.thread!=thread||old.append_scene!=value["append_scene"]{return Err(invalid());}}
            else{if self.tasks.len()>=100{return Err(invalid());}self.tasks.insert(id,Task{thread,append_scene:value["append_scene"].clone(),done:false});}
        }
        if event=="SSE_REPLY_END"&&value["end_type"].as_i64()==Some(3){
            if let Some(id)=task{self.tasks.get_mut(id).ok_or_else(invalid)?.done=true;}else{self.root_done=true;}
        }
        Ok(())
    }
}

pub(super) struct Reader {pending:Vec<u8>,task:Option<String>,done:bool,bytes:usize}
impl Reader{
    pub(super) fn new(task:Option<String>)->Self{Self{pending:Vec::new(),task,done:false,bytes:0}}
    pub(super) fn push(&mut self,bytes:&[u8],state:&mut State)->Result<(),proto::WireError>{
        self.bytes=self.bytes.checked_add(bytes.len()).filter(|size|*size<=64*1024*1024).ok_or_else(invalid)?;
        self.pending.extend_from_slice(bytes);
        loop{
            let boundary=[self.pending.windows(2).position(|bytes|bytes==b"\n\n").map(|at|(at,2)),self.pending.windows(4).position(|bytes|bytes==b"\r\n\r\n").map(|at|(at,4))].into_iter().flatten().min_by_key(|(at,_)|*at);
            let Some((at,len))=boundary else{break;};
            if at>2*1024*1024{return Err(invalid());}
            let bytes=self.pending.drain(..at+len).collect::<Vec<_>>();
            let text=std::str::from_utf8(&bytes[..at]).map_err(|_|invalid())?;
            let mut event="";let mut data=Vec::new();
            for line in text.lines(){if let Some(value)=line.strip_prefix("event:"){event=value.trim();}else if let Some(value)=line.strip_prefix("data:"){data.push(value.trim_start());}}
            if data.is_empty(){continue;}
            let value:Value=super::super::managed_task::declaration::parse_object(data.join("\n").as_bytes(),2*1024*1024).map_err(|_|invalid())?;
            state.frame(event,&value,self.task.as_deref())?;
            if event=="SSE_REPLY_END"&&value["end_type"].as_i64()==Some(3){self.done=true;}
        }
        if self.pending.len()>2*1024*1024{return Err(invalid());}Ok(())
    }
    pub(super) fn finish(&self)->Result<(),proto::WireError>{if !self.done||!self.pending.iter().all(u8::is_ascii_whitespace){return Err(invalid());}Ok(())}
}
fn scalar(value:&Value)->Option<String>{value.as_str().map(str::to_string).or_else(||value.as_u64().map(|number|number.to_string()))}
