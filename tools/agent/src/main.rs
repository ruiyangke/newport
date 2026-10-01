fn main() {
    let args: Vec<String> = std::env::args().collect();
    let name = std::path::Path::new(&args[0])
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let result = match name.as_ref() {
        "xclip" => newport_agent::clipboard::read(&args[1..]),
        "wl-paste" => newport_agent::clipboard::wl_paste(&args[1..]),
        "newport-browser" | newport_agent::migration::LEGACY_BROWSER => {
            newport_agent::browser::open(&args[1..], false)
        }
        "xdg-open" => newport_agent::browser::open(&args[1..], true),
        _ => match args.get(1).map(String::as_str) {
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
            Some("clipboard") => newport_agent::clipboard::read(&args[2..]),
            Some("open") => newport_agent::browser::open(&args[2..], false),
            Some("env") => newport_agent::environment::print(&args[2..]),
            Some("install") => newport_agent::install::install(),
            Some("display") => match newport_agent::display::run(std::env::args_os().skip(2)) {
                Ok(code) => std::process::exit(code),
                Err(error) => Err(error),
            },
            Some("--version") => {
                println!("{}", newport_agent::wire::VERSION);
                Ok(())
            }
            Some("--help") | None => {
                println!("Newport agent\n\nCommands: serve, git-rpc --stdio, install, clipboard, open URL, env, display --backend x11|wayland");
                Ok(())
            }
            _ => Err(std::io::Error::other("Unknown command; use --help")),
        },
    };
    if let Err(error) = result {
        eprintln!("newport-agent: {error}");
        std::process::exit(1);
    }
}
