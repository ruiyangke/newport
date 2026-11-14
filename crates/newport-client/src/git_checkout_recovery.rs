//! Plan a read-only return to the saved checkout after a linked folder vanishes.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use newport_protocol::git::code;
use newport_protocol::{desktop::preferences::Project, git::Path};

fn path_bytes(bytes_b64: &str) -> Option<Vec<u8>> {
    let bytes = STANDARD.decode(bytes_b64).ok()?;
    (bytes.starts_with(b"/") && !bytes.contains(&0)).then_some(bytes)
}

pub fn unavailable(code: Option<&str>) -> bool {
    matches!(
        code,
        Some(code::REPO_NOT_FOUND | code::CHECKOUT_UNAVAILABLE)
    )
}

/// Only an explicit unavailable-checkout code is evidence that navigation helps.
/// Compare the requested checkout to the bookmark, so a canonicalized symlink
/// or subdirectory of the saved project cannot be mistaken for another checkout.
pub fn destination(
    code: Option<&str>,
    server: &str,
    project: &Project,
    opened_path: Option<&Path>,
) -> Option<Path> {
    if !unavailable(code) || server.is_empty() || project.server.to_string() != server {
        return None;
    }
    let root = path_bytes(&project.path.bytes_b64)?;
    let opened = path_bytes(&opened_path?.bytes_b64)?;
    (root != opened).then(|| project.path.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    const SERVER: &str = "00000000-0000-0000-0000-000000000001";
    fn path(bytes: &[u8]) -> Path {
        Path {
            bytes_b64: STANDARD.encode(bytes),
            display: "same lossy display".into(),
        }
    }
    #[test]
    fn returns_exact_saved_bytes_only_for_a_missing_different_checkout() {
        let project = Project {
            id: uuid::Uuid::from_u128(7),
            server: uuid::Uuid::parse_str(SERVER).unwrap(),
            name: "project".into(),
            path: path(b"/project/\xff"),
        };
        let linked = path(b"/linked/\xfe");
        let linked = Some(&linked);
        assert_eq!(
            destination(Some("REPO_NOT_FOUND"), SERVER, &project, linked),
            Some(project.path.clone())
        );
        assert_eq!(
            destination(Some("CHECKOUT_UNAVAILABLE"), SERVER, &project, linked),
            Some(project.path.clone())
        );
        assert!(
            destination(
                Some("REPO_NOT_FOUND"),
                SERVER,
                &project,
                Some(&project.path)
            )
            .is_none()
        );
        assert!(destination(Some("REPO_NOT_FOUND"), "another-server", &project, linked).is_none());
        for code in [
            None,
            Some("AUTH_FAILED"),
            Some("OUTCOME_UNKNOWN"),
            Some("STALE_SNAPSHOT"),
            Some("REPO_REPLACED"),
            Some("IO_ERROR"),
            Some("GIT_UNAVAILABLE"),
        ] {
            assert!(destination(code, SERVER, &project, linked).is_none());
        }
        for invalid in [
            path(b"relative"),
            path(b"/bad\0path"),
            Path {
                bytes_b64: "invalid!".into(),
                display: "/valid".into(),
            },
        ] {
            assert!(
                destination(Some("REPO_NOT_FOUND"), SERVER, &project, Some(&invalid)).is_none()
            );
            let mut bad = project.clone();
            bad.path = invalid;
            assert!(destination(Some("REPO_NOT_FOUND"), SERVER, &bad, linked).is_none());
        }
    }
}
