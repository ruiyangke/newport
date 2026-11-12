#[cfg(target_os = "linux")]
mod fixture;
#[cfg(target_os = "linux")]
mod helpers;
#[cfg(target_os = "linux")]
mod measurement;

#[cfg(target_os = "linux")]
fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    match args.get(1).and_then(|s| s.to_str()) {
        Some("listeners" | "projects") => {
            std::process::exit(helpers::run(&args));
        }
        Some("--fixture-worker") => fixture::worker(&args[2..]),
        Some("--qualify-discovery-load") => {
            assert_ne!(
                helpers::effective_uid(),
                0,
                "Use an isolated ordinary account"
            );
            measurement::run(args.iter().any(|s| s == "--elevation"));
        }
        _ => println!("Use --qualify-discovery-load in an isolated Linux fixture."),
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    println!("Discovery load qualification requires Linux.");
}
