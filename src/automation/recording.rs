//! Observe the stock recorder and control its existing per-session switch.
use super::{control::Permit, displays, error::{BridgeError, Result}, sessions};
use hbb_common::tokio;
use scrap::record::RecordState;
use serde::Serialize;
use serde_json::{json, Value};
use std::{collections::{BTreeMap, VecDeque}, sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}, mpsc}, time::Duration};

pub type Shared = Arc<Mutex<State>>;
#[derive(Default)]
pub struct State {
    pub enabled: bool,
    pub generation: u64,
    epoch: u64,
    next: u64,
    writers: BTreeMap<u64,(u64,bool)>,
    files: VecDeque<File>,
    error: Option<String>,
}
#[derive(Serialize)]
struct File {
    id: u64,
    generation: u64,
    display_id: String,
    path: String,
    state: &'static str,
    frames: u64,
    bytes: Option<u64>,
    error: Option<String>,
}
impl State {
    pub fn requested(&mut self, enabled: bool, epoch: u64) -> u64 {
        if epoch<self.epoch {return self.generation;}
        if enabled && (!self.enabled || self.epoch!=epoch) { self.generation+=1; self.error=None; }
        self.enabled=enabled;
        self.epoch=epoch;
        self.generation
    }
    pub fn disconnected(&mut self, epoch: u64) {
        if self.epoch==epoch { self.enabled=false; }
    }
    pub fn fail(&mut self,error: String) {self.error=Some(error.chars().take(1024).collect());}
    fn active(&self) -> usize { self.writers.values().filter(|(_,active)|*active).count() }
    fn value(&self) -> Value {
        json!({"enabled":self.enabled,"generation":self.generation.to_string(),"connection_epoch":self.epoch.to_string(),
            "active_writers":self.active(),"files":self.files,"error":self.error,
            "status":if self.error.is_some(){"error"}else if self.enabled {if self.files.iter().any(|f|f.generation==self.generation && f.frames>0 && f.state=="writing") {"recording"}else{"waiting_for_frame"}}else if self.active()>0 {"stopping"}else{"stopped"}})
    }
}

pub struct Observer {
    store: Shared,
    writer: u64,
    generation: u64,
    display: usize,
    file: Option<u64>,
    receiver: mpsc::Receiver<RecordState>,
}
impl Observer {
    pub fn new(store: Shared, generation: u64, display: usize) -> (Self,mpsc::Sender<RecordState>) {
        let (sender,receiver)=mpsc::channel();
        let writer={let mut s=store.lock().unwrap();s.next+=1;let id=s.next;s.writers.insert(id,(generation,true));id};
        (Self {store,writer,generation,display,file:None,receiver},sender)
    }
    pub fn drain(&mut self) {
        while let Ok(event)=self.receiver.try_recv() {
            let mut s=self.store.lock().unwrap();
            match event {
                RecordState::NewFile(path)=>{
                    s.next+=1;let id=s.next;self.file=Some(id);
                    s.files.push_back(File {id,generation:self.generation,display_id:self.display.to_string(),path,state:"writing",frames:0,bytes:None,error:None});
                    while s.files.len()>64 {s.files.pop_front();}
                }
                RecordState::NewFrame=>{
                    if let Some(file)=s.files.iter_mut().find(|f|Some(f.id)==self.file) {file.frames+=1;}
                }
                RecordState::Error(error)=>{
                    if self.generation==s.generation {s.error=Some(error.clone());}
                    if let Some(file)=s.files.iter_mut().find(|f|Some(f.id)==self.file) {file.error=Some(error);file.state="failed";}
                }
                RecordState::WriteTail=>{
                    if let Some(file)=s.files.iter_mut().find(|f|Some(f.id)==self.file) {
                        match std::fs::metadata(&file.path) {
                            Ok(metadata)=>{file.bytes=Some(metadata.len());if file.error.is_none(){file.state="finalized";}},
                            Err(error)=>{file.error=Some(error.to_string());file.state="failed";},
                        }
                    }
                }
                RecordState::RemoveFile=>{
                    if let Some(file)=s.files.iter_mut().find(|f|Some(f.id)==self.file) {
                        if std::path::Path::new(&file.path).exists() {file.state="failed";file.error=Some("Stock recorder could not discard its short/empty output".into());}
                        else {file.bytes=Some(0);file.state="discarded";}
                    }
                }
            }
        }
    }
    pub fn error(&mut self, error: &str) {
        // Until the requested keyframe arrives, stock recording may reject delta frames.
        if error=="first frame is not key frame" {return;}
        self.drain();
        let mut s=self.store.lock().unwrap();
        let error=error.chars().take(1024).collect::<String>();
        if self.generation==s.generation {s.error=Some(error.clone());}
        if let Some(file)=s.files.iter_mut().find(|f|Some(f.id)==self.file) {file.error=Some(error);file.state="failed";}
    }
}
impl Drop for Observer {
    fn drop(&mut self) {
        self.drain();
        self.store.lock().unwrap().writers.remove(&self.writer);
    }
}

