fn main() {
    let args: Vec<String> = std::env::args().collect();
    let name = std::path::Path::new(&args[0])
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let result = match name.as_ref() {
        "xclip" => porthop_agent::commands::clipboard(&args[1..]),
        "wl-paste" => porthop_agent::commands::wl_paste(&args[1..]),
        "porthop-browser" => porthop_agent::commands::open(&args[1..], false),
        "xdg-open" => porthop_agent::commands::open(&args[1..], true),
        _ => match args.get(1).map(String::as_str) {
            Some("serve") => porthop_agent::agent::serve(
                args.get(2).map(String::as_str).unwrap_or("manual"),
                args.iter().any(|a| a == "--clipboard"),
                args.iter().any(|a| a == "--browser"),
            ),
            Some("clipboard") => porthop_agent::commands::clipboard(&args[2..]),
            Some("open") => porthop_agent::commands::open(&args[2..], false),
            Some("env") => porthop_agent::commands::environment(),
            Some("install") => porthop_agent::commands::install(),
            Some("display") => {
                porthop_agent::cli::main();
                return;
            }
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
