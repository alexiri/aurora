//! (FIX-290) A service of Aurora's that does not resolve keeps every self-hosted app's
//! allowance closed, and says so once a minute per name, not on every dial.

use aurora_core::ranges::AppHost;
use aurora_gateway::guard::Guard;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Sink {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Sink {
    /// The warnings that name `service`.
    fn warnings(&self, service: &str) -> usize {
        String::from_utf8_lossy(&self.0.lock().unwrap())
            .lines()
            .filter(|l| l.contains("did not resolve") && l.contains(service))
            .count()
    }
}

#[tokio::test]
async fn an_unresolvable_service_is_logged_once_not_on_every_dial() {
    const HOST: &str = "wiki.corp.test";
    let guard = Guard::new(&[], Vec::new()).unwrap();
    let at = vec!["10.1.2.3:443".parse().unwrap()];
    guard.set_static_names(vec![
        (HOST.to_owned(), at.clone()),
        (format!("{HOST}."), at),
    ]);
    let app = AppHost::new(HOST, 443).unwrap();
    let sink = Sink::default();
    let writer = sink.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    let _log = tracing::subscriber::set_default(subscriber);

    guard.set_own_services(vec!["first.invalid".to_owned()]);
    for _ in 0..5 {
        assert!(guard.resolve_app(&app).await.is_err());
    }
    assert_eq!(sink.warnings("first.invalid"), 1);
    // Another name has its own minute.
    guard.set_own_services(vec!["second.invalid".to_owned()]);
    for _ in 0..3 {
        assert!(guard.resolve_app(&app).await.is_err());
    }
    assert_eq!(sink.warnings("second.invalid"), 1);
    assert_eq!(sink.warnings("first.invalid"), 1);
}
