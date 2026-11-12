use super::*;
use crate::clipboard_export::{Commit, Read};
use crate::clipboard_router::driver::{Driver, Transport, Work};
use std::time::Duration;

struct Pump {
    remote: Remote,
    sent: Vec<wire::Control>,
    replace_on_commit: bool,
}
impl Transport for Pump {
    fn lease(&self) -> Option<Lease> {
        self.remote.receiver.lease()
    }
    async fn commit(&mut self, lease: &Lease) -> Result<Commit, String> {
        if std::mem::take(&mut self.replace_on_commit) {
            self.remote.offer(2, 1);
        }
        let (commit, command) = self.remote.receiver.commit(lease)?;
        self.sent.push(command);
        Ok(commit)
    }
    async fn read(&mut self, copy: &CommittedCopy, format: &str) -> Result<Read, String> {
        let (read, command) = self.remote.receiver.read_copy(copy, format)?;
        self.sent.push(command);
        Ok(read)
    }
}
impl Pump {
    fn acknowledge(&mut self) {
        let wire::Control::Commit { id, offer } = self.sent.pop().unwrap() else {
            panic!("Expected commit");
        };
        self.remote.event(Event::Committed {
            id,
            copy: Some(CopyLease {
                id: Uuid::new_v4().to_string(),
                offer,
            }),
        });
    }
}
async fn next(driver: &mut Driver) -> Work {
    tokio::time::timeout(Duration::from_secs(1), driver.next())
        .await
        .unwrap()
}
fn setup() -> (Driver, Arc<Native>, mpsc::UnboundedReceiver<Request>, Pump) {
    let (native, requests) = Native::new();
    let router = Router::new(native.clone());
    let pump = Pump {
        remote: Remote::new(),
        sent: vec![],
        replace_on_commit: false,
    };
    let driver = Driver::new(router, pump.remote.receiver.connection().unwrap()).unwrap();
    (driver, native, requests, pump)
}
async fn focus(driver: &mut Driver, pump: &mut Pump) {
    let remote = &pump.remote;
    assert!(
        driver
            .endpoint
            .begin_focus(remote.worker)
            .unwrap()
            .acknowledge(Grant::new(remote.runtime, remote.worker, 1).unwrap())
    );
    driver.tick();
    let work = next(driver).await;
    assert!(matches!(work, Work::Metadata(_)));
    driver.complete(work, pump).await.unwrap();
}

#[tokio::test]
async fn driver_waits_for_focus_metadata_and_copy_receipt_then_owns_native_publication() {
    let (mut driver, native, mut requests, mut pump) = setup();
    driver.tick();
    driver.propose(&mut pump).await.unwrap();
    assert!(pump.sent.is_empty());
    assert!(requests.try_recv().is_err());
    focus(&mut driver, &mut pump).await;
    driver.propose(&mut pump).await.unwrap();
    assert_eq!(pump.sent.len(), 1);
    driver.propose(&mut pump).await.unwrap();
    assert_eq!(pump.sent.len(), 1, "pending commit was duplicated");
    assert!(
        requests.try_recv().is_err(),
        "native publication preceded copy receipt"
    );
    pump.acknowledge();
    let work = next(&mut driver).await;
    assert!(matches!(work, Work::Commit(_)));
    driver.complete(work, &mut pump).await.unwrap();
    let request = requests.recv().await.unwrap();
    let source = request.source.clone();
    let publication = native.confirm(&request);
    driver.endpoint.blur();
    pump.remote.event(Event::Offer { offer: None });
    request.reply.send(publication).ok().unwrap();
    let work = next(&mut driver).await;
    assert!(matches!(work, Work::Publication(Ok(Some(_)))));
    driver.complete(work, &mut pump).await.unwrap();
    assert!(source.is_current());
    drop(driver);
    assert!(!source.is_current(), "teardown left native copy authorized");
    assert!(pump.remote.receiver.copy_release().is_some());
}

