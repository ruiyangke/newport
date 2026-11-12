use super::*;

fn options(text: &str) -> HashMap<String, Vec<String>> {
    parse(text)
}

#[test]
fn only_what_the_config_changes_is_reported() {
    let defaults = options(
        "user me\nhostname host.test\nport 22\nidentityfile ~/.ssh/id_rsa\nidentityfile ~/.ssh/id_ed25519\n",
    );
    let configured = options(
        "user me\nhostname host.test\nport 22\nidentityfile ~/.ssh/id_rsa\nidentityfile ~/.ssh/id_ed25519\nproxycommand gateway ssh --stdio %h\n",
    );
    assert_eq!(
        difference(&configured, &defaults, Some(Path::new("/home/me"))),
        Resolved {
            proxy_command: Some("gateway ssh --stdio %h".into()),
            ..Resolved::default()
        }
    );
}

#[test]
fn proxy_none_means_no_proxy_and_host_names_compare_without_case() {
    let defaults = options("hostname Host.Test\n");
    let configured = options("hostname host.test\nproxycommand none\nproxyjump none\n");
    assert_eq!(
        difference(&configured, &defaults, None),
        Resolved::default()
    );
}

#[test]
fn key_files_expand_home_and_user_and_skip_other_tokens() {
    let home = Some(Path::new("/home/me"));
    assert_eq!(
        expand("~/.ssh/%u_key", home, Some("me")).as_deref(),
        Some("/home/me/.ssh/me_key")
    );
    assert_eq!(
        expand("%d/keys/id", home, None).as_deref(),
        Some("/home/me/keys/id")
    );
    assert_eq!(
        expand("/keys/100%%", home, None).as_deref(),
        Some("/keys/100%")
    );
    assert_eq!(expand("~/.ssh/%h", home, Some("me")), None);
    assert_eq!(expand("~/.ssh/id", None, None), None);
}

#[test]
fn a_host_that_could_be_read_as_an_option_is_refused() {
    for host in [
        "",
        "-oProxyCommand=touch /tmp/x",
        "two words",
        "tab\there",
        "line\n",
    ] {
        assert!(!valid_host(host), "{host:?}");
    }
    for host in ["app.box.me.gw", "user@host", "::1", "alias"] {
        assert!(valid_host(host), "{host:?}");
    }
}

#[tokio::test]
async fn no_ssh_is_unavailable_and_an_invalid_host_never_runs() {
    let missing = Options {
        ssh: None,
        config: None,
        timeout: TIMEOUT,
    };
    assert_eq!(
        resolve_with("host", &missing).await,
        Err(Error::Unavailable)
    );
    let gone = Options {
        ssh: Some("/nonexistent/ssh".into()),
        ..missing.clone()
    };
    assert_eq!(resolve_with("host", &gone).await, Err(Error::Unavailable));
    assert_eq!(
        resolve_with("-oProxyCommand=x", &gone).await,
        Err(Error::InvalidHost)
    );
}

/// Runs the installed `ssh -G` against a temporary config.
#[cfg(unix)]
mod openssh {
    use super::*;

    struct Config {
        directory: tempfile::TempDir,
    }

    impl Config {
        fn new(text: &str) -> Self {
            let directory = tempfile::tempdir().unwrap();
            std::fs::write(directory.path().join("config"), text).unwrap();
            Self { directory }
        }

        fn path(&self, name: &str) -> PathBuf {
            self.directory.path().join(name)
        }

        fn options(&self) -> Options {
            Options {
                ssh: Some(find_ssh().expect("OpenSSH is installed where tests run")),
                config: Some(self.path("config")),
                timeout: TIMEOUT,
            }
        }

        async fn resolve(&self, host: &str) -> Result<Resolved, Error> {
            resolve_with(host, &self.options()).await
        }
    }

    #[tokio::test]
    async fn match_blocks_apply_as_ssh_applies_them() {
        // As tools that write SSH config often do: one block by prefix, and
        // a `Match` block by suffix that only `ssh` itself can evaluate.
        let config = Config::new(
            "Host gw.*\n  ProxyCommand gateway ssh --stdio --prefix gw. %h\n\
             Match host *.gw !exec \"false\"\n  ProxyCommand gateway ssh --stdio --suffix gw %h\n",
        );
        let suffix = config.resolve("app.box.me.gw").await.unwrap();
        assert_eq!(
            suffix.proxy_command.as_deref(),
            Some("gateway ssh --stdio --suffix gw %h")
        );
        assert_eq!(suffix.hostname, None);
        let prefix = config.resolve("gw.box").await.unwrap();
        assert_eq!(
            prefix.proxy_command.as_deref(),
            Some("gateway ssh --stdio --prefix gw. %h")
        );
    }

    #[tokio::test]
    async fn an_alias_reports_its_host_user_port_key_and_jump() {
        let config = Config::new("");
        let key = config.path("id_test");
        std::fs::write(&key, "").unwrap();
        std::fs::write(
            config.path("included"),
            format!(
                "Host box\n  HostName 10.0.0.5\n  User ops\n  Port 2222\n  IdentityFile {}\n  ProxyJump bastion\n",
                key.display()
            ),
        )
        .unwrap();
        std::fs::write(
            config.path("config"),
            format!("Include {}\n", config.path("included").display()),
        )
        .unwrap();
        assert_eq!(
            config.resolve("box").await.unwrap(),
            Resolved {
                hostname: Some("10.0.0.5".into()),
                user: Some("ops".into()),
                port: Some(2222),
                identity_file: Some(key.display().to_string()),
                proxy_command: None,
                proxy_jump: Some("bastion".into()),
            }
        );
    }

    #[tokio::test]
    async fn a_host_the_config_doesnt_mention_has_nothing_to_fill() {
        let config = Config::new("Host other\n  User ops\n  IdentityFile /nonexistent/key\n");
        assert_eq!(
            config.resolve("plain.example").await.unwrap(),
            Resolved::default()
        );
        // A key file that doesn't exist is never filled in.
        assert_eq!(config.resolve("other").await.unwrap().identity_file, None);
    }

    #[tokio::test]
    async fn an_invalid_config_reports_ssh_s_message() {
        let config = Config::new("NotAnOption yes\n");
        let Err(Error::Failed(message)) = config.resolve("host").await else {
            panic!("an invalid config is an error");
        };
        assert!(
            message.contains("NotAnOption") || message.contains("Bad configuration"),
            "{message}"
        );
    }

    #[tokio::test]
    async fn a_slow_match_exec_times_out() {
        let config = Config::new("Match exec \"sleep 10\"\n  User slow\n");
        let options = Options {
            timeout: Duration::from_millis(300),
            ..config.options()
        };
        let started = std::time::Instant::now();
        assert!(matches!(
            resolve_with("host", &options).await,
            Err(Error::Failed(_))
        ));
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
