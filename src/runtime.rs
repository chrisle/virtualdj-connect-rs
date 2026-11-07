use crate::{Error, Event};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc, Condvar, Mutex,
    },
    time::Duration,
};
pub(crate) const CAPACITY: usize = 256;
pub(crate) struct Control {
    pub running: AtomicBool,
    interval: Mutex<Duration>,
    wake: Condvar,
    pub failure: Mutex<Option<Error>>,
}
impl Control {
    pub fn new(interval: Duration) -> Result<Arc<Self>, Error> {
        if interval.is_zero() {
            return Err(Error("poll interval must be positive".into()));
        }
        Ok(Arc::new(Self {
            running: AtomicBool::new(false),
            interval: Mutex::new(interval),
            wake: Condvar::new(),
            failure: Mutex::new(None),
        }))
    }
    pub fn interval(&self) -> Duration {
        *self.interval.lock().unwrap()
    }
    pub fn set_interval(&self, interval: Duration) -> Result<(), Error> {
        if interval.is_zero() {
            return Err(Error("poll interval must be positive".into()));
        }
        *self.interval.lock().unwrap() = interval;
        self.wake.notify_all();
        Ok(())
    }
    pub fn wait(&self) {
        let interval = self.interval.lock().unwrap();
        let duration = *interval;
        if self.running.load(Ordering::Acquire) {
            drop(self.wake.wait_timeout(interval, duration).unwrap());
        }
    }
    pub fn stop(&self) {
        let _guard = self.interval.lock().unwrap();
        self.running.store(false, Ordering::Release);
        self.wake.notify_all();
    }
    pub fn start(&self) {
        *self.failure.lock().unwrap() = None;
        self.running.store(true, Ordering::Release);
    }
    pub fn emit(&self, sender: &SyncSender<Event>, event: Event) -> bool {
        if sender.try_send(event).is_err() {
            *self.failure.lock().unwrap() = Some(Error(
                "VirtualDJ event queue is full or closed; poller stopped".into(),
            ));
            self.stop();
            false
        } else {
            true
        }
    }
}
pub(crate) fn events() -> (SyncSender<Event>, Receiver<Event>) {
    mpsc::sync_channel(CAPACITY)
}