#[tokio::test]
async fn driver_teardown_drains_late_commit_and_cancels_pending_native_publication() {
    for native_pending in [false, true] {
        let (mut driver, _, mut requests, mut pump) = setup();
        focus(&mut driver, &mut pump).await;
        driver.propose(&mut pump).await.unwrap();
        let pending = if native_pending {
            pump.acknowledge();
            let work = next(&mut driver).await;
            driver.complete(work, &mut pump).await.unwrap();
            Some(requests.recv().await.unwrap())
        } else {
            None
        };
        drop(driver);
        if let Some(pending) = pending {
            assert!(!pending.source.is_current());
        } else {
            pump.acknowledge();
        }
        tokio::task::yield_now().await;
        pump.remote.receiver.expire();
        assert!(pump.remote.receiver.copy_release().is_some());
    }
}

#[tokio::test]
async fn newer_local_copy_during_commit_does_not_rebase_publication() {
    let (mut driver, native, mut requests, mut pump) = setup();
    focus(&mut driver, &mut pump).await;
    driver.propose(&mut pump).await.unwrap();
    native.local();
    driver.tick();
    let work = next(&mut driver).await;
    assert!(matches!(work, Work::Metadata(_)));
    driver.complete(work, &mut pump).await.unwrap();
    pump.acknowledge();
    let work = next(&mut driver).await;
    driver.complete(work, &mut pump).await.unwrap();
    let work = next(&mut driver).await;
    assert!(matches!(work, Work::Publication(Ok(None))));
    driver.complete(work, &mut pump).await.unwrap();
    assert!(requests.try_recv().is_err());
    assert!(pump.remote.receiver.copy_release().is_some());
}

#[tokio::test]
async fn source_replacement_during_commit_admission_refuses_old_and_accepts_new_offer() {
    let (mut driver, _, _, mut pump) = setup();
    focus(&mut driver, &mut pump).await;
    pump.replace_on_commit = true;
    driver.propose(&mut pump).await.unwrap();
    assert!(pump.sent.is_empty());
    driver.propose(&mut pump).await.unwrap();
    assert!(
        matches!(&pump.sent[..], [wire::Control::Commit { offer, .. }] if offer.offer.revision == 2)
    );
}

#[cfg(unix)]
mod files {
    use super::*;
    use crate::clipboard_files::tests::{Disk, Local};
    use crate::clipboard_router::driver::files::Files;
    use crate::desktop_integration::{
        ClipboardFiles, ClipboardFilesReply, ClipboardFilesStatus, DesktopIntegration,
    };
    use newport_platform::clipboard::FILES_FORMAT;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Answers questions with `answer`, or holds them when it is None.
    #[derive(Default)]
    struct Desktop {
        answer: Mutex<Option<bool>>,
        asked: Mutex<Vec<(String, ClipboardFiles, Arc<AtomicBool>)>>,
        held: Mutex<Vec<ClipboardFilesReply>>,
        statuses: Mutex<Vec<ClipboardFilesStatus>>,
    }
    impl DesktopIntegration for Desktop {
        fn on_main_thread(&self, _: Box<dyn FnOnce() + Send>) -> Result<(), String> {
            Err("No desktop thread".into())
        }
        fn clipboard_revision(&self) -> Option<isize> {
            None
        }
        fn clipboard_offer(&self, _: Option<isize>) -> Option<(isize, Vec<String>)> {
            None
        }
        fn read_clipboard_format(&self, _: isize, _: &str) -> Option<Vec<u8>> {
            None
        }
        fn open_url(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn request_clipboard_files(
            &self,
            server: &str,
            files: ClipboardFiles,
            reply: ClipboardFilesReply,
            cancelled: Arc<AtomicBool>,
        ) {
            self.asked
                .lock()
                .unwrap()
                .push((server.into(), files, cancelled));
            match *self.answer.lock().unwrap() {
                Some(answer) => reply(answer),
                None => self.held.lock().unwrap().push(reply),
            }
        }
        fn clipboard_files_status(&self, _: &str, status: ClipboardFilesStatus) {
            self.statuses.lock().unwrap().push(status);
        }
    }

