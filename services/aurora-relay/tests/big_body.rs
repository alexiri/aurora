//! A 1 GiB response streams through the relay in constant memory (`docs/18 §Port previews`
//! "Relay": "memory stays flat for a 1 GiB response"; FIX-073). Its own test binary so no other
//! test's allocations move the process's resident set while it is measured.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::time::{Duration, Instant};

use common::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The whole gibibyte arrives, and while it flows the process's resident set stays within
/// 96 MiB of where it started: the relay holds a few 16 KiB frames per stream, never the body.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_gibibyte_streams_in_constant_memory() {
    let r = rig(
        Timeouts {
            header: Duration::from_secs(60),
            idle_http: Duration::from_secs(60),
            idle_ws: Duration::from_secs(60),
            lifetime: Duration::from_secs(300),
            validate_every: Duration::from_secs(60),
        },
        false,
    )
    .await;
    let mut c = connect(&r).await;
    c.write_all(request("/gig", &[], true).as_bytes())
        .await
        .unwrap();
    let (status, h, rest) = read_head(&mut c).await;
    assert_eq!(status, 200);
    assert_eq!(header(&h, "content-length"), Some(GIB.to_string().as_str()));
    let base = rss_kib();
    let started = Instant::now();
    let mut peak = base;
    let mut got = rest.len() as u64;
    let mut buf = vec![0u8; 256 * 1024];
    let mut next_sample = 0u64;
    while got < GIB {
        let n = tokio::time::timeout(Duration::from_secs(30), c.read(&mut buf))
            .await
            .expect("the gibibyte stalled")
            .unwrap();
        assert!(n > 0, "closed after {got} of {GIB} bytes");
        got += n as u64;
        if got >= next_sample {
            peak = peak.max(rss_kib());
            next_sample = got + 8 * 1024 * 1024;
        }
        assert!(
            started.elapsed() < Duration::from_secs(120),
            "too slow: {got} bytes in 120 s"
        );
    }
    assert_eq!(got, GIB, "exactly the declared size");
    let grew_mib = (peak - base) / 1024;
    eprintln!(
        "1 GiB in {:?}; resident growth {grew_mib} MiB",
        started.elapsed()
    );
    assert!(
        grew_mib < 96,
        "resident set grew {grew_mib} MiB while streaming 1 GiB"
    );
}
