use super::Options;
use crate::{command, process, remote};
use anyhow::{Result, ensure};
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::Command,
};

pub(super) enum Target {
    Container(String),
    Ssh { host: String, identity: PathBuf },
}

impl Target {
    pub(super) fn new(options: &Options) -> Result<Self> {
        match (&options.container, &options.ssh, &options.ssh_identity) {
            (Some(name), None, None) => {
                ensure!(
                    !name.is_empty()
                        && !name.starts_with('-')
                        && name
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b)),
                    "Use a Docker container name or ID"
                );
                Ok(Self::Container(name.clone()))
            }
            (None, Some(host), Some(identity)) => {
                ensure!(
                    valid_host(host),
                    "Use an isolated SSH test destination as user@host"
                );
                ensure!(
                    identity.is_absolute() && identity.is_file(),
                    "Use an absolute SSH identity file"
                );
                Ok(Self::Ssh {
                    host: host.clone(),
                    identity: identity.clone(),
                })
            }
            _ => anyhow::bail!("Choose a container or an isolated SSH test VM and identity"),
        }
    }

    pub(super) fn exec(
        &self,
        args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    ) -> Result<Command> {
        match self {
            Self::Container(name) => {
                let mut cmd = command(remote::docker());
                cmd.args(["exec", "--user", "newport", name]);
                cmd.args(args);
                Ok(cmd)
            }
            Self::Ssh { host, identity } => {
                let arguments = args
                    .into_iter()
                    .map(|arg| quoted(arg.as_ref()))
                    .collect::<Result<Vec<_>>>()?;
                let mut cmd = ssh("ssh", identity);
                cmd.arg(host).arg(arguments.join(" "));
                Ok(cmd)
            }
        }
    }

    pub(super) fn copy(&self, source: &Path, destination: &str) -> Result<()> {
        match self {
            Self::Container(name) => {
                process::run(
                    command(remote::docker())
                        .arg("cp")
                        .arg(source)
                        .arg(format!("{name}:{destination}")),
                )?;
                process::run(command(remote::docker()).args([
                    "exec",
                    "--user",
                    "root",
                    name,
                    "chmod",
                    "0755",
                    destination,
                ]))
            }
            Self::Ssh { host, identity } => {
                process::run(
                    ssh("scp", identity)
                        .arg("--")
                        .arg(source)
                        .arg(format!("{host}:{destination}")),
                )?;
                process::run(&mut self.exec(["chmod", "0755", destination])?)
            }
        }
    }
}

fn ssh(program: &str, identity: &Path) -> Command {
    let mut cmd = command(program);
    cmd.args([
        "-o",
        "BatchMode=yes",
        "-o",
        "IdentitiesOnly=yes",
        "-o",
        "ConnectTimeout=10",
    ])
    .arg("-i")
    .arg(identity);
    cmd
}

fn valid_host(value: &str) -> bool {
    let Some((user, host)) = value.split_once('@') else {
        return false;
    };
    !user.is_empty()
        && !host.is_empty()
        && !user.starts_with('-')
        && !host.starts_with('-')
        && user
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-:[]".contains(&b))
}

fn quoted(value: &OsStr) -> Result<String> {
    let value = value
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("SSH test arguments must be UTF-8"))?;
    ensure!(
        !value.contains('\0'),
        "SSH test arguments cannot contain NUL"
    );
    Ok(format!("'{}'", value.replace('\'', "'\\''")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_destinations_cannot_be_options_or_shell_fragments() {
        for value in [
            "newport@198.51.100.7",
            "newport@[::1]",
            "test-user@lab.example",
        ] {
            assert!(valid_host(value));
        }
        for value in [
            "-oProxyCommand=evil",
            "newport@host;echo",
            "@host",
            "newport@",
            "newport@-option",
            "newport@host@other",
            "newport@host\ncommand",
        ] {
            assert!(!valid_host(value));
        }
    }

    #[test]
    fn remote_arguments_preserve_spaces_quotes_and_shell_metacharacters() {
        let values = [
            "",
            "two words",
            "a'b",
            "$(touch /tmp/not-executed)",
            "`echo secret`",
            "x;y",
            "line\nline",
        ];
        for value in values {
            let script = format!("printf '%s' {}", quoted(OsStr::new(value)).unwrap());
            let output = Command::new("sh").args(["-c", &script]).output().unwrap();
            assert!(output.status.success());
            assert_eq!(output.stdout, value.as_bytes());
        }
    }
}
