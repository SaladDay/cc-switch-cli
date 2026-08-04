use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::cli::tui::app::{SessionPageSource, SessionPageToken, SessionRowIdentity};
use crate::cli::tui::runtime_systems::types::SessionMsg;
use crate::services::session_cost::{QueryControl, SessionCostIdentity};
use crate::session_manager::SessionMeta;

use super::{
    execute_request, lazy_session_cost_sender, session_cost_mailbox, ActiveCostOverlay,
    SessionCostRequest, SessionCostRequestSender,
};

fn token(generation: &str, view_epoch: u64) -> SessionPageToken {
    SessionPageToken {
        scope_epoch: 7,
        view_epoch,
        source: SessionPageSource::Base,
        scope: "codex".to_string(),
        generation: generation.to_string(),
    }
}

fn row(session_id: &str, source_path: &str) -> SessionMeta {
    SessionMeta {
        provider_id: "codex".to_string(),
        session_id: session_id.to_string(),
        source_path: Some(source_path.to_string()),
        created_at: Some(2_000_000),
        source_mtime_ns: Some(20),
        ..SessionMeta::default()
    }
}

fn request(seq: u64, generation: &str, page: usize, session_id: &str) -> SessionCostRequest {
    let row = row(session_id, &format!("/{session_id}.jsonl"));
    SessionCostRequest {
        cost_seq: seq,
        page_token: token(generation, 3),
        page_index: page,
        identities: vec![SessionCostIdentity::from(&row)],
    }
}

#[test]
fn one_slot_mailbox_is_latest_wins_before_execution() {
    let active = Arc::new(AtomicU64::new(0));
    let (sender, receiver) = session_cost_mailbox(Arc::clone(&active));
    sender
        .submit(request(1, "old", 0, "old"))
        .expect("submit old");
    sender
        .submit(request(2, "newer", 1, "newer"))
        .expect("submit newer");
    sender
        .submit(request(3, "newest", 2, "newest"))
        .expect("submit newest");

    let received = receiver.recv_latest().expect("latest request");
    assert_eq!(received.cost_seq, 3);
    assert_eq!(received.page_token.generation, "newest");
    assert_eq!(received.page_index, 2);
    assert_eq!(active.load(Ordering::Acquire), 3);
}

#[test]
fn submitting_a_new_request_immediately_cancels_the_running_query_control() {
    let active = Arc::new(AtomicU64::new(1));
    let (sender, _receiver) = session_cost_mailbox(Arc::clone(&active));
    let old_control = QueryControl {
        active_cost_seq: Arc::clone(&active),
        cost_seq: 1,
        deadline: Instant::now() + Duration::from_secs(2),
    };
    assert!(!old_control.is_cancelled());

    sender
        .submit(request(2, "new", 0, "new"))
        .expect("submit replacement");
    assert!(
        old_control.is_cancelled(),
        "the old SQL progress handler must observe replacement before recv_latest"
    );
}

#[test]
fn cancelling_without_a_replacement_interrupts_the_running_query_control() {
    let active = Arc::new(AtomicU64::new(1));
    let (sender, _receiver) = session_cost_mailbox(Arc::clone(&active));
    let old_control = QueryControl {
        active_cost_seq: Arc::clone(&active),
        cost_seq: 1,
        deadline: Instant::now() + Duration::from_secs(2),
    };
    assert!(!old_control.is_cancelled());

    sender.cancel(2).expect("cancel running projection");

    assert!(
        old_control.is_cancelled(),
        "explicit cancellation must update the SQL progress-handler token"
    );
}

#[test]
fn query_control_cancels_at_its_deadline_without_a_replacement() {
    let active = Arc::new(AtomicU64::new(1));
    let control = QueryControl {
        active_cost_seq: active,
        cost_seq: 1,
        deadline: Instant::now() - Duration::from_millis(1),
    };
    assert!(control.is_cancelled());
    assert!(!control.is_superseded());
    assert!(control.deadline_exceeded());
}

#[test]
fn current_deadline_failure_publishes_an_empty_overlay_to_clear_stale_values() {
    let active = Arc::new(AtomicU64::new(1));
    let control = QueryControl {
        active_cost_seq: active,
        cost_seq: 1,
        deadline: Instant::now() - Duration::from_millis(1),
    };
    let (result_tx, result_rx) = std::sync::mpsc::channel();

    assert!(execute_request(
        request(1, "current", 0, "current"),
        control,
        &result_tx,
    ));
    let SessionMsg::CostOverlayReady {
        cost_seq, overlays, ..
    } = result_rx.recv().expect("deadline result")
    else {
        panic!("unexpected session worker result");
    };
    assert_eq!(cost_seq, 1);
    assert!(
        overlays.is_empty(),
        "a current timeout must degrade the accepted page to `-`"
    );
}

#[test]
fn overlay_guard_rejects_stale_seq_token_page_and_identity() {
    let current_token = token("generation-a", 3);
    let identities = vec![
        SessionRowIdentity::capture(&row("one", "/one.jsonl")),
        SessionRowIdentity::capture(&row("two", "/two.jsonl")),
    ];
    let active = ActiveCostOverlay::new(9, current_token.clone(), 2, identities.clone());

    assert!(active.accepts(9, &current_token, 2, &identities));
    assert!(!active.accepts(8, &current_token, 2, &identities));
    assert!(!active.accepts(9, &token("generation-b", 3), 2, &identities));
    assert!(!active.accepts(9, &current_token, 1, &identities));

    let mut changed = identities.clone();
    changed[1] = SessionRowIdentity::capture(&row("replacement", "/replacement.jsonl"));
    assert!(!active.accepts(9, &current_token, 2, &changed));
}

#[test]
fn overlay_guard_rejects_page_switch_even_when_generation_is_unchanged() {
    let current_token = token("generation-a", 3);
    let page_zero = vec![SessionRowIdentity::capture(&row("zero", "/zero.jsonl"))];
    let page_one = vec![SessionRowIdentity::capture(&row("one", "/one.jsonl"))];
    let active = ActiveCostOverlay::new(4, current_token.clone(), 0, page_zero.clone());

    assert!(active.accepts(4, &current_token, 0, &page_zero));
    assert!(!active.accepts(4, &current_token, 1, &page_one));
}

#[test]
fn sender_rejects_requests_after_receiver_shutdown() {
    let active = Arc::new(AtomicU64::new(0));
    let (sender, receiver): (SessionCostRequestSender, _) =
        session_cost_mailbox(Arc::clone(&active));
    drop(receiver);
    assert!(sender.submit(request(1, "gone", 0, "gone")).is_err());
}

#[test]
fn cost_worker_is_started_lazily_by_the_first_projection_request() {
    let active = Arc::new(AtomicU64::new(0));
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    let sender = lazy_session_cost_sender(Arc::clone(&active), result_tx);

    assert!(
        !sender.worker_started_for_test(),
        "constructing the Sessions runtime must not create a Cost worker"
    );

    let mut first = request(1, "lazy", 0, "lazy");
    first.identities[0].provider_id = "openclaw".to_string();
    sender.submit(first).expect("start lazy worker");

    assert!(sender.worker_started_for_test());
    assert!(matches!(
        result_rx.recv_timeout(Duration::from_secs(1)),
        Ok(SessionMsg::CostOverlayReady { cost_seq: 1, .. })
    ));
}
