//! Shared repository creation input validation. Bookmark recovery never repeats Git creation.
use base64::Engine as _;
use serde_json::{Value, json};
pub fn parameters(
    clone_repo: bool,
    name: &str,
    path: &str,
    url: &str,
    branch: &str,
) -> Result<Value, &'static str> {
    if name.trim().is_empty() || name.trim().len() > 256 || name.chars().any(char::is_control) {
        return Err("Enter a project name of up to 256 bytes without control characters.");
    }
    if !path.starts_with('/') || path.contains('\0') {
        return Err("Enter an absolute path on the server.");
    }
    if branch.contains('\0') || (!clone_repo && branch.trim().is_empty()) {
        return Err("Enter an initial branch.");
    }
    let mut params = json!({"path":{"display":path,"bytesB64":base64::engine::general_purpose::STANDARD.encode(path)}});
    if clone_repo {
        if url.trim().is_empty() || url.contains('\0') {
            return Err("Enter a repository URL.");
        }
        params["url"] = json!(url.trim());
        if !branch.trim().is_empty() {
            params["branch"] = json!(branch.trim());
        }
    } else {
        params["initialBranch"] = json!(branch.trim());
    }
    Ok(params)
}

#[cfg(test)]
mod tests {
    use super::parameters;
    use base64::Engine as _;
    #[test]
    fn creation_payloads_keep_path_bytes_and_distinguish_clone_and_init() {
        let init = parameters(false, "Project", "/repo é", "", "main").unwrap();
        assert_eq!(init["initialBranch"], "main");
        assert!(init.get("url").is_none());
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(init["path"]["bytesB64"].as_str().unwrap())
                .unwrap(),
            "/repo é".as_bytes()
        );
        let clone = parameters(true, "Project", "/repo", " /origin.git ", "").unwrap();
        assert_eq!(clone["url"], "/origin.git");
        assert!(clone.get("branch").is_none());
        assert!(parameters(false, "Project", "relative", "", "main").is_err());
        assert!(parameters(false, "Project", "/repo", "", "").is_err());
        assert!(parameters(true, "Project", "/repo", "", "").is_err());
        assert!(parameters(false, "bad\nname", "/repo", "", "main").is_err());
    }
}
