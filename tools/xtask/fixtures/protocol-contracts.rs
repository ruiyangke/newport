#[cfg(test)]
mod tests {
    #[test]
    fn desktop_discovery_validates_without_a_codec_or_domain_dependency() {
        use newport_protocol::{desktop::connection::discovery::Discovery, monitor::ports::discovery::{Coverage, Report, Snapshot}};
        let value = Discovery {
            current: Snapshot {
                generation: 1,
                coverage: Coverage { ipv4: Report::complete(), ..Default::default() },
                ..Default::default()
            },
            retained: Vec::new(),
            retention: Report::complete(),
        };
        value.validate().unwrap();
        assert!(value.has_current_inventory());
    }

    #[test]
    fn discovery_helper_contracts_validate_without_a_codec() {
        use newport_protocol::monitor::ports::discovery::{Snapshot, helper};
        let request = helper::Request { projects: None, contract_id: Some("a".repeat(64)), generation: 3 };
        let mut reply = helper::Reply {
            contract_id: request.contract_id.clone(), generation: request.generation,
            result: Ok(Box::new(Snapshot { generation: request.generation, ..Default::default() })),
        };
        reply.validate_for(&request).unwrap();
        reply.result.as_mut().unwrap().generation += 1;
        assert!(reply.validate_for(&request).is_err());
    }

    #[test]
    fn desktop_exchange_and_reply_contracts_work_without_an_encoder() {
        use newport_protocol::{common::Present, desktop::{self, host}};
        let id = desktop::RequestId::new(1).unwrap();
        let request = desktop::Request {
            id,
            operation: desktop::Operation::Host(Box::new(host::Operation::Activate(Box::new(Present {})))),
        };
        let response = desktop::Response {
            id,
            outcome: desktop::Outcome::Completed(Box::new(desktop::Reply::Host(Box::new(host::Reply::Activate(Box::new(Present {})))))),
        };
        response.validate_for(&request).unwrap();
        let mut exchange = desktop::Exchange::default();
        exchange.client(&desktop::ClientMessage::Hello(Box::new(desktop::Hello { version: desktop::VERSION, contract_id: Some("a".repeat(64)) }))).unwrap();
        exchange.server(&desktop::ServerMessage::Ready(Box::new(desktop::Ready {
            version: desktop::VERSION,
            backend_instance: "11111111-1111-4111-8111-111111111111".parse().unwrap(),
            contract_id: Some("a".repeat(64)),
        }))).unwrap();
        exchange.client(&desktop::ClientMessage::Request(Box::new(request))).unwrap();
        exchange.server(&desktop::ServerMessage::Response(Box::new(response))).unwrap();
        assert_eq!(exchange.phase(), desktop::Phase::Closing);
        exchange.eof().unwrap();
        assert_eq!(exchange.phase(), desktop::Phase::Ended);
        assert!(desktop::IdSequence::after(u64::MAX).allocate().is_err());
    }

    use newport_protocol::{
        display::{attachment::Attach, environment},
        remote_apps::{
            features::Features,
            session_launch::Target,
            tile::{Codec, Tile},
        },
        worker::{Chunk, ClientControl, Direction, Exit, ServerControl, Signal, Transport},
    };

    #[test]
    fn contracts_validate_and_share_payloads_without_an_encoder() {
        let wanted = Features::all();
        assert!(wanted.granted(&Features::default()).within(&wanted));
        Target::Command {
            argv: vec!["app".into(), "literal argument".into()],
        }
        .validate()
        .unwrap();
        assert!(Target::Command { argv: vec![] }.validate().is_err());
        assert!(environment::shared_terminal_v1::allowed("WAYLAND_DISPLAY"));
        let tile = Tile::new(0, Codec::RawBgra, vec![1, 2, 3, 4]);
        let cloned = tile.clone();
        assert_eq!(tile.data.as_ptr(), cloned.data.as_ptr());
        let attach = Attach {
            session: "s".into(),
            computer: "desktop".into(),
            clipboard: Some(newport_protocol::clipboard::Access {
                path: "/run/user/1000/newport/clipboard.sock".into(),
                runtime: "runtime".into(),
                worker: "worker".into(),
                token: "11111111-1111-4111-8111-111111111111".into(),
            }),
        };
        assert!(!format!("{attach:?}").contains("11111111-1111-4111-8111-111111111111"));
    }

