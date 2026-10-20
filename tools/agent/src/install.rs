//! Install managed command aliases while preserving unrelated files.
use crate::helper_transaction::{Origin, Snapshot, Transaction};
use std::{env, io, path::PathBuf};
use uuid::Uuid;
pub fn install() -> io::Result<()> {
    install_with_origin(Origin::Standalone, Uuid::new_v4()).map(|_| ())
}
pub(crate) fn install_quiet(operation_id: Uuid) -> io::Result<Option<String>> {
    install_with_origin(Origin::Deployment, operation_id)
}
fn install_with_origin(origin: Origin, operation_id: Uuid) -> io::Result<Option<String>> {
    let home =
        PathBuf::from(env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is required"))?);
    // Keep explicit standalone helper setup from racing destructive maintenance
    // after its initial admission check and before it takes the helper lock.
    let _usage = crate::installation_use::ServiceLease::acquire()?;
    let dir = home.join(".local/bin");
    let mut transaction = Transaction::open(&home, operation_id)?;
    let planned = (|| {
        for name in ["xclip", "wl-paste", "xdg-open", "newport-browser"] {
            let path = dir.join(name);
            let before = transaction.read(&path)?;
            let managed =
                matches!(&before, Snapshot::Link(target) if target.as_os_str() == "newport-agent");
            if name == "newport-browser" && before != Snapshot::Missing && !managed {
                return Err(io::Error::other(
                    "newport-browser is occupied by an unrelated command",
                ));
            }
            if before == Snapshot::Missing || managed {
                transaction.plan(&path, before, Snapshot::Link("newport-agent".into()))?;
            }
        }
        crate::shell_setup::plan(&mut transaction)
    })();
    let notices = match planned {
        Ok(notices) => notices,
        Err(error) => {
            transaction.cancel_plan()?;
            return Err(error);
        }
    };
    transaction.apply_install(origin)?;
    let notice =
        (!notices.is_empty()).then(|| format!("Manual shell setup needed. {}", notices.join(" ")));
    let ours = env::current_exe()?.canonicalize()?;
    let resolved = env::split_paths(&env::var_os("PATH").unwrap_or_default())
        .map(|p| p.join("xclip"))
        .find(|p| p.is_file());
    let ready = resolved.and_then(|p| p.canonicalize().ok()).as_ref() == Some(&ours);
    if origin == Origin::Standalone {
        if let Some(notice) = &notice {
            eprintln!("{notice}");
        }
        println!(
            "NEWPORT_SHIM_PATH={}",
            if ready { "ready" } else { "missing" }
        );
    }
    Ok(notice)
}
