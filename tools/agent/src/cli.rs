//! Shared command interface for the core and composed agent executables.
//! The embedding executable dispatches its own supervisor and desktop helpers
//! before entering this adapter. No library command terminates the process.
use std::{ffi::OsString, io};

pub fn run(args: &[OsString], services: &[&dyn crate::composition::Service]) -> io::Result<i32> {
    run_product(args, crate::product::Variant::Core, services)
}

pub fn run_product(
    args: &[OsString],
    variant: crate::product::Variant,
    services: &[&dyn crate::composition::Service],
) -> io::Result<i32> {
    crate::composition::capabilities(services)?;
    let executable = args
        .first()
        .ok_or_else(|| io::Error::other("Missing executable name"))?;
    let alias = std::path::Path::new(executable)
        .file_name()
        .unwrap_or_default();
    let helper = ["xclip", "xsel", "wl-paste", "newport-browser", "xdg-open"]
        .iter()
        .any(|name| alias == *name);
    if !helper {
        if args.get(1).is_some_and(|arg| arg == "gui") {
            return crate::gui_launch::run(args[2..].to_vec()).map(|()| 0);
        }
        if args.get(1).is_some_and(|arg| arg == "display") {
            return crate::display::run(args[2..].iter().cloned());
        }
    }
    let args = args
        .iter()
        .map(|arg| {
            arg.to_str().map(str::to_owned).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Command requires UTF-8 arguments",
                )
            })
        })
        .collect::<io::Result<Vec<_>>>()?;
    run_core(&args, variant, services).map(|()| 0)
}

fn run_core(
    args: &[String],
    variant: crate::product::Variant,
    services: &[&dyn crate::composition::Service],
) -> std::io::Result<()> {
    let name = std::path::Path::new(&args[0])
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    match name.as_ref() {
        "xclip" => crate::clipboard::read(&args[1..]),
        "xsel" => crate::clipboard::xsel(&args[1..]),
        "wl-paste" => crate::clipboard::wl_paste(&args[1..]),
        "newport-browser" => crate::browser::open(&args[1..], false),
        "xdg-open" => crate::browser::open(&args[1..], true),
        _ => match args.get(1).map(String::as_str) {
            Some("runtime") if args.len() == 3 && args[2] == "--stdio" => {
                crate::runtime::serve_product(variant, services)
            }
            Some("product") if args.len() == 3 && args[2] == "--json" => {
                let identity = crate::product::Identity {
                    schema_version: crate::product::VERSION,
                    build_sha256: crate::runtime::build_identity()?,
                    product: crate::runtime::compiled_product(variant, services)?,
                };
                println!("{}", serde_json::to_string(&identity)?);
                Ok(())
            }
            Some("worker") if args.len() >= 4 => {
                crate::runtime::worker_with(&args[2], &args[3..], services)
            }
            Some("serve") if args.iter().any(|a| a == "--managed") => {
                crate::agent::serve_managed(args.get(2).map(String::as_str).unwrap_or("manual"))
            }
            Some("serve") => crate::agent::serve(
                args.get(2).map(String::as_str).unwrap_or("manual"),
                args.iter().any(|a| a == "--clipboard"),
                args.iter().any(|a| a == "--browser"),
            ),
            Some("git-rpc")
                if args.get(2).map(String::as_str) == Some("--stdio") && args.len() == 3 =>
            {
                crate::git::serve()
            }
            Some("monitor")
                if args.get(2).map(String::as_str) == Some("--stdio") && args.len() == 3 =>
            {
                crate::monitor::serve()
            }
            Some("clipboard") => crate::clipboard::read(&args[2..]),
            Some("open") => crate::browser::open(&args[2..], false),
            Some("sessions") => crate::routing::list(),
            Some("listeners") if args.len() == 2 => crate::ports::print_listeners(),
            Some("print-env") if args.len() == 2 => crate::lsp::print_env(),
            Some("env") => crate::environment::print(&args[2..]),
            Some("install") => crate::install::install(),
            Some("deployment") => crate::deployment::run_product(
                &args[2..],
                &crate::runtime::compiled_product(variant, services)?,
            ),
            Some("--version") => {
                println!("{}", crate::wire::VERSION);
                Ok(())
            }
            Some("--help") | None => {
                println!(
                    "Newport agent\n\nCommands: runtime --stdio, serve, git-rpc --stdio, monitor --stdio, install, listeners, clipboard, open URL, env, sessions, display --backend x11|wayland"
                );
                Ok(())
            }
            _ => Err(std::io::Error::other("Unknown command; use --help")),
        },
    }
}
