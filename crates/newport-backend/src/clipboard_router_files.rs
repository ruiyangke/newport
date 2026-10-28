//! Downloads the files of a server copy once the copy is published on the Mac,
//! then offers them in its place. The download follows the copy, not focus: a
//! newer copy on either side, revoked permission or a disconnect stops it.
use crate::{
    clipboard_downloads::{self, Decision, Plan, Ticket},
    clipboard_export::source::Source,
    clipboard_files::Connect,
    clipboard_router::{Published, Router},
    desktop_integration::{ClipboardFiles, ClipboardFilesStatus, Desktop},
};
use anyhow::{Context, Result};
use newport_platform::clipboard::{FILES_FORMAT, provider::read_source};
use std::{
    future::Future,
    path::Path,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

/// An unanswered question is a no after this.
const ASK_LIMIT: Duration = Duration::from_secs(120);
/// A download still running after this shows that it is copying.
const SLOW: Duration = Duration::from_secs(3);
/// How often a question checks that its copy is still current.
const CHECK: Duration = Duration::from_millis(100);

/// Downloads the files of one published copy.
pub(crate) trait Download: Send + Sync + 'static {
    fn run(
        self: Arc<Self>,
        router: Router,
        published: Published,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}

/// The files of one server, reached over its SFTP connection.
pub(crate) struct Files<C> {
    connect: C,
    server: String,
    desktop: Desktop,
}
impl<C: Connect> Files<C> {
    pub(crate) fn new(connect: C, server: String, desktop: Desktop) -> Arc<Self> {
        Arc::new(Self {
            connect,
            server,
            desktop,
        })
    }

    async fn download(&self, router: Router, published: Published) {
        let Published { generation, source } = published;
        if !source
            .offer()
            .offer
            .formats
            .iter()
            .any(|f| f == FILES_FORMAT)
        {
            return;
        }
        let Some((cache, ticket)) = router.begin_download(generation) else {
            return;
        };
        let mut copying = None;
        let result = async {
            let Some((fs, plan)) = self.plan(&source, &ticket).await? else {
                return Ok(None);
            };
            let fetching = cache.fetch(&*fs, &plan, &ticket);
            tokio::pin!(fetching);
            tokio::select! {
                result = &mut fetching => return result.map(Some),
                () = tokio::time::sleep(SLOW) => {}
            }
            copying = Some(self.copying());
            fetching.await.map(Some)
        }
        .await;
        let error = match result {
            Ok(Some(folder)) => {
                attached(&router, generation, &folder, &ticket).await;
                None
            }
            Ok(None) => None,
            Err(error) => ticket.is_current().then(|| format!("{error:#}")),
        };
        if let Some(error) = error {
            if let Some(copying) = copying.as_mut() {
                copying.0 = None;
            }
            self.status(ClipboardFilesStatus::Failed(error));
        }
    }

    /// Shows that files are being copied, until the guard is dropped, whether
    /// the download ended or was stopped.
    fn copying(&self) -> Copying<'_, C> {
        self.status(ClipboardFilesStatus::Copying);
        Copying(Some(self))
    }

    /// Reads the copy's server paths and plans them; None when the copy is
    /// not to be downloaded.
    async fn plan(
        &self,
        source: &Source,
        ticket: &Ticket,
    ) -> Result<Option<(Arc<C::Remote>, Plan)>> {
        let Some(list) = read_source(source, FILES_FORMAT).await else {
            return if ticket.is_current() {
                Err(anyhow::anyhow!(
                    "The server didn’t send the copied file list"
                ))
            } else {
                Ok(None)
            };
        };
        let paths = clipboard_downloads::parse_paths(list.bytes().unwrap_or_default())?;
        drop(list);
        let fs = self
            .connect
            .connect()
            .await
            .context("Couldn’t reach the server")?;
        let plan = clipboard_downloads::plan(&*fs, &paths).await?;
        if !ticket.is_current() {
            return Ok(None);
        }
        let wanted = match clipboard_downloads::decide(&plan) {
            Decision::Auto | Decision::Refuse => true,
            Decision::Ask => self.ask(&plan, ticket).await,
        };
        Ok(wanted.then_some((fs, plan)))
    }

    /// Whether the person wants `plan` downloaded. Closes the question when
    /// the copy stops being current or after [`ASK_LIMIT`].
    async fn ask(&self, plan: &Plan, ticket: &Ticket) -> bool {
        let cancelled = Cancel(Arc::new(AtomicBool::new(false)));
        let (reply, answer) = tokio::sync::oneshot::channel();
        self.desktop.request_clipboard_files(
            &self.server,
            ClipboardFiles {
                items: plan.count(),
                bytes: plan.bytes(),
                outside_home: !plan.inside_home(),
                hidden: plan.hidden(),
                executable: plan.executable(),
                installer: plan.installer(),
                large: plan.large(),
                names: plan.shown_names(),
                top_level: plan.top_level_count(),
            },
            Box::new(move |yes| {
                let _ = reply.send(yes);
            }),
            cancelled.0.clone(),
        );
        let expired = tokio::time::sleep(ASK_LIMIT);
        tokio::pin!(expired, answer);
        let mut check = tokio::time::interval(CHECK);
        check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let yes = loop {
            tokio::select! {
                answer = &mut answer => break answer.unwrap_or(false),
                () = &mut expired => break false,
                _ = check.tick() => if !ticket.is_current() { break false },
            }
        };
        yes && ticket.is_current()
    }

    fn status(&self, status: ClipboardFilesStatus) {
        self.desktop.clipboard_files_status(&self.server, status);
    }
}
impl<C: Connect> Download for Files<C> {
    fn run(
        self: Arc<Self>,
        router: Router,
        published: Published,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async move { self.download(router, published).await })
    }
}

/// Offers `folder` in place of the copy. A copy replaced meanwhile has its
/// download retired by the cache.
async fn attached(router: &Router, generation: u64, folder: &Path, ticket: &Ticket) {
    if router.attach_files(generation, folder).await.is_none() && ticket.is_current() {
        log::warn!("Clipboard files were downloaded but the clipboard refused them");
    }
}

/// Ends the copying notice on drop.
struct Copying<'a, C: Connect>(Option<&'a Files<C>>);
impl<C: Connect> Drop for Copying<'_, C> {
    fn drop(&mut self) {
        if let Some(files) = self.0.take() {
            files.status(ClipboardFilesStatus::Finished);
        }
    }
}

/// Closes an open question when the download ends or is dropped.
struct Cancel(Arc<AtomicBool>);
impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