#[derive(Clone)]
pub struct Envelope {
    pub permit: Permit,
    pub enabled: bool,
    pub active: Arc<AtomicBool>,
    pub reply: Arc<Mutex<Option<tokio::sync::oneshot::Sender<Result<u64>>>>>,
}
impl Envelope {
    pub fn check(&self) -> Result<()> {
        if !self.active.load(Ordering::Acquire) {return Err(BridgeError::new("CANCELLED","Recording request cancelled before applying"));}
        check(&self.permit,self.enabled).map(|_|())
    }
    pub fn complete(&self, value: Result<u64>) {
        if let Some(reply)=self.reply.lock().unwrap().take() {let _cancelled=reply.send(value);}
    }
}
fn check(permit: &Permit, enabled: bool) -> Result<sessions::SessionSnapshot> {
    let s=displays::check(permit,true)?;
    if enabled && s.permissions.get("recording")!=Some(&true) {return Err(BridgeError::new("PERMISSION_DENIED","Remote recording permission is not granted"));}
    Ok(s)
}
pub fn get(permit: &Permit) -> Result<Value> {
    let s=displays::check(permit,false)?;
    let session=sessions::get(&s.session_id).ok_or_else(||BridgeError::new("SESSION_CLOSED","Desktop closed"))?;
    let state=session.recording().lock().unwrap().value();
    Ok(json!({"scope":"session","storage":"controller_files","connected":s.authenticated,
        "permission":s.permissions.get("recording"),"state":state,"output_directory":crate::ui_interface::video_save_directory(false),
        "limits":"Last 64 file records in memory; files remain on disk. Stock recorder discards recordings shorter than one second or without video. Codec/resolution changes may create multiple files. No audio track is recorded.",
        "lifecycle":"Recording continues after control handover, until explicitly stopped, permission revoked, or the desktop connection ends. The local and remote GUIs receive normal recording status."}))
}
pub async fn set(permit: Permit, enabled: bool, wait_ms: u64) -> Result<Value> {
    super::api::wait_budget(wait_ms)?;
    let s=check(&permit,enabled)?;
    let core=sessions::core(&s.session_id).ok_or_else(||BridgeError::new("GUI_UNAVAILABLE","Desktop unavailable"))?;
    let (tx,rx)=tokio::sync::oneshot::channel();
    let active=Arc::new(AtomicBool::new(true));
    struct Cancel(Arc<AtomicBool>);
    impl Drop for Cancel {fn drop(&mut self){self.0.store(false,Ordering::Release);}}
    let _cancel=Cancel(active.clone());
    let envelope=Envelope {permit:permit.clone(),enabled,active,reply:Arc::new(Mutex::new(Some(tx)))};
    let sender=core.sender.read().unwrap().clone().ok_or_else(||BridgeError::new("NOT_READY","Desktop IO unavailable"))?;
    sender.send(crate::client::Data::AutomationRecord(envelope)).map_err(|_|BridgeError::new("NOT_READY","Desktop IO closed"))?;
    let generation=tokio::time::timeout(Duration::from_secs(2),rx).await
        .map_err(|_|BridgeError::new("DELIVERY_UNKNOWN","Recording request acknowledgement timed out"))?
        .map_err(|_|BridgeError::new("DELIVERY_UNKNOWN","Recording request acknowledgement channel closed"))??;
    let deadline=tokio::time::Instant::now()+Duration::from_millis(wait_ms);
    loop {
        permit.check()?;
        let value=get(&permit)?;let state=&value["state"];
        let same=state["generation"]==generation.to_string();
        let wrote=state["files"].as_array().is_some_and(|files|files.iter().any(|f|f["generation"]==generation && f["frames"].as_u64().unwrap_or(0)>0 && f["state"]=="writing"));
        let confirmed=same && state["enabled"]==enabled && if enabled {wrote}else{state["active_writers"]==0};
        if confirmed || !same || !state["error"].is_null() || tokio::time::Instant::now()>=deadline {
            return Ok(json!({"delivery":"applied","confirmed":confirmed,"scope":"session","requested_enabled":enabled,"state":value}));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stop_waits_for_writer_tail_and_preserves_failure() {
        let store=Shared::default();let generation=store.lock().unwrap().requested(true,1);
        let (mut observer,tx)=Observer::new(store.clone(),generation,0);
        let path=std::env::temp_dir().join(format!("mcp-recording-test-{}",uuid::Uuid::new_v4()));
        std::fs::write(&path,b"test bytes").unwrap();
        tx.send(RecordState::NewFile(path.to_string_lossy().into())).unwrap();
        tx.send(RecordState::NewFrame).unwrap();observer.drain();
        store.lock().unwrap().requested(false,1);
        assert_eq!(store.lock().unwrap().value()["status"],"stopping");
        tx.send(RecordState::Error("muxer failed".into())).unwrap();
        tx.send(RecordState::WriteTail).unwrap();drop(observer);
        let value=store.lock().unwrap().value();
        assert_eq!(value["active_writers"],0);
        assert_eq!(value["files"][0]["state"],"failed");
        assert_eq!(value["files"][0]["bytes"],10);
        assert_eq!(value["error"],"muxer failed");
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn short_file_discard_and_old_connection_do_not_claim_current_recording() {
        let store=Shared::default();let generation=store.lock().unwrap().requested(true,1);
        let (mut observer,tx)=Observer::new(store.clone(),generation,1);
        let path=std::env::temp_dir().join(format!("mcp-removed-test-{}",uuid::Uuid::new_v4()));
        tx.send(RecordState::NewFile(path.to_string_lossy().into())).unwrap();
        tx.send(RecordState::RemoveFile).unwrap();observer.drain();
        store.lock().unwrap().requested(false,2);
        let next=store.lock().unwrap().requested(true,2);assert!(next>generation);
        store.lock().unwrap().requested(false,1);
        store.lock().unwrap().disconnected(1);
        observer.error("late old writer failure");drop(observer);
        let value=store.lock().unwrap().value();
        assert_eq!(value["enabled"],true);
        assert_eq!(value["error"],Value::Null);
        assert_eq!(value["files"][0]["state"],"failed");
        assert_eq!(value["status"],"waiting_for_frame");
    }
    #[test]
    fn file_history_is_bounded() {
        let store=Shared::default();let (mut observer,tx)=Observer::new(store.clone(),1,0);
        for n in 0..100 {tx.send(RecordState::NewFile(format!("file-{n}"))).unwrap();observer.drain();}
        assert_eq!(store.lock().unwrap().files.len(),64);
    }
}
