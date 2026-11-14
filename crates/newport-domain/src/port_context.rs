//! Observed service identity and useful process context for discovered listeners.
//!
//! Never infer a service from a port number or thread name, and never tokenize a
//! reconstructed command.

mod patterns;
pub use patterns::is_runtime_kind;
use patterns::{framework, python, runtime, service};

fn image_service(image: &str) -> Option<&'static str> {
    patterns::image_service(image_repository(image))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detail {
    pub label: &'static str,
    pub value: String,
}

fn image_repository(image: &str) -> &str {
    let repository = image.split('@').next().unwrap_or(image);
    // Only a colon after the final slash is a tag, not a registry port.
    match repository.rsplit_once(':') {
        Some((name, tag)) if !tag.contains('/') => name,
        _ => repository,
    }
}

/// Nix wrapProgram hides the original as `.name-wrapped`, appending `_`
/// on collisions. Repeated wrapping can nest this shape. This is a display
/// hint only: never rewrite the observed path or use it to address a process.
fn unwrapped_name(mut name: &str) -> &str {
    while let Some(inner) = name
        .strip_prefix('.')
        .and_then(|value| value.trim_end_matches('_').strip_suffix("-wrapped"))
        .filter(|value| !value.is_empty() && !value.contains('/'))
    {
        name = inner;
    }
    name
}

fn process_executable<'a>(path: Option<&'a str>, name: Option<&'a str>) -> &'a str {
    let name = path
        .map(|value| value.strip_suffix(" (deleted)").unwrap_or(value))
        .and_then(|value| value.rsplit('/').next())
        .filter(|value| !value.is_empty())
        .or(name)
        .unwrap_or_default();
    unwrapped_name(name)
}

fn workspace(directory: &str) -> Option<&str> {
    let value = directory.trim_end_matches('/');
    if !value.starts_with('/')
        || matches!(value, "/root" | "/home")
        || value
            .strip_prefix("/home/")
            .is_some_and(|rest| !rest.contains('/'))
        || [
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
        .any(|root| value == *root || value.strip_prefix(root).is_some_and(|s| s.starts_with('/')))
    {
        return None;
    }
    Some(value)
}

fn option<'a>(argv: &[&'a str], flag: &str) -> Option<&'a str> {
    let end = argv
        .iter()
        .position(|arg| *arg == "--")
        .unwrap_or(argv.len());
    let options = &argv[..end];
    if let Some(index) = options.iter().position(|arg| *arg == flag) {
        options.get(index + 1).copied()
    } else {
        options
            .iter()
            .find_map(|arg| arg.strip_prefix(flag)?.strip_prefix('='))
    }
}

/// Typed observed process context. Arguments are already split by the collector;
/// only the curated names and operational details below may become UI content.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProcessFacts<'a> {
    pub name: Option<&'a str>,
    pub executable: Option<&'a str>,
    pub executable_name: Option<&'a str>,
    pub directory: Option<&'a str>,
    pub arguments: &'a [String],
}

impl ProcessFacts<'_> {
    pub fn kinds(&self) -> Vec<&'static str> {
        let argv: Vec<_> = self.arguments.iter().skip(1).map(String::as_str).collect();
        process_kinds(
            process_executable(self.executable, self.executable_name.or(self.name)),
            &argv,
        )
    }

    pub fn label(&self) -> Option<String> {
        self.kinds()
            .into_iter()
            .find(|kind| !is_runtime_kind(kind))
            .or(self.executable_name.filter(|value| !value.is_empty()))
            .or_else(|| {
                let name = process_executable(self.executable, self.name);
                (!name.is_empty()).then_some(name)
            })
            .map(unwrapped_name)
            .map(str::to_owned)
    }

    pub fn details(&self) -> Vec<Detail> {
        let argv: Vec<_> = self.arguments.iter().skip(1).map(String::as_str).collect();
        curated_details(
            process_executable(self.executable, self.executable_name.or(self.name)),
            &argv,
            self.directory,
        )
    }
}

