//! The inline urgent path: a `warn` never waits on a sink the writer thread
//! holds for longer than its bound.

use super::*;

/// A sink that blocks the writer thread inside its first write until `stop`
/// is set, announcing itself on `entered` while it holds the sink mutex.
struct Blocked {
    entered: Option<std::sync::mpsc::SyncSender<()>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    out: Shared,
}

impl Write for Blocked {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Some(entered) = self.entered.take() {
            let _ = entered.try_send(());
            while !self.stop.load(std::sync::atomic::Ordering::SeqCst) {
                std::thread::park_timeout(Duration::from_millis(2));
            }
        }
        self.out.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn an_inline_warn_waits_for_a_wedged_sink_at_most_its_bound() {
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (entered, came_in) = std::sync::mpsc::sync_channel(1);
    let sink = Shared::default();
    let kernel = Registry::new().finish(
        Logger::new(
            LogFilter::default(),
            Format::Json,
            Box::new(Blocked {
                entered: Some(entered),
                stop: Arc::clone(&stop),
                out: sink.clone(),
            }),
            fixed,
        ),
        Arc::new(crate::ThreadRuntime::new()),
    );
    let ctx = Ctx::root(&kernel);
    // The one queued line parks the writer thread inside the sink mutex.
    log!(ctx, info, GREETED);
    came_in
        .recv_timeout(Duration::from_secs(5))
        .expect("the writer holds the sink");
    let started = Instant::now();
    log!(ctx, warn, GREETED);
    let waited = started.elapsed();
    // The inline bound is 50 ms; generous slack for a loaded machine.
    assert!(
        waited < Duration::from_secs(2),
        "the warn blocked on the wedged sink: {waited:?}"
    );
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(kernel.logger().flush());
    let text = sink.text();
    let warn_lines = text
        .lines()
        .filter(|line| line.contains(r#""level":"warn""#))
        .count();
    assert_eq!(
        warn_lines, 1,
        "the dropped warn is not in the sink, only the report: {text}"
    );
    let reports: Vec<&str> = text
        .lines()
        .filter(|line| line.contains(r#""event":"kernel.log_dropped""#))
        .collect();
    assert_eq!(reports.len(), 1, "one dropped-lines report: {text}");
    let count: usize =
        crate::testing::json_field(reports.first().expect("one report line"), "count")
            .expect("a count field")
            .parse()
            .expect("a numeric count");
    assert_eq!(count, 1, "exactly the one warn was dropped: {text}");
}