    struct Fixture {
        driver: Driver,
        router: Router,
        native: Arc<Native>,
        requests: mpsc::UnboundedReceiver<Request>,
        pump: Pump,
        disk: Arc<Disk>,
        desktop: Arc<Desktop>,
        cache: tempfile::TempDir,
    }
    impl Fixture {
        fn new(answer: Option<bool>) -> Self {
            let (native, requests) = Native::new();
            let cache = tempfile::tempdir().unwrap();
            let router = Router::with_download_root(native.clone(), cache.path().into());
            let mut pump = Pump {
                remote: Remote::new(),
                sent: vec![],
                replace_on_commit: false,
            };
            pump.remote
                .offer_formats(2, 1, &[FILES_FORMAT, "text/plain"]);
            let disk = Disk::new(false);
            let desktop = Arc::new(Desktop {
                answer: Mutex::new(answer),
                ..Desktop::default()
            });
            let driver = Driver::new(router.clone(), pump.remote.receiver.connection().unwrap())
                .unwrap()
                .with_files(Files::new(
                    Local(disk.clone()),
                    "dev".into(),
                    desktop.clone(),
                ));
            Self {
                driver,
                router,
                native,
                requests,
                pump,
                disk,
                desktop,
                cache,
            }
        }
        /// Copies and publishes the remote offer.
        async fn publish(&mut self) {
            focus(&mut self.driver, &mut self.pump).await;
            self.driver.propose(&mut self.pump).await.unwrap();
            self.pump.acknowledge();
            let work = next(&mut self.driver).await;
            assert!(matches!(work, Work::Commit(_)));
            self.driver.complete(work, &mut self.pump).await.unwrap();
            let request = self.requests.recv().await.unwrap();
            let publication = self.native.confirm(&request);
            request.reply.send(publication).ok().unwrap();
            let work = next(&mut self.driver).await;
            assert!(matches!(work, Work::Publication(Ok(Some(_)))));
            self.driver.complete(work, &mut self.pump).await.unwrap();
        }
        /// Serves the download's read of the path list with `paths`.
        async fn serve_paths(&mut self, paths: &[PathBuf]) {
            let work = next(&mut self.driver).await;
            assert!(matches!(work, Work::Demand(_)));
            self.driver.complete(work, &mut self.pump).await.unwrap();
            let Some(wire::Control::ReadCopy { id, format, .. }) = self.pump.sent.pop() else {
                panic!("Expected a read of the copy");
            };
            assert_eq!(format, FILES_FORMAT);
            let list = serde_json::json!({"version": 1, "paths": paths});
            self.pump.remote.event(Event::Data {
                id,
                offset: 0,
                chunk: Some(Chunk {
                    bytes: serde_json::to_vec(&list).unwrap(),
                    eof: true,
                }),
            });
            let work = next(&mut self.driver).await;
            assert!(matches!(work, Work::Read(Ok(()))));
            self.driver.complete(work, &mut self.pump).await.unwrap();
        }
        /// Runs driver work until the download ends.
        async fn finish(&mut self) {
            let work = tokio::time::timeout(Duration::from_secs(10), self.driver.next())
                .await
                .unwrap();
            assert!(matches!(work, Work::Download(Ok(()))));
            self.driver.complete(work, &mut self.pump).await.unwrap();
        }
        /// Download folders in the cache.
        fn folders(&self) -> Vec<PathBuf> {
            let mut folders: Vec<PathBuf> = std::fs::read_dir(self.cache.path())
                .map(|entries| entries.map(|entry| entry.unwrap().path()).collect())
                .unwrap_or_default();
            folders.sort();
            folders
        }
        fn attached(&self) -> Vec<(u64, PathBuf)> {
            self.native.attached.lock().unwrap().clone()
        }
    }
    /// A folder and a file in the server user's home.
    fn copied(disk: &Disk) -> Vec<PathBuf> {
        let docs = disk.home.path().join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(docs.join("a.txt"), b"first").unwrap();
        let file = disk.home.path().join("b.txt");
        std::fs::write(&file, vec![7u8; 70_000]).unwrap();
        vec![docs, file]
    }
    async fn until(condition: impl Fn() -> bool) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !condition() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("condition not reached");
    }

