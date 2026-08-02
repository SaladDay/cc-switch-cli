use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::cli::tui::app::{SessionPageToken, SessionRowIdentity};
use crate::services::session_cost::{QueryControl, SessionCostIdentity};

use super::types::SessionMsg;

const SESSION_COST_QUERY_DEADLINE: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
pub(crate) struct SessionCostRequest {
    pub(crate) cost_seq: u64,
    pub(crate) page_token: SessionPageToken,
    pub(crate) page_index: usize,
    pub(crate) identities: Vec<SessionCostIdentity>,
}

#[derive(Debug, Clone)]
pub(crate) struct ActiveCostOverlay {
    cost_seq: u64,
    page_token: SessionPageToken,
    page_index: usize,
    identities: Vec<SessionRowIdentity>,
}

impl ActiveCostOverlay {
    pub(crate) fn new(
        cost_seq: u64,
        page_token: SessionPageToken,
        page_index: usize,
        identities: Vec<SessionRowIdentity>,
    ) -> Self {
        Self {
            cost_seq,
            page_token,
            page_index,
            identities,
        }
    }

    pub(crate) fn accepts(
        &self,
        cost_seq: u64,
        page_token: &SessionPageToken,
        page_index: usize,
        identities: &[SessionRowIdentity],
    ) -> bool {
        self.cost_seq == cost_seq
            && self.page_token == *page_token
            && self.page_index == page_index
            && self.identities == identities
    }
}

struct MailboxSlot {
    latest: Option<SessionCostRequest>,
    sender_count: usize,
    receiver_alive: bool,
}

struct Mailbox {
    slot: Mutex<MailboxSlot>,
    ready: Condvar,
    active_cost_seq: Arc<AtomicU64>,
}

enum WorkerStarterState {
    Pending {
        receiver: SessionCostRequestReceiver,
        active_cost_seq: Arc<AtomicU64>,
        result_tx: mpsc::Sender<SessionMsg>,
    },
    Started {
        _handle: std::thread::JoinHandle<()>,
    },
    Failed,
}

struct WorkerStarter {
    state: Mutex<WorkerStarterState>,
}

pub(crate) struct SessionCostRequestSender {
    mailbox: Arc<Mailbox>,
    worker_starter: Option<Arc<WorkerStarter>>,
}

impl Clone for SessionCostRequestSender {
    fn clone(&self) -> Self {
        if let Ok(mut slot) = self.mailbox.slot.lock() {
            slot.sender_count = slot.sender_count.saturating_add(1);
        }
        Self {
            mailbox: Arc::clone(&self.mailbox),
            worker_starter: self.worker_starter.clone(),
        }
    }
}

impl Drop for SessionCostRequestSender {
    fn drop(&mut self) {
        if let Ok(mut slot) = self.mailbox.slot.lock() {
            slot.sender_count = slot.sender_count.saturating_sub(1);
            if slot.sender_count == 0 {
                self.mailbox.ready.notify_all();
            }
        }
    }
}

impl SessionCostRequestSender {
    pub(crate) fn submit(&self, request: SessionCostRequest) -> Result<(), SessionCostRequest> {
        if !self.ensure_worker_started() {
            return Err(request);
        }
        let mut slot = self.mailbox.slot.lock().map_err(|_| request.clone())?;
        if !slot.receiver_alive {
            return Err(request);
        }
        self.mailbox
            .active_cost_seq
            .store(request.cost_seq, Ordering::Release);
        slot.latest = Some(request);
        self.mailbox.ready.notify_one();
        Ok(())
    }

    pub(crate) fn cancel(&self, cost_seq: u64) -> Result<(), u64> {
        let mut slot = self.mailbox.slot.lock().map_err(|_| cost_seq)?;
        if !slot.receiver_alive {
            return Err(cost_seq);
        }
        self.mailbox
            .active_cost_seq
            .store(cost_seq, Ordering::Release);
        slot.latest = None;
        self.mailbox.ready.notify_all();
        Ok(())
    }

    fn ensure_worker_started(&self) -> bool {
        let Some(starter) = &self.worker_starter else {
            return true;
        };
        let Ok(mut state) = starter.state.lock() else {
            return false;
        };
        match &*state {
            WorkerStarterState::Started { .. } => return true,
            WorkerStarterState::Failed => return false,
            WorkerStarterState::Pending { .. } => {}
        }
        let pending = std::mem::replace(&mut *state, WorkerStarterState::Failed);
        let WorkerStarterState::Pending {
            receiver,
            active_cost_seq,
            result_tx,
        } = pending
        else {
            return false;
        };
        match std::thread::Builder::new()
            .name("cc-switch-session-cost".to_string())
            .spawn(move || worker_loop(receiver, active_cost_seq, result_tx))
        {
            Ok(handle) => {
                *state = WorkerStarterState::Started { _handle: handle };
                true
            }
            Err(error) => {
                log::debug!("failed to lazily spawn session cost worker thread: {error}");
                false
            }
        }
    }

