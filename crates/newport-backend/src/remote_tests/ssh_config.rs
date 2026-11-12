//! A proxy command chosen by a `Match` block, as only `ssh -G` resolves it,
//! carries Newport's own SSH connection to the server.
use super::*;

#[tokio::test]
#[ignore = "Requires cargo xtask test remote, real OpenSSH and nc"]
async fn a_proxy_command_from_a_match_block_carries_the_connection() {
    let direct = server();
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config");
    std::fs::write(
        &config,
        format!(
            "Host gw.*\n  ProxyCommand false\n\
             Match host *.gw !exec \"false\"\n  ProxyCommand nc {} %p\n",
            direct.ssh_host
        ),
    )
    .unwrap();
    let options = crate::ssh_resolve::Options {
        config: Some(config),
        ..Default::default()
    };
    let resolved = crate::ssh_resolve::resolve_with("app.box.me.gw", &options)
        .await
        .unwrap();
    let command = resolved.proxy_command.expect("the Match block's command");
    assert_eq!(command, format!("nc {} %p", direct.ssh_host));
    let mut through = direct.clone();
    through.ssh_host = "app.box.me.gw".into();
    through.proxy = Some(crate::model::Proxy::Command(command));
    let session = crate::remote::Shell::new(&through);
    assert_eq!(
        session.execute("printf through", None).await.unwrap(),
        "through"
    );
}