    #[test]
    fn launch_admission_retains_its_byte_budget_without_protobuf() {
        let uri = format!("file:///{}", "a".repeat(8184));
        let target = Target::Desktop {
            id: "app.desktop".into(),
            uris: vec![uri; 4],
            action: None,
        };
        assert!(target.validate().unwrap_err().contains("wire limit"));
    }

    #[test]
    fn worker_controls_validate_without_an_encoder() {
        const TRANSPORT: Transport = Transport::MONITOR;
        assert_eq!(TRANSPORT.limits(Direction::ToWorker), newport_protocol::monitor::CLIENT_LIMITS);
        assert_eq!(TRANSPORT.limits(Direction::FromWorker), newport_protocol::monitor::SERVER_LIMITS);
        let signal = Signal::new(15).unwrap();
        assert_eq!(ClientControl::Signal(signal), ClientControl::Signal(signal));
        assert!(Signal::new(0).is_err());
        let chunk = Chunk::new(b"private diagnostic".to_vec().into()).unwrap();
        let diagnostic = ServerControl::Stderr(chunk.clone());
        assert!(!format!("{diagnostic:?}").contains("private"));
        assert_eq!(chunk.data().as_ptr(), chunk.clone().data().as_ptr());
        assert!(matches!(ServerControl::Exit(Exit::Status(0)), ServerControl::Exit(_)));
    }

    #[test]
    fn runtime_permissions_cannot_add_access_or_change_revision() {
        use newport_protocol::runtime::{Applied, Permissions};
        let requested = Permissions { revision: 7, clipboard: true, browser: false };
        Applied { permissions: requested, error: None }.validate_for(&requested).unwrap();
        let added = Permissions { browser: true, ..requested };
        assert!(Applied { permissions: added, error: None }.validate_for(&requested).is_err());
        let stale = Permissions { revision: 6, ..requested };
        assert!(Applied { permissions: stale, error: None }.validate_for(&requested).is_err());
    }

    #[test]
    fn desktop_permission_snapshots_allow_only_disabled_unacknowledged_access() {
        use newport_protocol::{desktop::configuration::PermissionsSnapshot, runtime::Permissions};
        let mut snapshot = PermissionsSnapshot {
            desired: Permissions { revision: 1, clipboard: true, browser: true },
            effective: Permissions { revision: 0, clipboard: false, browser: false },
            error: None, clipboard_epoch: 0, browser_epoch: 0, session_epoch: 0,
        };
        snapshot.validate().unwrap();
        assert!(snapshot.effective.validate().is_err());
        snapshot.effective.clipboard = true;
        assert!(snapshot.validate().is_err());
        snapshot.effective.clipboard = false;
        snapshot.effective.browser = true;
        assert!(snapshot.validate().is_err());
        snapshot.effective.browser = false;
        snapshot.desired.revision = 0;
        assert!(snapshot.validate().is_err());
    }

    #[test]
    fn terminal_results_match_the_requested_operation_without_an_encoder() {
        use newport_protocol::terminal::{Reply, Request};
        let request = Request::End { id: "11111111111141118111111111111111".into() };
        request.validate().unwrap();
        Reply::Ended { ended: false }.validate_for(&request).unwrap();
        assert!(Reply::Renamed { renamed: true }.validate_for(&request).is_err());
    }

