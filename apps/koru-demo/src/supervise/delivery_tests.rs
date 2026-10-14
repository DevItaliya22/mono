// SPDX-License-Identifier: MPL-2.0

use super::*;
use std::sync::atomic::AtomicUsize;

struct Delayed {
    resume: Arc<Mutex<Receiver<()>>>,
    entered: Sender<()>,
    finished: Sender<()>,
    delivered: Arc<AtomicUsize>,
}
impl Mechanism for Delayed {
    fn perform(&mut self, _: Request) -> Response {
        panic!("unguarded dispatch")
    }
    fn perform_guarded(&mut self, _: Request, permit: &DeliveryPermit) -> Response {
        self.entered.send(()).unwrap();
        self.resume.lock().unwrap().recv().unwrap();
        permit.consequence(|| self.delivered.fetch_add(1, Ordering::SeqCst));
        self.finished.send(()).unwrap();
        Response::Checked(true)
    }
}

fn resumed_after(cancellation: bool) {
    let (resume, paused) = channel();
    let paused = Arc::new(Mutex::new(paused));
    let (entered, entering) = channel();
    let (finished, finishing) = channel();
    let delivered = Arc::new(AtomicUsize::new(0));
    let sink = delivered.clone();
    let start: Start = Arc::new(move |_| {
        Ok(Box::new(Delayed {
            resume: paused.clone(),
            entered: entered.clone(),
            finished: finished.clone(),
            delivered: sink.clone(),
        }))
    });
    let mut supervisor = Supervisor::new(
        start,
        Limits {
            budget: if cancellation {
                Duration::from_secs(2)
            } else {
                Duration::from_millis(10)
            },
            max_abandoned: 2,
        },
    );
    let cancel = CancelToken::new();
    let canceller = if cancellation {
        let token = cancel.clone();
        Some(std::thread::spawn(move || {
            entering.recv().unwrap();
            token.cancel();
        }))
    } else {
        None
    };
    let result = supervisor.call(Request::BindWindow { native_handle: 123 }, &cancel);
    assert_eq!(
        result,
        if cancellation {
            Outcome::Cancelled
        } else {
            Outcome::Lost
        }
    );
    // The abandoned provider returns AFTER the supervisor already reported
    // failure. Its next attempted consequence must see revocation.
    resume.send(()).unwrap();
    finishing.recv_timeout(Duration::from_secs(2)).unwrap();
    if let Some(thread) = canceller {
        thread.join().unwrap();
    }
    assert_eq!(delivered.load(Ordering::SeqCst), 0);
}

#[test]
fn timed_out_provider_cannot_inject_when_it_eventually_returns() {
    resumed_after(false);
}
#[test]
fn cancelled_provider_cannot_inject_when_it_eventually_returns() {
    resumed_after(true);
}
