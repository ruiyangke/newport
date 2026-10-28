//! An independent consumer of the public composition contract.
use newport_agent::{
    composition::{Service, WorkerContext},
    runtime_protocol::{self as wire, Capability},
};
use std::{io, process::Command, time::Duration};

struct Fixture(bool);
impl Service for Fixture {
    fn name(&self) -> &'static str {
        if self.0 {
            "fixture"
        } else {
            "missing-fixture"
        }
    }
    fn capability(&self) -> Capability {
        Capability {
            versions: vec![7],
            available: self.0,
            reason: (!self.0).then(|| "Fixture dependency absent".into()),
        }
    }
    fn run(&self, context: WorkerContext, args: &[String]) -> io::Result<()> {
        let mut output = io::stdout().lock();
        wire::write(
            &mut output,
            &[
                context.session_id(),
                context.runtime_id(),
                context.worker_id(),
                context.build_sha256(),
            ],
        )?;
        match args.first().map(String::as_str) {
            Some("native-clipboard") => {
                let provider = context.native_clipboard()?;
                wire::write(&mut output, &provider.poll()?.lease.is_some())?;
                let mut published = false;
                while !context.cancellation().is_cancelled() {
                    // Runtime teardown can close provider IPC before this
                    // process's lease watcher gets its next scheduling turn.
                    let poll = match provider.poll() {
                        Ok(poll) => poll,
                        Err(_) => break,
                    };
                    if let Some(lease) = poll.lease.filter(|_| !published) {
                        if !provider.publish(&lease, &poll.offer, 1, vec!["text/plain".into()])? {
                            return Err(io::Error::other(
                                "Native fixture lost initial publication",
                            ));
                        }
                        wire::write(&mut output, &provider.poll()?.offer)?;
                        published = true;
                    }
                    for request in poll.requests {
                        if request.format == "text/plain" {
                            provider.reply(&request, b"native through runtime", true)?;
                        } else {
                            provider.fail(&request)?;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                wire::write(&mut output, &"cancelled")
            }
            Some("clipboard") | Some("clipboard-offer") | Some("clipboard-watch-empty") => {
                let reader = context.clipboard()?;
                let offer = reader.offer()?;
                wire::write(&mut output, &offer)?;
                if args[0] == "clipboard-offer" {
                    return Ok(());
                }
                if args[0] == "clipboard-watch-empty" {
                    let deadline = std::time::Instant::now() + Duration::from_secs(5);
                    while std::time::Instant::now() < deadline {
                        context.cancellation().check()?;
                        let offer = reader.offer()?;
                        if offer.formats.is_empty() {
                            return wire::write(&mut output, &offer);
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    return Err(io::Error::other("Clipboard producer did not retire"));
                }
                let mut read = reader.read(
                    &offer,
                    args.get(1)
                        .ok_or_else(|| io::Error::other("Format required"))?,
                )?;
                let mut bytes = vec![];
                loop {
                    let chunk = read.next(4096)?;
                    bytes.extend(chunk.bytes);
                    if chunk.eof {
                        break;
                    }
                }
                wire::write(&mut output, &bytes)
            }
            Some("hold") => {
                while !context.cancellation().is_cancelled() {
                    std::thread::sleep(Duration::from_millis(5));
                }
                wire::write(&mut output, &"cancelled")
            }
            Some("crash") => std::process::exit(23),
            Some("helper") => {
                let helper = Command::new(std::env::current_exe()?)
                    .arg("--fixture-helper")
                    .output()?;
                if !helper.status.success() {
                    return Err(io::Error::other("Helper failed"));
                }
                wire::write(&mut output, &String::from_utf8(helper.stdout).unwrap())
            }
            _ => wire::write(&mut output, &args),
        }
    }
}

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    // The host dispatches owned re-execution modes before the shared core CLI.
    if args.get(1).is_some_and(|a| a == "--fixture-helper") {
        println!("helper");
        return;
    }
    match newport_agent::cli::run(&args, &[&Fixture(true), &Fixture(false)]) {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
