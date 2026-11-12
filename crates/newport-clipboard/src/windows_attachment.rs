//! File attachment admission is independent of native clipboard ownership.
use std::sync::Mutex;

/// A receipt describes native effects independently of the caller's lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) enum FileWrite {
    Unchanged,
    Committed { revision: isize, complete: bool },
    Uncertain,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stage<B> {
    Preparing(B),
    Committing(Option<B>),
    Native,
    Canceled,
    Uncertain,
}
pub(super) struct Attachment<B>(Mutex<Stage<B>>);
impl<B: Copy> Attachment<B> {
    pub(super) fn new(baseline: Option<B>) -> Self {
        Self(Mutex::new(baseline.map_or(Stage::Native, Stage::Preparing)))
    }
    pub(super) fn stage(&self) -> Stage<B> {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub(super) fn prepare(&self, baseline: Option<B>) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            baseline.map_or(Stage::Native, Stage::Preparing);
    }
    /// Pure policy only: the caller releases this gate before native work.
    pub(super) fn claim(&self, confirm: impl FnOnce() -> bool) -> bool {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let baseline = match *state {
            Stage::Preparing(baseline) => Some(baseline),
            Stage::Native => None,
            _ => return false,
        };
        if !confirm() {
            return false;
        }
        *state = Stage::Committing(baseline);
        true
    }
    /// Cancellation may retire a reservation only before claim wins this gate.
    pub(super) fn cancel_preparation(&self) -> bool {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !matches!(*state, Stage::Preparing(_)) {
            return false;
        }
        *state = Stage::Canceled;
        true
    }
    pub(super) fn settle(&self, outcome: FileWrite) {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *state = match outcome {
            FileWrite::Committed { .. } => Stage::Native,
            FileWrite::Uncertain => Stage::Uncertain,
            FileWrite::Unchanged => match *state {
                Stage::Preparing(_) | Stage::Committing(Some(_)) => Stage::Canceled,
                Stage::Committing(None) => Stage::Native,
                other => other,
            },
        };
    }
}

/// DROPFILES uses a fixed DWORD/POINT/BOOL header followed by UTF-16 paths.
/// Validation of path identity belongs to RetainedFiles before this encoding.
pub(super) fn dropfiles(paths: &[&str], limit: usize) -> Option<Vec<u8>> {
    if paths.is_empty() {
        return None;
    }
    let mut length = 22usize;
    for path in paths {
        if path.is_empty() || path.contains('\0') {
            return None;
        }
        length = length.checked_add(path.encode_utf16().count().checked_add(1)?.checked_mul(2)?)?;
    }
    if length > limit {
        return None;
    }
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(length).ok()?;
    bytes.extend_from_slice(&20u32.to_le_bytes());
    bytes.extend_from_slice(&[0; 12]);
    bytes.extend_from_slice(&1u32.to_le_bytes());
    for path in paths {
        for unit in path.encode_utf16().chain(Some(0)) {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
    }
    bytes.extend_from_slice(&[0, 0]);
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preparation_cancellation_and_commit_claim_have_one_winner() {
        let canceled = Attachment::new(None);
        canceled.prepare(Some(41));
        assert_eq!(canceled.stage(), Stage::Preparing(41));
        assert!(canceled.cancel_preparation());
        assert!(!canceled.claim(|| panic!("canceled reservation cannot confirm")));
        assert_eq!(canceled.stage(), Stage::Canceled);

        let committed = std::sync::Arc::new(Attachment::new(Some(41)));
        let (entered, seen) = std::sync::mpsc::channel();
        let (resume, paused) = std::sync::mpsc::channel();
        let claimant = committed.clone();
        let worker = std::thread::spawn(move || {
            claimant.claim(|| {
                entered.send(()).unwrap();
                paused
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .unwrap();
                true
            })
        });
        seen.recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        let canceled = committed.clone();
        let canceler = std::thread::spawn(move || canceled.cancel_preparation());
        resume.send(()).unwrap();
        assert!(worker.join().unwrap());
        assert!(!canceler.join().unwrap());
        assert_eq!(committed.stage(), Stage::Committing(Some(41)));
    }

    #[test]
    fn failed_confirmation_preserves_cancelability_and_receipts_do_not_replay() {
        let preparation = Attachment::new(Some(41));
        assert!(!preparation.claim(|| false));
        assert_eq!(preparation.stage(), Stage::Preparing(41));
        assert!(preparation.cancel_preparation());
        preparation.settle(FileWrite::Unchanged);
        assert_eq!(preparation.stage(), Stage::Canceled);
        assert!(!preparation.claim(|| true));

        for receipt in [
            FileWrite::Committed {
                revision: 42,
                complete: false,
            },
            FileWrite::Uncertain,
        ] {
            let preparation = Attachment::new(Some(41));
            assert!(preparation.claim(|| true));
            assert!(!preparation.claim(|| panic!("a claimed operation must not replay")));
            preparation.settle(receipt);
            assert!(!preparation.cancel_preparation());
            assert_eq!(
                preparation.stage(),
                match receipt {
                    FileWrite::Committed { .. } => Stage::Native,
                    _ => Stage::Uncertain,
                }
            );
            if receipt == FileWrite::Uncertain {
                assert!(!preparation.claim(|| true));
            }
        }
        let mixed = Attachment::<isize>::new(None);
        assert!(mixed.claim(|| true));
        mixed.settle(FileWrite::Unchanged);
        assert_eq!(
            mixed.stage(),
            Stage::Native,
            "failed file attachment retains published text"
        );
    }

    #[test]
    fn file_list_has_a_wide_header_and_double_terminator_with_bounded_allocation() {
        let paths = [r"C:\cache\文書.txt", r"C:\cache\folder"];
        let bytes = dropfiles(&paths, 4096).unwrap();
        assert_eq!(&bytes[..4], &20u32.to_le_bytes());
        assert_eq!(&bytes[16..20], &1u32.to_le_bytes());
        let units: Vec<_> = bytes[20..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes(*b))
            .collect();
        assert_eq!(
            String::from_utf16(&units).unwrap(),
            format!("{}\0{}\0\0", paths[0], paths[1])
        );
        assert!(dropfiles(&paths, bytes.len() - 1).is_none());
        assert!(dropfiles(&["C:\\unsafe\0tail"], 4096).is_none());
        assert!(dropfiles(&[], 4096).is_none());
    }
}
