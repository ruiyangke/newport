//! Project markers and Git worktrees observed without invoking repository code.
use super::{Host, MAX_FIELD, Project};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read},
    os::unix::fs::OpenOptionsExt,
    time::{Duration, Instant},
};

/// Directories that hold projects but are never one themselves.
fn boundary(dir: &str) -> bool {
    if let Some(rest) = dir.strip_prefix("/home/") {
        return !rest.contains('/');
    }
    if dir.starts_with("/root/") {
        return false;
    }
    matches!(
        dir,
        "" | "/" | "/root" | "/tmp" | "/var" | "/srv" | "/opt" | "/mnt" | "/media"
    )
}

/// System directories never belong to a project.
fn system(dir: &str) -> bool {
    [
        "/usr",
        "/bin",
        "/sbin",
        "/etc",
        "/proc",
        "/sys",
        "/dev",
        "/run",
        "/nix/store",
        "/var/lib",
        "/var/log",
    ]
    .iter()
    .any(|prefix| {
        dir.strip_prefix(prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    })
}

const VCS: [&str; 9] = [
    ".git",
    ".hg",
    ".jj",
    ".sl",
    ".fslckout",
    "_FOSSIL_",
    ".bzr",
    "_darcs",
    ".svn",
];
const PROJECT_FILES: [&str; 27] = [
    "package.json",
    "Cargo.toml",
    "go.mod",
    "pyproject.toml",
    "setup.py",
    "requirements.txt",
    "Gemfile",
    "composer.json",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "mix.exs",
    "deno.json",
    "deno.jsonc",
    "flake.nix",
    "stack.yaml",
    "dune-project",
    "build.sbt",
    "project.clj",
    "deps.edn",
    "pubspec.yaml",
    "Package.swift",
    "CMakeLists.txt",
    "compose.yaml",
    "compose.yml",
    "docker-compose.yml",
    "docker-compose.yaml",
];

const MAX_OPERATIONS: usize = 65536;
const MAX_DEPTH: usize = 32;
const CACHE_BYTES: usize = 1 << 20;
pub(super) const TIME_LIMIT: Duration = Duration::from_secs(2);

pub(super) struct Resolved {
    pub project: Option<Project>,
    pub limitation: Option<io::Error>,
}

struct Worktree {
    directory: String,
    repository: String,
    branch: Option<String>,
    main: bool,
}

pub(super) struct Resolver<'a> {
    host: Host,
    deadline: Instant,
    cancelled: &'a dyn Fn() -> bool,
    operations: usize,
    cache_bytes: usize,
    markers: BTreeMap<String, bool>,
    projects: BTreeMap<String, Option<Project>>,
}

impl<'a> Resolver<'a> {
    pub fn new(root: &std::path::Path, deadline: Instant, cancelled: &'a dyn Fn() -> bool) -> Self {
        Self {
            host: Host::new(root),
            deadline: deadline.min(Instant::now() + TIME_LIMIT),
            cancelled,
            operations: 0,
            cache_bytes: 0,
            markers: BTreeMap::new(),
            projects: BTreeMap::new(),
        }
    }

