use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use mcctl_platform::{Artifact, Digest, Error, Loader, Plan, download};

fn plan(url: &str, dest: &str) -> Plan {
    Plan {
        loader: Loader::Vanilla,
        minecraft: "26.3".to_owned(),
        loader_version: None,
        artifacts: vec![Artifact {
            url: url.to_owned(),
            digest: Some(Digest::Sha1("0".repeat(40))),
            dest: PathBuf::from(dest),
        }],
        installer: None,
    }
}

fn offline_agent() -> ureq::Agent {
    ureq::Agent::new_with_defaults()
}

const URL: &str = "https://127.0.0.1:9/server.jar";

#[test]
fn download_rejects_escaping_dest() {
    let root = tempfile::tempdir().unwrap();
    let staging = root.path().join("staging");
    fs::create_dir(&staging).unwrap();
    for dest in ["../server.jar", "/tmp/server.jar", "a/../../server.jar", ""] {
        let result = download(&offline_agent(), &plan(URL, dest), &staging);
        assert!(matches!(result, Err(Error::UnsafePath { .. })), "{dest}");
    }
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    assert_eq!(fs::read_dir(&staging).unwrap().count(), 0);
}

#[test]
fn download_refuses_symlinked_folders() {
    let root = tempfile::tempdir().unwrap();
    let outside = root.path().join("outside");
    let staging = root.path().join("staging");
    fs::create_dir(&outside).unwrap();
    fs::create_dir(&staging).unwrap();
    symlink(&outside, staging.join("libraries")).unwrap();

    let result = download(&offline_agent(), &plan(URL, "libraries/x.jar"), &staging);
    assert!(matches!(result, Err(Error::UnsafePath { .. })));
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
}

#[test]
fn download_refuses_symlinked_staging() {
    let root = tempfile::tempdir().unwrap();
    let real = root.path().join("real");
    fs::create_dir(&real).unwrap();
    let staging = root.path().join("staging");
    symlink(&real, &staging).unwrap();

    let result = download(&offline_agent(), &plan(URL, "server.jar"), &staging);
    assert!(matches!(result, Err(Error::UnsafePath { .. })));
}

#[test]
fn download_refuses_existing_dest() {
    let staging = tempfile::tempdir().unwrap();
    fs::write(staging.path().join("server.jar"), "keep").unwrap();

    let result = download(&offline_agent(), &plan(URL, "server.jar"), staging.path());
    assert!(matches!(result, Err(Error::AlreadyExists { .. })));
    assert_eq!(
        fs::read(staging.path().join("server.jar")).unwrap(),
        b"keep"
    );
}

#[test]
fn download_refuses_plain_http() {
    let staging = tempfile::tempdir().unwrap();
    let result = download(
        &offline_agent(),
        &plan("http://example.com/server.jar", "server.jar"),
        staging.path(),
    );
    assert!(matches!(result, Err(Error::InsecureUrl { .. })));
    assert!(!Path::new(&staging.path().join("server.jar")).exists());
}
