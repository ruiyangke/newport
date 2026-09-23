fn main() {
    let args: Vec<String> = std::env::args().collect();
    let name = std::path::Path::new(&args[0])
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let result = match name.as_ref() {
        "xclip" => porthop_agent::clipboard::read(&args[1..]),
        "wl-paste" => porthop_agent::clipboard::wl_paste(&args[1..]),
        "porthop-browser" => porthop_agent::browser::open(&args[1..], false),
        "xdg-open" => porthop_agent::browser::open(&args[1..], true),
        _ => match args.get(1).map(String::as_str) {
            Some("serve") => porthop_agent::agent::serve(
                args.get(2).map(String::as_str).unwrap_or("manual"),
                args.iter().any(|a| a == "--clipboard"),
                args.iter().any(|a| a == "--browser"),
            ),
            Some("clipboard") => porthop_agent::clipboard::read(&args[2..]),
            Some("open") => porthop_agent::browser::open(&args[2..], false),
            Some("env") => porthop_agent::environment::print(),
            Some("install") => porthop_agent::install::install(),
            Some("display") => match porthop_agent::display::run(std::env::args_os().skip(2)) {
                Ok(code) => std::process::exit(code),
                Err(error) => Err(error),
            },
            Some("--version") => {
                println!("{}", porthop_agent::wire::VERSION);
                Ok(())
            }
            Some("--help") | None => {
                println!("Porthop agent\n\nCommands: serve, install, clipboard, open URL, env, display --backend x11|wayland");
                Ok(())
            }
            _ => Err(std::io::Error::other("Unknown command; use --help")),
        },
    };
    if let Err(error) = result {
        eprintln!("porthop-agent: {error}");
        std::process::exit(1);
    }
}
