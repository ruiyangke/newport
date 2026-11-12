use super::*;
use std::{fs, os::unix::fs::PermissionsExt, sync::Arc};

const CASE: &str = "NEWPORT_DISCOVERY_IMAGE_FIXTURE_CASE";
const DIRECTORY: &str = "NEWPORT_DISCOVERY_IMAGE_FIXTURE_DIRECTORY";

#[test]
fn selects_platform_from_image_index_and_preserves_unowned_images() -> Result<()> {
    if let Ok(case) = std::env::var(CASE) {
        return fixture_case(&case);
    }
    for case in ["absent", "cached", "changed", "unavailable"] {
        let temp = tempfile::tempdir()?;
        let script = temp.path().join("docker");
        fs::write(&script, DOCKER)?;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700))?;
        process::Process::spawn(
            command(std::env::current_exe()?)
                .args([
                    "--exact",
                    "discovery::matrix::fixture::tests::selects_platform_from_image_index_and_preserves_unowned_images",
                    "--nocapture",
                ])
                .env(CASE, case)
                .env(DIRECTORY, temp.path())
                .env("NEWPORT_TEST_DOCKER", &script),
        )?
        .success(Duration::from_secs(30))
        .with_context(|| format!("Image fixture case: {case}"))?;
    }
    Ok(())
}

fn fixture_case(case: &str) -> Result<()> {
    let directory =
        std::path::PathBuf::from(std::env::var_os(DIRECTORY).context("Missing fixture")?);
    let mut runner = Supervisor::new(directory.join("evidence"), 0, Arc::default())?;
    let mut fixture = Fixture::new("fixture".into(), "example:fixture".into(), "owner".into())?;
    let result = fixture.provision(Distro::Debian12, Architecture::X86_64, &mut runner);
    let cleanup = fixture.cleanup();
    cleanup?;
    if case == "unavailable" {
        assert!(result.is_err());
        assert!(!directory.join("created").exists());
    } else {
        result?;
        let image = fixture.image.as_ref().context("Missing selected image")?;
        assert_eq!(image.architecture, "amd64");
        assert_eq!(image.id, "sha256:platform");
        assert!(directory.join("container-removed").exists());
    }
    assert_eq!(
        directory.join("pulled").exists(),
        matches!(case, "absent" | "changed")
    );
    assert_eq!(directory.join("image-removed").exists(), case == "absent");
    assert!(fixture.container_removed);
    Ok(())
}

const DOCKER: &str = r##"#!/bin/sh
set -eu
cd "$NEWPORT_DISCOVERY_IMAGE_FIXTURE_DIRECTORY"
case "$1 $2" in
  'image inspect')
    shift 2
    if [ "$1" = '--help' ]; then
      printf '      --platform string   Inspect a specific platform\n'
      exit 0
    fi
    platform=''
    if [ "$1" = '--platform' ]; then
      test "$2" = 'linux/amd64'
      platform="$2"
      shift 2
    fi
    test "$#" = 1
    case "$NEWPORT_DISCOVERY_IMAGE_FIXTURE_CASE" in
      absent|changed) test -e pulled || exit 1 ;;
    esac
    if [ -n "$platform" ]; then
      test "$1" = 'sha256:index'
      test "$NEWPORT_DISCOVERY_IMAGE_FIXTURE_CASE" != unavailable || exit 1
      printf '[{"Id":"sha256:platform","Architecture":"amd64","Os":"linux","RepoDigests":["example@sha256:index"]}]'
    elif [ "$NEWPORT_DISCOVERY_IMAGE_FIXTURE_CASE" = changed ] && [ -e created ]; then
      test "$1" = 'example:fixture'
      printf '[{"Id":"sha256:changed","Architecture":"","Os":"","RepoDigests":[]}]'
    else
      test "$1" = 'example:fixture'
      printf '[{"Id":"sha256:index","Architecture":"","Os":"","RepoDigests":["example@sha256:index"]}]'
    fi ;;
  'info --format') printf daemon ;;
  'pull --platform')
    test "$3" = 'linux/amd64' && test "$4" = 'example:fixture'
    touch pulled ;;
  'run --detach')
    case " $* " in *' --platform linux/amd64 '*) ;; *) exit 2 ;; esac
    case " $* " in *' sha256:index '*) ;; *) exit 2 ;; esac
    touch created
    printf '%064d' 1 ;;
  'exec --user')
    shift 4
    case "$1" in
      sh) : ;;
      df) printf 'Filesystem 1024-blocks Used Available Capacity Mounted on\nfixture 60000000 0 60000000 0%% /\n' ;;
      cat) printf 'ID=fixture\n' ;;
      uname) printf 'Linux fixture\n' ;;
      *) exit 2 ;;
    esac ;;
  'container ls')
    case " $* " in *' name=^/fixture$ '*) ;; *) exit 2 ;; esac
    case " $* " in *' label=newport.discovery.matrix=owner '*) ;; *) exit 2 ;; esac
    if [ -e created ]; then printf '%064d' 1; fi ;;
  'rm --force') touch container-removed ;;
  'image rm')
    test "$3" = 'example:fixture'
    test "$NEWPORT_DISCOVERY_IMAGE_FIXTURE_CASE" = absent
    touch image-removed ;;
  *) exit 2 ;;
esac
"##;
