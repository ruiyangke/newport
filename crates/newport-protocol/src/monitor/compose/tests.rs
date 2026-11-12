use super::*;

fn job() -> Job {
    Job {
        job: "abc".into(),
        project: "app".into(),
        command: "docker compose -p app up -d".into(),
        state: State::Running,
        exit_code: None,
        started_at: 1,
        finished_at: None,
        output: "line\n".into(),
        offset: 5,
    }
}
fn project() -> Project {
    Project {
        cli: Some(Cli {
            kind: CliKind::Plugin,
            version: "2.29.1".into(),
        }),
        cli_error: None,
        working_dir: Some("/srv/app".into()),
        config_files: vec!["/srv/app/compose.yaml".into()],
        files: vec![File {
            path: "/srv/app/compose.yaml".into(),
            text: Some("services: {}".into()),
            error: None,
        }],
        services: Some(vec![Service {
            name: "web".into(),
            image: None,
            build: true,
            profiles: vec![],
        }]),
        config_error: None,
    }
}
#[test]
fn mutations_reject_unusable_arguments_before_execution() {
    let run = |action| Request::Run {
        project: "app".into(),
        action,
    };
    assert!(
        run(Action::Scale {
            service: "web".into(),
            replicas: MAX_REPLICAS
        })
        .validate()
        .is_ok()
    );
    assert!(
        run(Action::Scale {
            service: "web".into(),
            replicas: MAX_REPLICAS + 1
        })
        .validate()
        .is_err()
    );
    assert!(
        run(Action::Pull {
            services: vec!["web".into(); MAX_SERVICES + 1]
        })
        .validate()
        .is_err()
    );
    assert!(
        run(Action::Pull {
            services: vec!["--all".into()]
        })
        .validate()
        .is_err()
    );
    assert!(
        Request::Cancel {
            job: "../job".into()
        }
        .validate()
        .is_err()
    );
    assert!(
        Request::Project {
            project: "App".into()
        }
        .validate()
        .is_err()
    );
}
#[test]
fn job_results_match_identity_and_progress() {
    let request = Request::Job {
        job: "abc".into(),
        offset: 0,
    };
    Reply::Job(job()).validate_for(&request).unwrap();
    assert!(
        Reply::Job(job())
            .validate_for(&Request::Job {
                job: "other".into(),
                offset: 0
            })
            .is_err()
    );
    for state in [State::Succeeded, State::Failed, State::Cancelled] {
        let mut v = job();
        v.state = state;
        assert!(v.validate().is_err());
        v.finished_at = Some(2);
        if state == State::Succeeded {
            v.exit_code = Some(0);
        }
        v.validate().unwrap();
    }
    let mut v = job();
    v.offset = 4;
    assert!(v.validate().is_err());
    v.offset = (MAX_OUTPUT + 1) as u64;
    v.output = "x".repeat(MAX_OUTPUT + 1);
    assert!(v.validate().is_err());
    assert!(
        Reply::Cancelled(Done { ok: false })
            .validate_for(&Request::Cancel { job: "abc".into() })
            .is_err()
    );
}
#[test]
fn project_reply_bounds_include_nested_profiles_and_file_text() {
    let mut p = project();
    p.validate().unwrap();
    p.files[0].text = Some("x".repeat(MAX_FILE_TEXT + 1));
    assert!(p.validate().is_err());
    p.files[0].text = Some("x".repeat(MAX_FILE_TEXT));
    p.files = vec![p.files[0].clone(); MAX_FILES];
    assert!(p.validate().is_err());
    let mut p = project();
    p.services.as_mut().unwrap()[0].profiles = vec![String::new(); MAX_ITEMS];
    assert!(p.validate().is_err());
    let mut p = project();
    p.config_error = Some("invalid".into());
    assert!(p.validate().is_err());
}
#[test]
fn job_lists_have_unique_ids_and_no_output_payload() {
    assert!(Jobs { jobs: vec![job()] }.validate().is_err());
    let mut v = job();
    v.output.clear();
    v.offset = 0;
    Jobs {
        jobs: vec![v.clone()],
    }
    .validate()
    .unwrap();
    assert!(
        Jobs {
            jobs: vec![v.clone(), v.clone()]
        }
        .validate()
        .is_err()
    );
    let jobs = (0..=MAX_JOBS)
        .map(|i| Job {
            job: i.to_string(),
            ..v.clone()
        })
        .collect();
    assert!(Jobs { jobs }.validate().is_err());
}
