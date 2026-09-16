//! Bounded, in-memory history of stock desktop chat messages.
use super::{control::Permit, displays, error::{BridgeError, Result}, sessions, wire};
use hbb_common::{message_proto::{ChatMessage, Message, Misc}, tokio};
use serde::Serialize;
use serde_json::{json, Value};
use std::{collections::VecDeque, time::Duration};

const MAX_TEXT: usize = 16 * 1024;
const MAX_MESSAGES: usize = 256;
const MAX_BYTES: usize = 256 * 1024;

#[derive(Clone, Serialize)]
pub struct Entry {
    sequence: u64,
    connection_epoch: u64,
    direction: &'static str,
    text: String,
    truncated: bool,
    received_at: chrono::DateTime<chrono::Utc>,
}
pub struct History {
    id: String,
    next: u64,
    bytes: usize,
    entries: VecDeque<Entry>,
}
impl Default for History {
    fn default() -> Self {
        Self { id: uuid::Uuid::new_v4().to_string(), next: 1, bytes: 0, entries: VecDeque::new() }
    }
}
impl History {
    pub fn append(&mut self, epoch: u64, direction: &'static str, text: &str) {
        let mut end = text.len().min(MAX_TEXT);
        while !text.is_char_boundary(end) { end -= 1; }
        self.entries.push_back(Entry { sequence:self.next, connection_epoch:epoch, direction,
            text:text[..end].into(), truncated:end<text.len(), received_at:chrono::Utc::now() });
        self.next += 1;
        self.bytes += end;
        while self.entries.len()>MAX_MESSAGES || self.bytes>MAX_BYTES {
            if let Some(old)=self.entries.pop_front() { self.bytes -= old.text.len(); }
        }
    }
    pub fn read(&self, cursor: Option<&str>, limit: usize) -> Result<Value> {
        if !(1..=100).contains(&limit) { return Err(BridgeError::invalid("max_messages must be 1..100")); }
        let oldest=self.entries.front().map(|m|m.sequence).unwrap_or(self.next);
        let from=match cursor {
            None=>oldest,
            Some(c)=>{
                let (id,seq)=c.rsplit_once(':').ok_or_else(||BridgeError::invalid("Invalid chat cursor"))?;
                let seq=seq.parse::<u64>().map_err(|_|BridgeError::invalid("Invalid chat cursor"))?;
                if id!=self.id || seq==0 || seq>self.next { return Err(BridgeError::new("CURSOR_INVALID","Cursor belongs to another chat history or is beyond its end")); }
                seq
            }
        };
        let entries=self.entries.iter().filter(|m|m.sequence>=from).take(limit).collect::<Vec<_>>();
        let next=entries.last().map(|m|m.sequence+1).unwrap_or(self.next);
        Ok(json!({"messages":entries,"next_cursor":format!("{}:{}",self.id,next),
            "gap":from<oldest,"has_more":next<self.next,"retained_messages":self.entries.len(),
            "scope":"session","storage":"controller_memory","max_retained_messages":MAX_MESSAGES,"max_retained_bytes":MAX_BYTES}))
    }
}
pub fn check(permit: &Permit, write: bool) -> Result<sessions::SessionSnapshot> {
    let s=displays::check(permit,write)?;
    if write {
        permit.check()?;
        if !s.authenticated { return Err(BridgeError::new("NOT_READY","Chat requires an authenticated desktop connection")); }
    }
    Ok(s)
}
pub async fn read(permit: Permit, cursor: Option<String>, limit: usize, wait_ms: u64) -> Result<Value> {
    super::api::wait_budget(wait_ms)?;
    let s=check(&permit,false)?;
    let session=sessions::get(&s.session_id).ok_or_else(||BridgeError::new("SESSION_CLOSED","Desktop closed"))?;
    let deadline=tokio::time::Instant::now()+Duration::from_millis(wait_ms);
    let mut changed=session.subscribe();
    loop {
        let s=check(&permit,false)?;
        let mut value=session.chat_read(cursor.as_deref(),limit)?;
        value["connected"]=json!(s.authenticated);
        value["connection_epoch"]=json!(s.connection_epoch.to_string());
        if value["messages"].as_array().is_some_and(|m|!m.is_empty()) || !s.authenticated || tokio::time::Instant::now()>=deadline { return Ok(value); }
        tokio::select! { _=changed.changed()=>{}, _=tokio::time::sleep(Duration::from_millis(100))=>{} }
    }
}
pub async fn send(permit: Permit, text: String) -> Result<Value> {
    check(&permit,true)?;
    if text.is_empty() || text.len()>MAX_TEXT { return Err(BridgeError::invalid("Chat text must contain 1..16384 UTF-8 bytes")); }
    let mut misc=Misc::new();misc.set_chat_message(ChatMessage {text,..Default::default()});
    let mut message=Message::new();message.set_misc(misc);
    wire::send(permit,message).await?;
    Ok(json!({"delivery":"sent","confirmed":false,"scope":"session","hint":"No peer delivery/read receipt exists. Read the bounded local chat history separately."}))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ordered_messages_and_cursor_resume_across_epochs() {
        let mut h=History::default();h.append(1,"incoming","中文一");
        let first=h.read(None,1).unwrap();
        h.append(2,"outgoing","中文二");
        let second=h.read(first["next_cursor"].as_str(),100).unwrap();
        assert_eq!(second["messages"][0]["text"],"中文二");
        assert_eq!(second["messages"][0]["connection_epoch"],2);
        assert_eq!(second["messages"].as_array().unwrap().len(),1);
        assert!(History::default().read(first["next_cursor"].as_str(),100).is_err());
    }
    #[test]
    fn eviction_reports_gap_and_truncation_preserves_utf8() {
        let mut h=History::default();let cursor=h.read(None,100).unwrap()["next_cursor"].as_str().unwrap().to_owned();
        let text="中".repeat(MAX_TEXT);
        for _ in 0..20 { h.append(1,"incoming",&text); }
        let value=h.read(Some(&cursor),100).unwrap();
        assert_eq!(value["gap"],true);
        assert!(h.bytes<=MAX_BYTES);
        assert_eq!(value["messages"][0]["truncated"],true);
        assert!(value["messages"][0]["text"].as_str().unwrap().ends_with('中'));
    }
}
