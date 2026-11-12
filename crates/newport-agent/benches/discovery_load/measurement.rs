use super::{
    fixture::{Fixture, Load, metadata::Metadata},
    helpers::{self, Operation, Reports},
};
use newport_agent::{
    containers::Containers,
    ports::discovery::{Discovery, Mode},
};
use newport_protocol::monitor::{
    self,
    ports::{
        MAX_FIELD, MAX_REPLY_TEXT,
        discovery::{self, IssueKind, Outcome, Request},
    },
    wire,
};
use serde::Serialize;
use std::time::{Duration, Instant};

const SAMPLES: usize = 21;

#[derive(Serialize)]
struct Sample {
    elapsed_us: u128,
    processes_visited: usize,
    descriptors_visited: usize,
    cgroups_visited: usize,
    sockets: usize,
    holders: usize,
    root_holders: usize,
    framed_bytes: usize,
    text_bytes: usize,
    protobuf_fields: usize,
    ipv4: Outcome,
    process_coverage: Outcome,
    details: Outcome,
    elevation: Outcome,
    cgroups: Outcome,
    helpers: Vec<helpers::Measurement>,
}

pub fn run(elevation: bool) {
    let reports = Reports::bind();
    for (name, load) in [
        (
            "typical",
            Load {
                processes: 8,
                root_processes: 0,
                sockets_per_process: 4,
                extra_descriptors: 32,
                long_arguments: 0,
                ..Default::default()
            },
        ),
        (
            "large",
            Load {
                processes: 128,
                root_processes: 0,
                sockets_per_process: 16,
                extra_descriptors: 256,
                long_arguments: 4,
                ..Default::default()
            },
        ),
        (
            "record-and-text-budget",
            Load {
                processes: 160,
                root_processes: 0,
                sockets_per_process: 32,
                extra_descriptors: 32,
                long_arguments: 8,
                ..Default::default()
            },
        ),
    ] {
        let fixture = Fixture::new(load);
        measure(
            name,
            &fixture,
            false,
            false,
            Duration::from_secs(20),
            &reports,
        );
        if name == "large" {
            measure(
                "expired-deadline",
                &fixture,
                false,
                false,
                Duration::ZERO,
                &reports,
            );
        }
    }
    for (name, load) in [
        (
            "aggregate-text-budget",
            Load {
                processes: 256,
                sockets_per_process: 1,
                directory_bytes: MAX_FIELD - 6,
                command_bytes: MAX_FIELD,
                ..Default::default()
            },
        ),
        (
            "protobuf-field-budget",
            Load {
                processes: 128,
                sockets_per_process: 4,
                short_arguments: 1024,
                ..Default::default()
            },
        ),
        (
            "project-enrichment-within-budget",
            Load {
                processes: 32,
                sockets_per_process: 1,
                directory_bytes: 3500,
                command_bytes: MAX_FIELD,
                project: true,
                ..Default::default()
            },
        ),
        (
            "project-enrichment-budget",
            Load {
                processes: 256,
                sockets_per_process: 1,
                directory_bytes: 3500,
                command_bytes: MAX_FIELD,
                project: true,
                ..Default::default()
            },
        ),
    ] {
        let fixture = Fixture::new(load);
        measure(
            name,
            &fixture,
            false,
            false,
            Duration::from_secs(20),
            &reports,
        );
    }
    if elevation {
        let fixture = Fixture::new(Load {
            processes: 16,
            root_processes: 8,
            sockets_per_process: 2,
            extra_descriptors: 64,
            long_arguments: 0,
            ..Default::default()
        });
        measure(
            "root-without-enrichment",
            &fixture,
            true,
            false,
            Duration::from_secs(20),
            &reports,
        );
        measure(
            "root-explicit-refresh",
            &fixture,
            false,
            true,
            Duration::from_secs(20),
            &reports,
        );
    }
}