    #[test]
    fn catalogue_validates_and_reserves_bytes_without_an_encoder() {
        use newport_protocol::remote_apps::apps::{Application, Catalogue, CATALOGUE_BUDGET, MAX_CATALOGUE_BYTES};
        let app = Application { id: "fixture.desktop".into(), name: "Fixture".into(), description: String::new(),
            keywords: vec![], icon: None, actions: vec![] };
        assert!(CATALOGUE_BUDGET + app.byte_budget().unwrap() < MAX_CATALOGUE_BYTES);
        Catalogue { apps: vec![app.clone()] }.validate().unwrap();
        assert!(Catalogue { apps: vec![app.clone(), app] }.validate().is_err());
    }

    #[test]
    fn clipboard_exports_validate_without_an_encoder() {
        use newport_protocol::clipboard::{Chunk, exports::{Control, Event, CHUNK, MAX_BYTES}};
        Control::Cancel { id: 1 }.validate().unwrap();
        assert!(Control::Cancel { id: 0 }.validate().is_err());
        Event::Data { id: 1, offset: MAX_BYTES - CHUNK,
            chunk: Some(Chunk { bytes: vec![0; CHUNK], eof: true }) }.validate().unwrap();
        assert!(Event::Data { id: 1, offset: MAX_BYTES,
            chunk: Some(Chunk { bytes: vec![0], eof: true }) }.validate().is_err());
    }

    #[test]
    fn integration_and_host_imports_validate_without_an_encoder() {
        use newport_protocol::{clipboard::imports, integration::{Call, Request, Response, Reply}};
        let offer = imports::Offer { revision: i64::MIN, formats: vec!["image/png".into()] };
        offer.validate().unwrap();
        let call = Call { id: 1, request: Request::ClipboardOffer(offer) };
        Response { id: 1, reply: Reply::Acknowledged { message: String::new() } }.validate_for(&call).unwrap();
        assert!(Response { id: 2, reply: Reply::Acknowledged { message: String::new() } }.validate_for(&call).is_err());
        assert!(imports::Reply { id: 1, revision: i64::MIN, encoding: imports::Encoding::Failed,
            done: false, bytes: vec![] }.validate().is_err());
    }

    #[test]
    fn git_calls_validate_without_an_encoder() {
        use newport_protocol::git::{Call, Closed, Path, Reply, Request, Response};
        let id = "11111111-1111-4111-8111-111111111111";
        Request::Open { path: Path::new(b"/tmp/repository") }.validate().unwrap();
        let call = Call { id: id.into(), request: Box::new(Request::Close { repo_id: "repository".into() }) };
        call.validate().unwrap();
        Response { id: id.into(), result: Ok(Reply::Close(Box::new(Closed { closed: true }))) }.validate_for(&call).unwrap();
        assert!(Request::Open { path: Path::new(b"invalid\0path") }.validate().is_err());
    }

    #[test]
    fn metrics_validate_without_an_encoder() {
        use newport_protocol::monitor::metrics::{Core, Process, Snapshot};
        let mut snapshot = Snapshot {
            cores: 1,
            cpus: vec![Core { id: 0, usage: 100.0, history: vec![0.0] }],
            processes: vec![Process { pid: 1, cpu: 200.0, elapsed: Some(0), ..Default::default() }],
            ..Default::default()
        };
        snapshot.validate().unwrap();
        snapshot.cpus[0].usage = f64::NAN;
        assert!(snapshot.validate().is_err());
    }

    #[test]
    fn monitor_responses_match_the_call_without_an_encoder() {
        use newport_protocol::monitor::{Call, Reply, Request, Response, stable_display::State};
        let call = Call { id: 1, request: Request::StableDisplay };
        call.validate().unwrap();
        Response { id: 1, reply: Reply::StableDisplay(State::default()) }.validate_for(&call).unwrap();
        assert!(Response { id: 2, reply: Reply::StableDisplay(State::default()) }.validate_for(&call).is_err());
        assert!(Call { id: 0, request: Request::Metrics }.validate().is_err());
    }

