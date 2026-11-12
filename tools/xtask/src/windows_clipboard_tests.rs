//! Native fixture processes use their own noninteractive clipboard station.
use crate::util;
use anyhow::{Result, ensure};
use clap::{Args, ValueEnum};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Mode {
    Static,
    Render,
    Privacy,
    Destroy,
}
impl Mode {
    fn name(self) -> &'static str {
        match self {
            Self::Static => "static",
            Self::Render => "render",
            Self::Privacy => "privacy",
            Self::Destroy => "destroy",
        }
    }
}

#[derive(Args, Debug)]
pub struct Options {
    /// Exact native test executable produced by cargo test --no-run.
    #[arg(long)]
    test_executable: PathBuf,
    /// SHA-256 from the candidate artifact record.
    #[arg(long)]
    expected_sha256: String,
    #[arg(long, value_enum)]
    mode: Mode,
    /// Allowlisted short fixture name, required for static mode.
    #[arg(long)]
    static_test: Option<String>,
    /// Fresh absolute directory for isolated profile, temporary files and evidence.
    #[arg(long)]
    output_directory: PathBuf,
    #[arg(long, default_value_t = 45, value_parser = clap::value_parser!(u32).range(15..=120))]
    timeout_seconds: u32,
    /// Existing logged-on session whose ordinary token the SYSTEM harness duplicates.
    #[arg(long, requires = "expected_user_sid", value_parser = clap::value_parser!(u32).range(1..))]
    user_session_id: Option<u32>,
    /// Exact measured user SID for the selected session; never inferred by the harness.
    #[arg(long, requires = "user_session_id")]
    expected_user_sid: Option<String>,
}

fn command(root: &Path, options: &Options) -> Command {
    let mut command = Command::new("powershell.exe");
    command
        // Apply only to this reviewed fixture process; do not change the
        // machine or user's persistent PowerShell execution policy.
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(root.join("tools/xtask/native/windows-clipboard/run-clipboard-station.ps1"))
        .arg("-TestExecutable")
        .arg(&options.test_executable)
        .arg("-ExpectedSha256")
        .arg(&options.expected_sha256)
        .arg("-Mode")
        .arg(options.mode.name())
        .arg("-OutputDirectory")
        .arg(&options.output_directory)
        .arg("-TimeoutSeconds")
        .arg(options.timeout_seconds.to_string());
    if let Some(test) = &options.static_test {
        command.arg("-StaticTest").arg(test);
    }
    if let (Some(session), Some(sid)) = (options.user_session_id, &options.expected_user_sid) {
        command
            .arg("-UserSessionId")
            .arg(session.to_string())
            .arg("-ExpectedUserSid")
            .arg(sid);
    }
    command
}

pub fn run(options: Options) -> Result<()> {
    ensure!(cfg!(windows), "Native clipboard fixtures require Windows");
    ensure!(
        options.test_executable.is_absolute() && options.test_executable.is_file(),
        "Select an existing absolute test executable"
    );
    ensure!(
        options.output_directory.is_absolute() && !options.output_directory.exists(),
        "Select a fresh absolute output directory"
    );
    ensure!(
        options.expected_sha256.len() == 64
            && options
                .expected_sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit()),
        "Expected SHA-256 must contain 64 hexadecimal digits"
    );
    ensure!(
        matches!(options.mode, Mode::Static) == options.static_test.is_some(),
        "Only static mode requires --static-test"
    );
    ensure!(
        options.user_session_id.is_some() == options.expected_user_sid.is_some(),
        "Broker mode requires both --user-session-id and --expected-user-sid"
    );
    util::run(util::without_release_secrets(&mut command(
        &crate::root(),
        &options,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixture_paths_remain_individual_arguments_without_shell_interpolation() {
        let options = Options {
            test_executable: "fixture path/test.exe".into(),
            expected_sha256: "a".repeat(64),
            mode: Mode::Static,
            static_test: Some(
                "unsupported_file_publication_preserves_the_current_clipboard".into(),
            ),
            output_directory: "evidence path/fresh".into(),
            timeout_seconds: 45,
            user_session_id: None,
            expected_user_sid: None,
        };
        let command = command(Path::new("workspace path"), &options);
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(command.get_program(), "powershell.exe");
        assert_eq!(
            args[6],
            Path::new(
                "workspace path/tools/xtask/native/windows-clipboard/run-clipboard-station.ps1"
            )
        );
        assert_eq!(args[8], options.test_executable);
        assert_eq!(args[14], options.output_directory);
        assert_eq!(&args[3..5], ["-ExecutionPolicy", "Bypass"]);
        assert!(!args.iter().any(|arg| *arg == "-Command"));
    }

    #[test]
    fn broker_identity_requires_an_explicit_session_and_user_pair() {
        use clap::Parser;
        #[derive(Parser)]
        struct Cli {
            #[command(flatten)]
            options: Options,
        }
        let base = [
            "fixture",
            "--test-executable",
            "fixture.exe",
            "--expected-sha256",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "--mode",
            "render",
            "--output-directory",
            "fresh",
        ];
        assert!(Cli::try_parse_from(base).is_ok());
        for extra in [
            vec!["--user-session-id", "7"],
            vec!["--expected-user-sid", "S-1-5-21-1-2-3-1001"],
            vec![
                "--user-session-id",
                "0",
                "--expected-user-sid",
                "S-1-5-21-1-2-3-1001",
            ],
        ] {
            assert!(Cli::try_parse_from(base.into_iter().chain(extra)).is_err());
        }
        let cli = Cli::try_parse_from(base.into_iter().chain([
            "--user-session-id",
            "7",
            "--expected-user-sid",
            "S-1-5-21-1-2-3-1001",
        ]))
        .unwrap();
        let command = command(Path::new("workspace"), &cli.options);
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(
            &args[args.len() - 4..],
            [
                "-UserSessionId",
                "7",
                "-ExpectedUserSid",
                "S-1-5-21-1-2-3-1001"
            ]
        );
    }
}
