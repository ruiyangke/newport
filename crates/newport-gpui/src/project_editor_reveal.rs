//! Locate a retained source buffer without making the navigator own its lifetime.
use super::*;

fn parents(root: &str, path: &str) -> Option<Vec<String>> {
    let prefix = format!("{}/", root.trim_end_matches('/'));
    let relative = path.strip_prefix(&prefix)?;
    let parts: Vec<_> = relative.split('/').collect();
    if parts.len() > 33
        || parts.iter().any(|p| matches!(*p, "" | "." | ".." | ".git"))
        || relative.chars().any(char::is_control)
    {
        return None;
    }
    let mut result = vec![root.to_owned()];
    let mut parent = root.trim_end_matches('/').to_owned();
    for part in &parts[..parts.len() - 1] {
        parent.push('/');
        parent.push_str(part);
        result.push(parent.clone());
    }
    Some(result)
}

impl CodeWorkspace {
    pub(crate) fn reveal_active(
        &mut self,
        explicit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reveal_target = None;
        self.reveal_notice = None;
        self.reveal_refresh.clear();
        if !self.available || !self.visible || self.reviewing {
            return;
        }
        let Some(path) = self
            .active
            .and_then(|i| self.buffers.get(i))
            .map(|b| b.path.clone())
        else {
            return;
        };
        if explicit {
            self.files = true;
            self.navigator.panel = 0;
            self.show_tree = true;
            // Explicit locate also retries a missing or renamed file against fresh listings.
            if let Some(parents) = parents(&self.root, &path) {
                self.reveal_refresh.extend(parents);
            }
        }
        if !self.files || !self.show_tree {
            return;
        }
        self.reveal_notice = None;
        self.reveal_target = Some(path);
        self.continue_reveal(window, cx);
        if explicit {
            self.focus_tree(window, cx);
        }
        cx.notify();
    }

    pub(super) fn continue_reveal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.reveal_target.clone() else {
            return;
        };
        if !self.available
            || !self.visible
            || !self.files
            || !self.show_tree
            || self.reviewing
            || !self
                .active
                .and_then(|i| self.buffers.get(i))
                .is_some_and(|b| b.path == target)
        {
            self.reveal_target = None;
            return;
        }
        let Some(parents) = parents(&self.root, &target) else {
            self.reveal_target = None;
            self.reveal_notice = Some("This path cannot be shown in the project explorer.".into());
            return;
        };
        for (i, parent) in parents.iter().enumerate() {
            if i > 0
                && !self.directories[&parents[i - 1]]
                    .iter()
                    .any(|e| e["path"].as_str() == Some(parent) && e["kind"] == "directory")
            {
                self.reveal_target = None;
                self.reveal_notice = Some("The file's folder is no longer listed. Your open buffer is kept. Use Reveal in Explorer to retry.".into());
                return;
            }
            self.expanded.insert(parent.clone());
            if self.loading.contains(parent) {
                self.reveal_refresh.remove(parent);
                return;
            }
            if self.reveal_refresh.contains(parent) || !self.directories.contains_key(parent) {
                if self.loading.len() >= 8 {
                    return;
                }
                // Keep the existing rows visible while refreshing; no loading row or blank tree.
                self.reveal_refresh.remove(parent);
                // One level at a time: never crawl siblings or recursively scan the repository.
                self.list(parent.clone(), window, cx);
                return;
            }
        }
        let index = self.tree_rows().iter().position(|row| row.path == target);
        self.reveal_target = None;
        if let Some(index) = index {
            self.tree_cursor = Some(target);
            self.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
        } else {
            self.reveal_notice = Some("The file is not in the current explorer listing. Your open buffer is kept. Use Reveal in Explorer to retry.".into());
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::parents;
    #[test]
    fn reveal_only_visits_ancestors_inside_the_exact_root() {
        assert_eq!(
            parents("/repo", "/repo/src/deep/file.rs").unwrap(),
            ["/repo", "/repo/src", "/repo/src/deep"]
        );
        assert_eq!(parents("/", "/src/file.rs").unwrap(), ["/", "/src"]);
        for path in [
            "/repository/file.rs",
            "/repo/../outside",
            "/repo/.git/config",
            "/repo//file",
            "/repo/",
        ] {
            assert!(parents("/repo", path).is_none(), "{path}");
        }
    }
}