    fn check(&mut self) -> io::Result<()> {
        if (self.cancelled)() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Project discovery cancelled",
            ));
        }
        if Instant::now() >= self.deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Project discovery timed out",
            ));
        }
        if self.operations == MAX_OPERATIONS {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "Project filesystem operation limit reached",
            ));
        }
        self.operations += 1;
        Ok(())
    }

    pub fn resolve(&mut self, directory: &str) -> io::Result<Resolved> {
        self.check()?;
        if !directory.starts_with('/')
            || directory.len() > MAX_FIELD
            || directory.ends_with(" (deleted)")
            || directory.chars().any(char::is_control)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Project directory cannot be inspected",
            ));
        }
        if system(directory) || boundary(directory) {
            return Ok(Resolved {
                project: None,
                limitation: None,
            });
        }
        let directory = self.real(directory)?;
        if let Some(project) = self.projects.get(&directory) {
            return Ok(Resolved {
                project: project.clone(),
                limitation: None,
            });
        }
        let Some((root, marker)) = self.root(&directory)? else {
            self.projects.insert(directory, None);
            return Ok(Resolved {
                project: None,
                limitation: None,
            });
        };
        let mut project = Project {
            root,
            marker,
            ..Project::default()
        };
        let mut limitation = None;
        if project.marker == ".git" {
            match self.worktree(&project.root) {
                Ok(Some(worktree)) => {
                    project.worktree = Some(worktree.directory);
                    project.root = worktree.repository;
                    project.branch = worktree.branch;
                    project.main = Some(worktree.main);
                }
                Ok(None) => {}
                Err(error) => limitation = Some(error),
            }
        }
        if limitation.is_none() {
            self.projects.insert(directory, Some(project.clone()));
        }
        Ok(Resolved {
            project: Some(project),
            limitation,
        })
    }

    // Prefer an explicit root, then the nearest VCS, topmost manifest, or Makefile chain.
    fn root(&mut self, dir: &str) -> io::Result<Option<(String, String)>> {
        if system(dir) {
            return Ok(None);
        }
        let (mut local, mut vcs, mut top, mut make) = (None, None, None, None);
        let mut make_done = false;
        let mut current = dir.trim_end_matches('/').to_owned();
        for depth in 0..=MAX_DEPTH {
            if boundary(&current) {
                break;
            }
            if depth == MAX_DEPTH {
                return Err(io::Error::new(
                    io::ErrorKind::FileTooLarge,
                    "Project ancestor limit reached",
                ));
            }
            if local.is_none() && self.exists(&format!("{current}/.projectile"))? {
                local = Some(current.clone());
            }
            if vcs.is_none()
                && let Some(marker) = self.marker(&current, &VCS)?
            {
                vcs = Some((current.clone(), marker));
            }
            if let Some(marker) = self.marker(&current, &PROJECT_FILES)? {
                top = Some((current.clone(), marker));
            }
            if !make_done {
                if self.exists(&format!("{current}/Makefile"))? {
                    make = Some(current.clone());
                } else if make.is_some() {
                    make_done = true;
                }
            }
            match current.rfind('/') {
                Some(slash) => current.truncate(slash),
                None => break,
            }
        }
        Ok(local
            .map(|root| (root, ".projectile".to_owned()))
            .or(vcs)
            .or(top)
            .or_else(|| make.map(|root| (root, "Makefile".to_owned()))))
    }

    fn marker(&mut self, dir: &str, markers: &[&str]) -> io::Result<Option<String>> {
        for marker in markers {
            if self.exists(&format!("{dir}/{marker}"))? {
                return Ok(Some((*marker).into()));
            }
        }
        Ok(None)
    }

    fn exists(&mut self, logical: &str) -> io::Result<bool> {
        self.check()?;
        if let Some(found) = self.markers.get(logical) {
            return Ok(*found);
        }
        let found = match fs::metadata(self.host.path(logical)) {
            Ok(_) => true,
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(error),
        };
        if logical.len() <= CACHE_BYTES - self.cache_bytes {
            self.cache_bytes += logical.len();
            self.markers.insert(logical.into(), found);
        }
        Ok(found)
    }

    fn real(&mut self, logical: &str) -> io::Result<String> {
        self.check()?;
        let real = fs::canonicalize(self.host.path(logical))?;
        let inside = real.strip_prefix(&self.host.root).map_err(|_| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Project path is outside the observed filesystem",
            )
        })?;
        let text = inside.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "Project path is not UTF-8")
        })?;
        if text.len() >= MAX_FIELD {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "Project path limit reached",
            ));
        }
        Ok(format!("/{text}"))
    }

    fn read(&mut self, logical: &str) -> io::Result<Option<String>> {
        self.check()?;
        let file = match fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
            .open(self.host.path(logical))
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Project metadata is not a regular file",
            ));
        }
        let mut bytes = Vec::new();
        file.take(
            u64::try_from(MAX_FIELD + 1)
                .map_err(|_| io::Error::other("Invalid project byte limit"))?,
        )
        .read_to_end(&mut bytes)?;
        self.check()?;
        if bytes.len() > MAX_FIELD {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "Project metadata byte limit reached",
            ));
        }
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    fn worktree(&mut self, dir: &str) -> io::Result<Option<Worktree>> {
        self.check()?;
        let dot_git = format!("{dir}/.git");
        let metadata = fs::metadata(self.host.path(&dot_git))?;
        let git_dir = if metadata.is_dir() {
            dot_git
        } else {
            let Some(contents) = self.read(&dot_git)? else {
                return Ok(None);
            };
            let named = contents
                .lines()
                .next()
                .and_then(|line| line.strip_prefix("gitdir: "))
                .filter(|named| !named.is_empty())
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Invalid Git directory reference",
                    )
                })?;
            self.relative(dir, named)?
        };
        let here = self.real(dir)?;
        let (top, main) = match self.read(&format!("{git_dir}/commondir"))? {
            Some(common) => {
                let common = common
                    .lines()
                    .next()
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidData, "Invalid Git common directory")
                    })?;
                let common = self.relative(&git_dir, common)?;
                (
                    common.strip_suffix("/.git").unwrap_or(&common).to_owned(),
                    false,
                )
            }
            None => (here.clone(), true),
        };
        let branch = self.head(&git_dir)?;
        Ok(Some(Worktree {
            directory: here,
            repository: top,
            branch,
            main,
        }))
    }

    fn head(&mut self, git_dir: &str) -> io::Result<Option<String>> {
        self.check()?;
        let path = format!("{git_dir}/HEAD");
        if fs::symlink_metadata(self.host.path(&path)).is_ok_and(|metadata| metadata.is_symlink()) {
            let target = fs::read_link(self.host.path(&path))?;
            let target = target.to_str().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "Git HEAD link is not UTF-8")
            })?;
            let reference = target
                .strip_prefix(&format!("{git_dir}/"))
                .unwrap_or(target);
            if valid_reference(reference) {
                return Ok(Some(
                    reference
                        .strip_prefix("refs/heads/")
                        .unwrap_or(reference)
                        .into(),
                ));
            }
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid Git HEAD link",
            ));
        }
        let head = self.read(&path)?.unwrap_or_default();
        let head = head.lines().next().unwrap_or_default();
        let branch = match head.strip_prefix("ref: ") {
            Some(name) if valid_reference(name) => {
                Some(name.strip_prefix("refs/heads/").unwrap_or(name).to_owned())
            }
            None if matches!(head.len(), 40 | 64)
                && head.bytes().all(|byte| byte.is_ascii_hexdigit()) =>
            {
                Some(head[..7].to_owned())
            }
            None if head.is_empty() => None,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid Git HEAD",
                ));
            }
        };
        Ok(branch)
    }

    fn relative(&mut self, base: &str, path: &str) -> io::Result<String> {
        if path.starts_with('/') {
            self.real(path)
        } else {
            self.real(&format!("{base}/{path}"))
        }
    }
}

fn valid_reference(name: &str) -> bool {
    name.starts_with("refs/")
        && name.len() <= MAX_FIELD
        && !name.ends_with('.')
        && !name.contains("..")
        && !name.contains("@{")
        && !name
            .bytes()
            .any(|byte| byte.is_ascii_control() || b" ~^:?*[\\".contains(&byte))
        && name.split('/').all(|component| {
            !component.is_empty() && !component.starts_with('.') && !component.ends_with(".lock")
        })
}

#[cfg(test)]
mod tests;
