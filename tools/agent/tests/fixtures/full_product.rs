use newport_agent::{
    composition::{Service, WorkerContext},
    runtime_protocol::Capability,
};
struct Gui;
impl Service for Gui {
    fn name(&self) -> &'static str {
        "remote-apps"
    }
    fn capability(&self) -> Capability {
        Capability {
            versions: vec![1],
            available: false,
            reason: Some("Fixture has no renderer".into()),
        }
    }
    fn run(&self, _: WorkerContext, _: &[String]) -> std::io::Result<()> {
        unreachable!()
    }
}
fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    match newport_agent::cli::run_product(&args, newport_agent::product::Variant::Full, &[&Gui]) {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("newport-agent: {error}");
            std::process::exit(1);
        }
    }
}
