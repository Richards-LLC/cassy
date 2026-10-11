//! The factory director's two-second refresh reads, off the loop thread
//! (cas-ee9ab).
//!
//! Measured on a 1.6 GB store at N=16 (cas-98b24): 13.9% of daemon passes
//! took 100 ms or more, on the refresh cadence. Each refresh tick read every
//! task, agent and recent event on the loop thread (the full director
//! snapshot), and the delivery stage read the same snapshot again before it
//! revalidated events. Profiling the passes by phase put almost all of the
//! refresh step's time in those two reads.
//!
//! They now run on one background thread with its own SQLite connection, so
//! they hold no in-process connection mutex the loop's own store calls need.
//! The loop starts a read when the refresh is due, keeps passing, and applies
//! the finished read on a later pass. Panels are stale by at most one refresh.
//! Delivery keeps its fresh-read-after-detection order: events detected from
//! one read are revalidated against a second read started after detection.

use std::path::PathBuf;
use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;

use crate::ui::factory::app::{
    DeliveryInputs, DeliveryRequest, DirectorRefreshLoad, DirectorRefreshRequest, FactoryApp,
};
use crate::ui::factory::director::{DirectorEvent, DirectorStores};

type ReadJob = Box<dyn FnOnce(Option<&DirectorStores>) + Send>;

/// One background thread for refresh reads. It opens its own SQLite
/// connection on the first read and keeps it.
struct RefreshReader {
    sender: Option<mpsc::Sender<ReadJob>>,
}

impl RefreshReader {
    fn start(cas_dir: PathBuf, project_id: Option<String>) -> Self {
        let (sender, receiver) = mpsc::channel::<ReadJob>();
        let spawned = std::thread::Builder::new()
            .name("factory-refresh-reader".into())
            .spawn(move || {
                let mut stores: Option<DirectorStores> = None;
                let mut opened = false;
                for job in receiver {
                    if !opened {
                        opened = true;
                        stores = DirectorStores::open_dedicated(&cas_dir)
                            .map(|stores| stores.with_project_id(project_id.clone()))
                            .map_err(|error| {
                                tracing::warn!(
                                    %error,
                                    "refresh reader could not open its own connection; \
                                     reading through the shared one"
                                );
                            })
                            .ok();
                    }
                    let stores = stores.as_ref();
                    if let Err(panic) =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(stores)))
                    {
                        tracing::error!(?panic, "factory refresh read panicked");
                    }
                }
            });
        match spawned {
            Ok(_) => Self {
                sender: Some(sender),
            },
            Err(error) => {
                tracing::error!(%error, "could not start the factory refresh reader");
                Self { sender: None }
            }
        }
    }

    /// Queue `read`. `None` when the reader is gone.
    fn submit<T: Send + 'static>(
        &self,
        read: impl FnOnce(Option<&DirectorStores>) -> T + Send + 'static,
    ) -> Option<Pending<T>> {
        let (result, receiver) = mpsc::sync_channel(1);
        self.sender
            .as_ref()?
            .send(Box::new(move |stores| {
                let _ = result.send(read(stores));
            }))
            .ok()?;
        Some(Pending {
            receiver,
            ready: None,
        })
    }
}

/// A read in flight.
struct Pending<T> {
    receiver: mpsc::Receiver<T>,
    ready: Option<T>,
}

/// Whether `slot`'s read has finished. A read that died (its job panicked)
/// clears the slot so the next one can start.
fn finished<T>(slot: &mut Option<Pending<T>>) -> bool {
    let Some(pending) = slot.as_mut() else {
        return false;
    };
    if pending.ready.is_some() {
        return true;
    }
    match pending.receiver.try_recv() {
        Ok(value) => {
            pending.ready = Some(value);
            true
        }
        Err(TryRecvError::Empty) => false,
        Err(TryRecvError::Disconnected) => {
            *slot = None;
            false
        }
    }
}

fn take_finished<T>(slot: &mut Option<Pending<T>>) -> Option<T> {
    if finished(slot) {
        slot.take().and_then(|pending| pending.ready)
    } else {
        None
    }
}

/// Background refresh reads for one daemon loop.
pub(crate) struct DirectorRefresh {
    reader: Option<RefreshReader>,
    director: Option<Pending<DirectorRefreshLoad>>,
    delivery: Option<Pending<(Vec<DirectorEvent>, DeliveryInputs)>>,
    /// Test hook: sleep this long inside every background read.
    read_delay: Duration,
}

impl DirectorRefresh {
    pub(crate) fn new() -> Self {
        Self {
            reader: None,
            director: None,
            delivery: None,
            read_delay: Duration::ZERO,
        }
    }

    /// Model a slow store: every read sleeps `delay` first.
    #[cfg(test)]
    pub(crate) fn with_read_delay(mut self, delay: Duration) -> Self {
        self.read_delay = delay;
        self
    }

    fn reader(&mut self, app: &FactoryApp) -> &RefreshReader {
        self.reader.get_or_insert_with(|| {
            RefreshReader::start(app.cas_dir().to_path_buf(), app.project_scope())
        })
    }

    /// Whether a finished read is waiting to be applied.
    pub(crate) fn has_finished(&mut self) -> bool {
        // Both are polled: a dead read must free its slot either way.
        let director = finished(&mut self.director);
        let delivery = finished(&mut self.delivery);
        director || delivery
    }

    /// Start a director read unless a read is already in flight. A delivery
    /// read in flight also defers it, so its events are delivered before the
    /// next detection.
    pub(crate) fn start_director_read(&mut self, app: &FactoryApp) {
        if self.director.is_some() || self.delivery.is_some() {
            return;
        }
        let request: DirectorRefreshRequest = app.director_refresh_request();
        let delay = self.read_delay;
        self.director = self.reader(app).submit(move |stores| {
            if !delay.is_zero() {
                std::thread::sleep(delay);
            }
            request.read(stores)
        });
    }

    pub(crate) fn take_director_read(&mut self) -> Option<DirectorRefreshLoad> {
        take_finished(&mut self.director)
    }

    /// Start the delivery read for `events`. Hands the events back when no
    /// read can start, so the caller can deliver them another way.
    pub(crate) fn start_delivery_read(
        &mut self,
        app: &FactoryApp,
        events: Vec<DirectorEvent>,
    ) -> Result<(), Vec<DirectorEvent>> {
        if self.delivery.is_some() {
            return Err(events);
        }
        let request: DeliveryRequest = app.delivery_request();
        let delay = self.read_delay;
        // The events ride along with the read: if the reader is gone the
        // closure (and they) are dropped, so keep a copy to hand back.
        let fallback = events.clone();
        match self.reader(app).submit(move |stores| {
            if !delay.is_zero() {
                std::thread::sleep(delay);
            }
            (events, request.read(stores))
        }) {
            Some(pending) => {
                self.delivery = Some(pending);
                Ok(())
            }
            None => Err(fallback),
        }
    }

    pub(crate) fn take_delivery_read(&mut self) -> Option<(Vec<DirectorEvent>, DeliveryInputs)> {
        take_finished(&mut self.delivery)
    }
}

#[cfg(test)]
#[path = "director_refresh_tests.rs"]
mod tests;
