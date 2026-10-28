fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    match newport_agent::cli::run(&args, &[]) {
        Ok(0) => (),
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("newport-agent: {error}");
            std::process::exit(1);
        }
    }
}
