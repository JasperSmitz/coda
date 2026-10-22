//! The TUI panic hook: only panics on the thread that installed it (the UI
//! thread) restore the terminal and reach the previous hook; panics on
//! other threads, including ones later caught by `catch_unwind`, are only
//! logged. Kept in its own test binary because the hook is process-global.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use coda_player::app::install_panic_hook;

#[test]
fn only_ui_thread_panics_reach_the_previous_hook() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&calls);
    install_panic_hook(Box::new(move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
    }));

    // A plain background thread.
    let joined = std::thread::spawn(|| panic!("background")).join();
    assert!(joined.is_err());

    // A panic caught by `catch_unwind` on another thread (the scan's guard).
    std::thread::spawn(|| {
        let caught = std::panic::catch_unwind(|| panic!("tag parser"));
        assert!(caught.is_err());
    })
    .join()
    .unwrap();

    // A Tokio worker thread.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .build()
        .unwrap();
    let task = runtime.block_on(async { tokio::spawn(async { panic!("worker") }).await });
    assert!(task.unwrap_err().is_panic());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "background panics only log"
    );

    // The UI (installing) thread chains to the previous hook.
    let caught = std::panic::catch_unwind(|| panic!("ui"));
    assert!(caught.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    drop(std::panic::take_hook());
}