fn measure(
    name: &str,
    fixture: &Fixture,
    identity: bool,
    refresh: bool,
    budget: Duration,
    reports: &Reports,
) {
    let metadata = (fixture.load.directory_bytes > 0 || fixture.load.short_arguments > 0)
        .then(|| Metadata::read(fixture));
    let mut samples = Vec::with_capacity(SAMPLES);
    let mut first_coverage = None;
    let allocation = allocation_counter::measure(|| {
        let containers = Containers::new(Box::new(Vec::new));
        let mut discovery = Discovery::default();
        for index in 0..SAMPLES {
            if refresh {
                if index > 0 {
                    // Manual refresh also respects the helper's minimum retry interval.
                    std::thread::sleep(Duration::from_secs(10));
                }
                discovery.invalidate();
            }
            let started = Instant::now();
            let scan = discovery
                .collect(
                    u64::try_from(index + 1).unwrap(),
                    started + budget,
                    if identity {
                        Mode::Identity
                    } else {
                        Mode::Full(&containers)
                    },
                    &|| false,
                )
                .unwrap();
            let elapsed_us = started.elapsed().as_micros();
            let mut framed_bytes = 0;
            let mut text_bytes = 0;
            let mut protobuf_fields = 0;
            let mut helpers = Vec::new();
            allocation_counter::opt_out(|| {
                helpers = reports.take();
                for helper in &helpers {
                    assert_eq!(
                        helper.effective_uid,
                        match helper.operation {
                            Operation::Listeners => 0,
                            Operation::Projects => helpers::effective_uid(),
                        }
                    );
                }
                if refresh {
                    assert_eq!(
                        helpers
                            .iter()
                            .filter(|h| h.operation == Operation::Listeners)
                            .count(),
                        1
                    );
                }
                if name.starts_with("project-enrichment-") {
                    assert_eq!(
                        helpers
                            .iter()
                            .filter(|h| h.operation == Operation::Projects)
                            .count(),
                        1
                    );
                }
                if index == 0 {
                    first_coverage = Some(scan.snapshot.coverage.clone());
                }
                let call = monitor::Call {
                    id: 1,
                    request: monitor::Request::Ports(Request { refresh }),
                };
                let response = monitor::Response {
                    id: 1,
                    reply: monitor::Reply::Ports(Box::new(scan.snapshot.clone())),
                };
                let frame = wire::encode_reply(&response, &call).unwrap();
                assert_eq!(
                    wire::read_reply(&mut frame.as_slice(), &call).unwrap(),
                    response
                );
                framed_bytes = frame.len();
                assert!(framed_bytes <= monitor::SERVER_LIMITS.message);
                text_bytes = scan.snapshot.validate_records().unwrap();
                assert!(text_bytes <= MAX_REPLY_TEXT);
                protobuf_fields = discovery::wire::measure(&scan.snapshot).unwrap().fields;
                if let Some(metadata) = &metadata {
                    metadata.check_response(name, &scan.snapshot);
                }
            });
            let root_holders = fixture
                .root_pids()
                .filter(|pid| {
                    scan.snapshot
                        .processes
                        .iter()
                        .any(|p| p.identity.pid == *pid)
                })
                .count();
            if name == "root-explicit-refresh" {
                assert_eq!(
                    root_holders, fixture.load.root_processes,
                    "sample {index}: {:?}",
                    scan.snapshot.coverage
                );
                let report = &scan.snapshot.coverage.elevation;
                // Container root can still lack permission to inspect other users.
                assert!(
                    report.outcome == Outcome::Complete
                        || (report.outcome == Outcome::Partial
                            && !report.issues.is_empty()
                            && report
                                .issues
                                .iter()
                                .all(|i| i.kind == IssueKind::PermissionDenied)),
                    "{report:?}"
                );
            } else if identity {
                assert_eq!(root_holders, 0);
            }
            if name == "record-and-text-budget" {
                assert_eq!(scan.snapshot.coverage.ipv4.outcome, Outcome::Partial);
                assert!(!scan.snapshot.sockets.is_empty());
            } else if budget.is_zero() {
                assert_ne!(scan.snapshot.coverage.ipv4.outcome, Outcome::Complete);
                assert!(elapsed_us < 1_000_000);
            } else {
                assert_eq!(scan.snapshot.coverage.ipv4.outcome, Outcome::Complete);
                for worker in &fixture.workers {
                    assert!(worker.ports.iter().all(|port| {
                        scan.snapshot
                            .sockets
                            .iter()
                            .any(|s| s.endpoint.port == *port)
                    }));
                }
            }
            if refresh {
                allocation_counter::opt_out(|| {
                    eprintln!(
                        "elevation sample {}/{}: {root_holders} fixture holders, {:?}",
                        index + 1,
                        SAMPLES,
                        scan.snapshot.coverage.elevation.outcome
                    );
                });
            }
            samples.push(Sample {
                elapsed_us,
                processes_visited: scan.visited_processes,
                descriptors_visited: scan.visited_descriptors,
                cgroups_visited: scan.visited_cgroups,
                sockets: scan.snapshot.sockets.len(),
                holders: scan.snapshot.processes.len(),
                root_holders,
                framed_bytes,
                text_bytes,
                protobuf_fields,
                ipv4: scan.snapshot.coverage.ipv4.outcome,
                process_coverage: scan.snapshot.coverage.processes.outcome,
                details: scan.snapshot.coverage.details.outcome,
                elevation: scan.snapshot.coverage.elevation.outcome,
                cgroups: scan.snapshot.coverage.cgroups.outcome,
                helpers,
            });
        }
    });
    let mut elapsed: Vec<_> = samples.iter().map(|s| s.elapsed_us).collect();
    elapsed.sort_unstable();
    println!(
        "{}",
        serde_json::json!({
            "scenario": name, "load": fixture.load, "samples": samples,
            "fixture_metadata": metadata,
            "first_coverage": first_coverage,
            "p50_us": elapsed[(elapsed.len() * 50).div_ceil(100) - 1],
            "p95_us": elapsed[(elapsed.len() * 95).div_ceil(100) - 1],
            "collector_thread_peak_bytes": allocation.bytes_max,
            "collector_thread_total_bytes": allocation.bytes_total,
            "collector_thread_retained_bytes": allocation.bytes_current,
            "allocation_scope": "Production collector on this thread, including cache lifetime; excludes child helpers, fixture processes and protobuf round-trip assertions",
            "helper_allocation_scope": "Each completed production helper CLI on its main thread, including request decoding, collection/enrichment and reply encoding; excludes process startup, reporting, allocator overhead, C allocations and other threads. Per-process peaks are not a simultaneous whole-process-tree peak.",
        })
    );
}