    #[cfg(test)]
    pub(super) fn worker_started_for_test(&self) -> bool {
        self.worker_starter.as_ref().is_some_and(|starter| {
            starter
                .state
                .lock()
                .is_ok_and(|state| matches!(*state, WorkerStarterState::Started { .. }))
        })
    }
}

pub(crate) struct SessionCostRequestReceiver {
    mailbox: Arc<Mailbox>,
}

impl SessionCostRequestReceiver {
    pub(crate) fn recv_latest(&self) -> Option<SessionCostRequest> {
        let mut slot = self.mailbox.slot.lock().ok()?;
        loop {
            if let Some(request) = slot.latest.take() {
                return Some(request);
            }
            if slot.sender_count == 0 || !slot.receiver_alive {
                return None;
            }
            slot = self.mailbox.ready.wait(slot).ok()?;
        }
    }
}

impl Drop for SessionCostRequestReceiver {
    fn drop(&mut self) {
        if let Ok(mut slot) = self.mailbox.slot.lock() {
            slot.receiver_alive = false;
            slot.latest = None;
            self.mailbox.ready.notify_all();
        }
    }
}

pub(crate) fn session_cost_mailbox(
    active_cost_seq: Arc<AtomicU64>,
) -> (SessionCostRequestSender, SessionCostRequestReceiver) {
    let mailbox = Arc::new(Mailbox {
        slot: Mutex::new(MailboxSlot {
            latest: None,
            sender_count: 1,
            receiver_alive: true,
        }),
        ready: Condvar::new(),
        active_cost_seq,
    });
    (
        SessionCostRequestSender {
            mailbox: Arc::clone(&mailbox),
            worker_starter: None,
        },
        SessionCostRequestReceiver { mailbox },
    )
}

/// Build the Cost mailbox without creating a thread. The first actual
/// projection request starts the worker; cancellation alone remains a cheap
/// atomic/mailbox update.
pub(crate) fn lazy_session_cost_sender(
    active_cost_seq: Arc<AtomicU64>,
    result_tx: mpsc::Sender<SessionMsg>,
) -> SessionCostRequestSender {
    let (mut sender, receiver) = session_cost_mailbox(Arc::clone(&active_cost_seq));
    sender.worker_starter = Some(Arc::new(WorkerStarter {
        state: Mutex::new(WorkerStarterState::Pending {
            receiver,
            active_cost_seq,
            result_tx,
        }),
    }));
    sender
}

pub(crate) fn worker_loop(
    receiver: SessionCostRequestReceiver,
    active_cost_seq: Arc<AtomicU64>,
    result_tx: mpsc::Sender<SessionMsg>,
) {
    while let Some(request) = receiver.recv_latest() {
        let control = QueryControl {
            active_cost_seq: Arc::clone(&active_cost_seq),
            cost_seq: request.cost_seq,
            deadline: Instant::now() + SESSION_COST_QUERY_DEADLINE,
        };
        if !execute_request(request, control, &result_tx) {
            break;
        }
    }
}

fn execute_request(
    request: SessionCostRequest,
    control: QueryControl,
    result_tx: &mpsc::Sender<SessionMsg>,
) -> bool {
    let overlays = crate::services::session_cost::project_page(&request.identities, &control);
    // A replacement owns the visible page and makes this result stale.
    // A deadline, however, is a current-request failure: publish the empty
    // projection so an accepted overlay clears previously displayed data
    // to `-` instead of leaving a stale estimate on screen.
    if control.is_superseded() {
        return true;
    }
    let identities = request
        .identities
        .iter()
        .map(|identity| SessionRowIdentity {
            provider_id: identity.provider_id.clone(),
            session_id: identity.session_id.clone(),
            source_path: identity.source_path.clone(),
        })
        .collect();
    result_tx
        .send(SessionMsg::CostOverlayReady {
            cost_seq: request.cost_seq,
            page_token: request.page_token,
            page_index: request.page_index,
            identities,
            overlays,
        })
        .is_ok()
}

#[cfg(test)]
mod tests;
