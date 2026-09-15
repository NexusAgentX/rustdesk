use crate::client::{MediaData, MediaSender};
use crossbeam_queue::ArrayQueue;
use hbb_common::message_proto::VideoFrame;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, RwLock,
};

#[derive(Clone)]
pub(crate) struct DecoderLayout {
    applied: Arc<AtomicU64>,
    scheduled: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_barrier_follows_old_keyframes_and_clears_delta_queue() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let queue = RwLock::new(ArrayQueue::new(2));
        assert!(queue.read().unwrap().push(VideoFrame::new()).is_ok());
        sender
            .send(MediaData::VideoFrame(Box::new(VideoFrame::new())))
            .unwrap();
        let mut layout = DecoderLayout::new(1);
        let callback = layout.clone();
        assert!(layout.schedule(2, &sender, &queue).unwrap());
        assert_eq!(callback.revision(), 1);
        assert!(queue.read().unwrap().is_empty());
        assert!(matches!(receiver.recv().unwrap(), MediaData::VideoFrame(_)));
        let MediaData::AutomationLayout(applied, revision) = receiver.recv().unwrap() else {
            panic!("missing layout barrier")
        };
        applied.store(revision, Ordering::Release);
        assert_eq!(callback.revision(), 2);
        assert!(!layout.schedule(2, &sender, &queue).unwrap());
        drop(receiver);
        assert!(layout.schedule(3, &sender, &queue).is_err());
        assert_eq!(callback.revision(), 2);
    }
}

impl DecoderLayout {
    pub(crate) fn new(revision: u64) -> Self {
        Self {
            applied: Arc::new(AtomicU64::new(revision)),
            scheduled: revision,
        }
    }

    pub(crate) fn revision(&self) -> u64 {
        self.applied.load(Ordering::Acquire)
    }

    pub(crate) fn schedule(
        &mut self,
        revision: u64,
        sender: &MediaSender,
        queue: &RwLock<ArrayQueue<VideoFrame>>,
    ) -> Result<bool, &'static str> {
        if revision == self.scheduled {
            return Ok(false);
        }
        // VideoQueue notifications and their frames use different queues. Remove old
        // delta frames before the barrier; old notifications may consume new frames
        // early, but those callbacks still carry the old revision and are rejected.
        let queue = queue.write().unwrap();
        while queue.pop().is_some() {}
        sender
            .send(MediaData::AutomationLayout(self.applied.clone(), revision))
            .map_err(|_| "video decoder closed before layout barrier")?;
        self.scheduled = revision;
        Ok(true)
    }
}
