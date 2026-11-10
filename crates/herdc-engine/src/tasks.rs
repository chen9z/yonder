//! Tasks belong to a session, never to its internal message senders.
use std::future::Future;
use std::sync::Arc;
use tokio::sync::watch;

pub(crate) struct Session(watch::Sender<bool>);

#[derive(Clone)]
pub(crate) struct Tasks(watch::Receiver<bool>);

impl Session {
    pub(crate) fn new() -> (Arc<Self>, Tasks) {
        let (tx, rx) = watch::channel(false);
        (Arc::new(Self(tx)), Tasks(rx))
    }

    pub(crate) fn tasks(&self) -> Tasks { Tasks(self.0.subscribe()) }

    pub(crate) fn close(&self) {
        self.0.send_replace(true);
    }
}

impl Tasks {
    pub(crate) fn spawn(&self, work: impl Future<Output = ()> + Send + 'static) {
        let mut stop = self.0.clone();
        tokio::spawn(async move {
            if *stop.borrow() { return; }
            tokio::select! {
                biased;
                _ = stop.changed() => {}
                _ = work => {}
            }
        });
    }
}