    #[tokio::test]
    async fn a_published_copy_of_files_downloads_and_attaches_them() {
        let mut f = Fixture::new(None);
        let paths = copied(&f.disk);
        f.publish().await;
        f.serve_paths(&paths).await;
        f.finish().await;
        let folders = f.folders();
        assert_eq!(folders.len(), 1);
        let folder = &folders[0];
        assert_eq!(std::fs::read(folder.join("docs/a.txt")).unwrap(), b"first");
        assert_eq!(std::fs::read(folder.join("b.txt")).unwrap().len(), 70_000);
        let generation = match f.router.offer(None).await.unwrap().origin {
            ClipboardOrigin::Imported { publication } => publication,
            ClipboardOrigin::Local => panic!("The import was replaced"),
        };
        assert_eq!(f.attached(), [(generation, folder.clone())]);
        // Small copies inside home never ask, and a quick one shows no status.
        assert!(f.desktop.asked.lock().unwrap().is_empty());
        assert!(f.desktop.statuses.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_copy_without_files_downloads_nothing() {
        let mut f = Fixture::new(None);
        f.pump.remote.offer_formats(3, 1, &["text/plain"]);
        f.publish().await;
        f.finish().await;
        assert!(f.folders().is_empty());
        assert!(f.attached().is_empty());
    }

    #[tokio::test]
    async fn a_newer_mac_copy_mid_download_attaches_nothing_and_leaves_no_folder() {
        let mut f = Fixture::new(None);
        let paths = copied(&f.disk);
        f.disk.hold_reads.store(true, Ordering::Release);
        f.publish().await;
        f.serve_paths(&paths).await;
        let disk = f.disk.clone();
        until(|| disk.reading.load(Ordering::Acquire) > 0).await;
        assert_eq!(f.folders().len(), 1, "the download is staged");
        f.native.local();
        f.router.offer(None).await.unwrap();
        f.disk
            .reads
            .add_permits(tokio::sync::Semaphore::MAX_PERMITS);
        f.finish().await;
        assert!(f.folders().is_empty());
        assert!(f.attached().is_empty());
        assert!(f.desktop.statuses.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_slow_download_shows_that_it_is_copying_until_it_ends() {
        let mut f = Fixture::new(None);
        let paths = copied(&f.disk);
        f.disk.hold_reads.store(true, Ordering::Release);
        f.publish().await;
        f.serve_paths(&paths).await;
        let disk = f.disk.clone();
        until(|| disk.reading.load(Ordering::Acquire) > 0).await;
        assert!(f.desktop.statuses.lock().unwrap().is_empty());
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(3)).await;
        tokio::time::resume();
        let desktop = f.desktop.clone();
        until(|| !desktop.statuses.lock().unwrap().is_empty()).await;
        f.disk
            .reads
            .add_permits(tokio::sync::Semaphore::MAX_PERMITS);
        f.finish().await;
        assert_eq!(
            *f.desktop.statuses.lock().unwrap(),
            [
                ClipboardFilesStatus::Copying,
                ClipboardFilesStatus::Finished
            ]
        );
        assert_eq!(f.attached().len(), 1);
    }

    #[tokio::test]
    async fn dropping_the_driver_ends_the_copying_notice_of_a_slow_download() {
        let mut f = Fixture::new(None);
        let paths = copied(&f.disk);
        f.disk.hold_reads.store(true, Ordering::Release);
        f.publish().await;
        f.serve_paths(&paths).await;
        let disk = f.disk.clone();
        until(|| disk.reading.load(Ordering::Acquire) > 0).await;
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(3)).await;
        tokio::time::resume();
        let desktop = f.desktop.clone();
        until(|| !desktop.statuses.lock().unwrap().is_empty()).await;
        assert_eq!(
            *desktop.statuses.lock().unwrap(),
            [ClipboardFilesStatus::Copying]
        );
        let Fixture { driver, .. } = f;
        drop(driver);
        until(|| desktop.statuses.lock().unwrap().len() == 2).await;
        assert_eq!(
            *desktop.statuses.lock().unwrap(),
            [
                ClipboardFilesStatus::Copying,
                ClipboardFilesStatus::Finished
            ]
        );
    }

    #[tokio::test]
    async fn dropping_the_driver_stops_its_download() {
        let mut f = Fixture::new(None);
        let paths = copied(&f.disk);
        f.disk.hold_reads.store(true, Ordering::Release);
        f.publish().await;
        f.serve_paths(&paths).await;
        let disk = f.disk.clone();
        until(|| disk.reading.load(Ordering::Acquire) > 0).await;
        let Fixture {
            driver,
            native,
            cache,
            ..
        } = f;
        drop(driver);
        until(|| std::fs::read_dir(cache.path()).unwrap().next().is_none()).await;
        assert!(native.attached.lock().unwrap().is_empty());
    }

    /// A file outside the server user's home makes the copy ask first.
    fn outside(disk: &Disk) -> (tempfile::TempDir, Vec<PathBuf>) {
        let other = tempfile::tempdir().unwrap();
        std::fs::write(other.path().join("c.txt"), b"outside").unwrap();
        let mut paths = copied(disk);
        paths.push(other.path().join("c.txt"));
        (other, paths)
    }
    fn asked(f: &Fixture) -> Vec<(String, ClipboardFiles, bool)> {
        f.desktop
            .asked
            .lock()
            .unwrap()
            .iter()
            .map(|(server, files, cancelled)| {
                (
                    server.clone(),
                    files.clone(),
                    cancelled.load(Ordering::Acquire),
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn a_copy_reaching_outside_home_asks_and_downloads_on_yes_only() {
        for answer in [true, false] {
            let mut f = Fixture::new(Some(answer));
            let (_other, paths) = outside(&f.disk);
            f.publish().await;
            f.serve_paths(&paths).await;
            f.finish().await;
            let files = ClipboardFiles {
                items: 4,
                bytes: 5 + 70_000 + 7,
                outside_home: true,
                hidden: false,
                executable: false,
                installer: false,
                large: false,
                names: vec!["docs".into(), "b.txt".into(), "c.txt".into()],
                top_level: 3,
            };
            assert_eq!(asked(&f), [("dev".to_owned(), files, true)]);
            assert_eq!(f.folders().len(), usize::from(answer));
            assert_eq!(f.attached().len(), usize::from(answer));
            if answer {
                assert_eq!(
                    std::fs::read(f.folders()[0].join("c.txt")).unwrap(),
                    b"outside"
                );
            }
        }
    }

    #[tokio::test]
    async fn an_unanswered_question_is_a_no_after_two_minutes() {
        let mut f = Fixture::new(None);
        let (_other, paths) = outside(&f.disk);
        f.publish().await;
        f.serve_paths(&paths).await;
        let desktop = f.desktop.clone();
        until(|| desktop.asked.lock().unwrap().len() == 1).await;
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(119)).await;
        assert!(!asked(&f)[0].2, "closed before its time");
        tokio::time::advance(Duration::from_secs(2)).await;
        tokio::time::resume();
        f.finish().await;
        assert!(asked(&f)[0].2, "the question stayed open");
        assert!(f.folders().is_empty());
        assert!(f.attached().is_empty());
    }

    #[tokio::test]
    async fn a_question_closes_when_its_copy_is_replaced() {
        let mut f = Fixture::new(None);
        let (_other, paths) = outside(&f.disk);
        f.publish().await;
        f.serve_paths(&paths).await;
        let desktop = f.desktop.clone();
        until(|| desktop.asked.lock().unwrap().len() == 1).await;
        f.native.local();
        f.router.offer(None).await.unwrap();
        f.finish().await;
        assert!(asked(&f)[0].2);
        // A late yes changes nothing.
        let reply = f.desktop.held.lock().unwrap().pop().unwrap();
        reply(true);
        assert!(f.folders().is_empty());
        assert!(f.attached().is_empty());
    }

    #[tokio::test]
    async fn names_a_mac_folder_cannot_hold_fail_with_a_notice() {
        let mut f = Fixture::new(None);
        let other = tempfile::tempdir().unwrap();
        std::fs::write(other.path().join("Notes.txt"), b"x").unwrap();
        std::fs::write(f.disk.home.path().join("notes.txt"), b"y").unwrap();
        f.publish().await;
        f.serve_paths(&[
            other.path().join("Notes.txt"),
            f.disk.home.path().join("notes.txt"),
        ])
        .await;
        f.finish().await;
        assert_eq!(
            *f.desktop.statuses.lock().unwrap(),
            [ClipboardFilesStatus::Failed(
                "“Notes.txt” and “notes.txt” would have the same name on this Mac".into()
            )]
        );
        assert!(f.folders().is_empty());
        assert!(f.attached().is_empty());
    }
}
