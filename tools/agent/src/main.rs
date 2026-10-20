fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Err(error) = run(&args) {
        eprintln!("newport-agent: {error}");
        std::process::exit(1);
    }
}

fn run(args: &[String]) -> std::io::Result<()> {
    let name = std::path::Path::new(&args[0])
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    // Hold admission for the complete service process, including registered
    // workers and direct/legacy CLI entry points. Lifecycle commands obtain
    // their own maintenance authority rather than a conflicting shared lease.
    let service = matches!(
        name.as_ref(),
        "xclip" | "wl-paste" | "newport-browser" | "xdg-open"
    ) || matches!(
        args.get(1).map(String::as_str),
        Some(
            "runtime"
                | "worker"
                | "serve"
                | "git-rpc"
                | "monitor"
                | "clipboard"
                | "open"
                | "display"
                | "env"
                | "sessions"
        )
    );
    let _lease = if service {
        Some(newport_agent::installation_use::ServiceLease::acquire()?)
    } else {
        None
    };
    match name.as_ref() {
        "xclip" => newport_agent::clipboard::read(&args[1..]),
        "wl-paste" => newport_agent::clipboard::wl_paste(&args[1..]),
        "newport-browser" => newport_agent::browser::open(&args[1..], false),
        "xdg-open" => newport_agent::browser::open(&args[1..], true),
        _ => match args.get(1).map(String::as_str) {
            Some("runtime") if args.len() == 3 && args[2] == "--stdio" => {
                newport_agent::runtime::serve()
            }
            Some("worker") if args.len() >= 4 => {
                newport_agent::runtime::worker(&args[2], &args[3..])
            }
            Some("serve") if args.iter().any(|a| a == "--managed") => {
                newport_agent::agent::serve_managed(
                    args.get(2).map(String::as_str).unwrap_or("manual"),
                )
            }
            Some("serve") => newport_agent::agent::serve(
                args.get(2).map(String::as_str).unwrap_or("manual"),
                args.iter().any(|a| a == "--clipboard"),
                args.iter().any(|a| a == "--browser"),
            ),
            Some("git-rpc")
                if args.get(2).map(String::as_str) == Some("--stdio") && args.len() == 3 =>
            {
                newport_agent::git::serve()
            }
            Some("monitor")
                if args.get(2).map(String::as_str) == Some("--stdio") && args.len() == 3 =>
            {
                newport_agent::monitor::serve()
            }
            Some("clipboard") => newport_agent::clipboard::read(&args[2..]),
            Some("open") => newport_agent::browser::open(&args[2..], false),
            Some("sessions") => newport_agent::routing::list(),
            Some("listeners") if args.len() == 2 => newport_agent::ports::print_listeners(),
            Some("env") => newport_agent::environment::print(&args[2..]),
            Some("install") => newport_agent::install::install(),
            Some("deployment") => newport_agent::deployment::run(&args[2..]),
            Some("display") => match newport_agent::display::run(std::env::args_os().skip(2)) {
                Ok(code) => std::process::exit(code),
                Err(error) => Err(error),
            },
            Some("--version") => {
                println!("{}", newport_agent::wire::VERSION);
                Ok(())
            }
            Some("--help") | None => {
                println!("Newport agent\n\nCommands: runtime --stdio, serve, git-rpc --stdio, monitor --stdio, install, listeners, clipboard, open URL, env, sessions, display --backend x11|wayland");
                Ok(())
            }
            _ => Err(std::io::Error::other("Unknown command; use --help")),
        },
    }
}