fn curated_details(exe: &str, argv: &[&str], directory: Option<&str>) -> Vec<Detail> {
    let mut details = Vec::new();
    let mut add = |label, value: Option<&str>| {
        if let Some(value) = value.filter(|value| {
            !value.is_empty() && !value.starts_with('-') && !value.contains(['\n', '\r'])
        }) {
            details.push(Detail {
                label,
                value: value.to_owned(),
            });
        }
    };
    match exe {
        "postgres" => add("Data directory", option(argv, "-D")),
        "mysqld" | "mariadbd" => {
            add("Data directory", option(argv, "--datadir"));
            add("Configuration", option(argv, "--defaults-file"));
        }
        "nginx" => add("Configuration", option(argv, "-c")),
        "sshd" => add("Configuration", option(argv, "-f")),
        "redis-server" => add(
            "Configuration",
            argv.first().copied().filter(|s| s.ends_with(".conf")),
        ),
        "caddy" => add("Configuration", option(argv, "--config")),
        _ => {
            if python(exe) {
                let module = option(argv, "-m").filter(|value| {
                    !value.is_empty()
                        && value
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
                });
                if module.is_some() {
                    add("Module", module);
                } else {
                    add(
                        "Script",
                        argv.first().copied().filter(|s| s.ends_with(".py")),
                    );
                }
            } else if exe == "java" {
                add("Application", option(argv, "-jar"));
            } else if exe == "dotnet" {
                add(
                    "Application",
                    argv.first().copied().filter(|s| s.ends_with(".dll")),
                );
            } else if matches!(exe, "ruby" | "php") {
                add(
                    "Script",
                    argv.first()
                        .copied()
                        .filter(|s| s.ends_with(".rb") || s.ends_with(".php")),
                );
            }
            if !exe.is_empty() && exe != "MainThread" {
                let runtime = python(exe)
                    || matches!(
                        exe,
                        "node"
                            | "nodejs"
                            | "bun"
                            | "deno"
                            | "ruby"
                            | "php"
                            | "java"
                            | "dotnet"
                            | "uvicorn"
                            | "gunicorn"
                    );
                add(
                    if runtime {
                        "Project directory"
                    } else {
                        "Working directory"
                    },
                    directory.and_then(workspace),
                );
            }
        }
    }
    details.truncate(2);
    details
}

pub fn container_kinds(image: Option<&str>) -> Vec<&'static str> {
    let mut result: Vec<_> = image.and_then(image_service).into_iter().collect();
    result.push("Docker");
    result
}

fn process_kinds(executable: &str, argv: &[&str]) -> Vec<&'static str> {
    let mut result = Vec::new();
    let mut add = |kind| {
        if let Some(kind) = kind
            && !result.contains(&kind)
            && result.len() < 3
        {
            result.push(kind);
        }
    };
    let exe = executable.to_lowercase();
    if exe == "kubectl" {
        add(Some(patterns::kubectl(argv)));
        return result;
    }
    add(service(&exe));
    let is_python = python(&exe);
    if is_python && let Some(index) = argv.iter().position(|arg| *arg == "-m") {
        add(argv.get(index + 1).and_then(|module| framework(module)));
    }
    for arg in argv {
        if arg.starts_with('-') {
            continue;
        }
        let base = unwrapped_name(arg.rsplit('/').next().unwrap_or(arg));
        add(patterns::framework_argument(base, argv));
    }
    add(framework(&exe));
    add(if is_python {
        Some("Python")
    } else {
        runtime(&exe)
    });
    result
}

/// Condense only paths that belong to the same observed process owner.
pub fn tildify(path: &str, users: &[Option<&str>]) -> String {
    let Some(user) = users
        .iter()
        .flatten()
        .copied()
        .find(|user| !user.is_empty())
    else {
        return path.to_owned();
    };
    if users
        .iter()
        .flatten()
        .any(|other| !other.is_empty() && *other != user)
    {
        return path.to_owned();
    }
    let home = if user == "root" {
        "/root".to_owned()
    } else {
        format!("/home/{user}")
    };
    if path == home {
        "~".to_owned()
    } else if let Some(rest) = path
        .strip_prefix(&home)
        .filter(|rest| rest.starts_with('/'))
    {
        format!("~{rest}")
    } else {
        path.to_owned()
    }
}

pub fn short_path(path: &str, users: &[Option<&str>], budget: usize) -> String {
    if !path.starts_with('/') {
        return path.to_owned();
    }
    let tilded = tildify(path, users);
    if tilded.encode_utf16().count() <= budget {
        return tilded;
    }
    let parts: Vec<_> = tilded.split('/').filter(|part| !part.is_empty()).collect();
    let mut kept = parts[parts.len().saturating_sub(2)..].join("/");
    for part in parts[..parts.len().saturating_sub(2)].iter().rev() {
        let next = format!("{part}/{kept}");
        if next.encode_utf16().count().saturating_add(2) > budget {
            break;
        }
        kept = next;
    }
    format!("…/{kept}")
}

#[cfg(test)]
mod tests;
