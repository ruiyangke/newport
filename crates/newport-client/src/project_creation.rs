//! Shared repository creation input validation. Bookmark recovery never repeats Git creation.
use crate::backend::RepositoryCreation;
pub fn parameters(
    clone_repo: bool,
    name: &str,
    path: &str,
    url: &str,
    branch: &str,
) -> Result<RepositoryCreation, &'static str> {
    if name.trim().is_empty() {
        return Err("Enter a project name.");
    }
    if name.trim().len() > 256 || name.chars().any(char::is_control) {
        return Err("Enter a project name of up to 256 bytes without control characters.");
    }
    if !path.starts_with('/') || path.contains('\0') {
        return Err("Enter an absolute path on the server.");
    }
    if branch.contains('\0') || (!clone_repo && branch.trim().is_empty()) {
        return Err("Enter an initial branch.");
    }
    let path = newport_protocol::git::Path::new(path.as_bytes());
    if clone_repo {
        if url.trim().is_empty() || url.contains('\0') {
            return Err("Enter a repository URL.");
        }
        Ok(RepositoryCreation::Clone {
            path,
            url: url.trim().into(),
            branch: (!branch.trim().is_empty()).then(|| branch.trim().to_owned()),
            bare: false,
        })
    } else {
        Ok(RepositoryCreation::Init {
            path,
            initial_branch: branch.trim().into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::parameters;
    #[test]
    fn creation_payloads_keep_path_bytes_and_distinguish_clone_and_init() {
        let init = parameters(false, "Project", "/repo é", "", "main").unwrap();
        let crate::backend::RepositoryCreation::Init {
            path,
            initial_branch,
        } = init
        else {
            panic!("init");
        };
        assert_eq!(initial_branch, "main");
        assert_eq!(path.decode().unwrap(), "/repo é".as_bytes());
        let clone = parameters(true, "Project", "/repo", " /origin.git ", "").unwrap();
        let crate::backend::RepositoryCreation::Clone {
            url, branch, bare, ..
        } = clone
        else {
            panic!("clone");
        };
        assert_eq!(url, "/origin.git");
        assert!(branch.is_none());
        assert!(!bare);
        assert!(parameters(false, "Project", "relative", "", "main").is_err());
        assert!(parameters(false, "Project", "/repo", "", "").is_err());
        assert!(parameters(true, "Project", "/repo", "", "").is_err());
        assert!(parameters(false, "bad\nname", "/repo", "", "main").is_err());
    }
}