    #[test]
    fn compose_jobs_validate_without_an_encoder() {
        use newport_protocol::monitor::compose::{Action, Request};
        Request::Run { project: "app".into(), action: Action::Scale { service: "web".into(), replicas: 2 } }.validate().unwrap();
        assert!(Request::Cancel { job: "../job".into() }.validate().is_err());
    }

    #[test]
    fn container_mutations_validate_without_an_encoder() {
        use newport_protocol::monitor::containers::{ContainerVerb, Done, Reply, Request};
        let request = Request::Action { container: "web".into(), verb: ContainerVerb::Restart };
        request.validate().unwrap();
        Reply::Action(Done { ok: true }).validate_for(&request).unwrap();
        assert!(Reply::ProjectAction(Done { ok: true }).validate_for(&request).is_err());
        assert!(Request::Remove { containers: vec!["../web".into()] }.validate().is_err());
    }

    #[test]
    fn service_mutations_validate_without_an_encoder() {
        use newport_protocol::monitor::services::{ActionReply, Reply, Request, Scope, Verb};
        let request = Request::Action { scope: Scope::User, unit: "worker.service".into(), verb: Verb::Restart };
        request.validate().unwrap();
        let reply = Reply::Action(ActionReply { ok: true, needs_password: None, command: None });
        reply.validate_for(&request).unwrap();
        assert!(reply.validate_for(&Request::Snapshot).is_err());
    }

    #[test]
    fn file_watch_replies_stay_inside_the_project_without_an_encoder() {
        use newport_protocol::monitor::file_watch::{Reply, Request};
        let request = Request { watch: "11111111-1111-4111-8111-111111111111".into(), root: "/repo".into(),
            directories: vec!["/repo".into()], cursor: None };
        request.validate().unwrap();
        let mut reply = Reply { cursor: Some("current".into()), paths: vec!["/repo/file".into()], reset: false };
        reply.validate_for(&request).unwrap();
        reply.paths.push("/repo-other/file".into());
        assert!(reply.validate_for(&request).is_err());
    }

    #[test]
    fn local_helpers_validate_without_an_encoder() {
        use newport_protocol::{browser, clipboard::{imports, service}};
        browser::Request { url: "https://example.com/path".into() }.validate().unwrap();
        assert!(browser::Request { url: "file:///private/path".into() }.validate().is_err());
        assert!(browser::ServerMessage::Hello { version: 0 }.validate().is_err());
        service::Hello { version: service::VERSION, runtime: "runtime".into() }.validate().unwrap();
        let mut read = service::Request::Read {
            runtime: "runtime".into(), revision: i64::MIN, format: "text/plain".into(),
            offset: 0, limit: imports::CHUNK, export: None,
        };
        read.validate().unwrap();
        if let service::Request::Read { limit, .. } = &mut read { *limit = 0; }
        assert!(read.validate().is_err());
        assert!(service::Response::Requests(vec![imports::Request {
            id: 0, revision: 1, format: "text/plain".into(),
        }]).validate().is_err());
    }

    #[test]
    fn display_preparation_contains_no_credentials_without_an_encoder() {
        use newport_protocol::display::attachment;
        let prepare = attachment::prepare("build");
        assert!(prepare.body.is_empty());
        attachment::admit_prepare(prepare.clone(), "display").unwrap();
        let with_body = attachment::Envelope { body: vec![1].into(), ..prepare };
        assert!(attachment::admit_prepare(with_body, "display").is_err());
    }

    #[test]
    fn holder_identity_and_source_revision_validate_without_encoding() {
        use newport_protocol::{clipboard::source, terminal::holder};
        let id = "11111111111141118111111111111111";
        holder::Hello { version: holder::VERSION, terminal_id: id.into() }.require(id).unwrap();
        holder::ClientMessage::End.validate().unwrap();
        source::Request::Read { revision: i64::MIN, format: "text/plain".into() }.validate().unwrap();
        source::Response::Hello { version: source::VERSION }.validate().unwrap();
    }
}
