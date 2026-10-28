//! Reviewed upstream transfers.
use base64::Engine as _;
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Fetch,
    Pull,
    Push,
}
impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Fetch => "Fetch",
            Self::Pull => "Pull",
            Self::Push => "Push",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub snapshot: String,
    pub remote: String,
    pub destination: String,
    pub branch: String,
    pub oid: String,
    pub ahead: u64,
    pub behind: u64,
}
fn reference(value: &Value) -> Option<&str> {
    let display = value["display"].as_str()?;
    (value["bytesB64"] == base64::engine::general_purpose::STANDARD.encode(display))
        .then_some(display)
}
impl Target {
    pub fn from_status(repo: &Value, status: &Value) -> Result<Self, String> {
        if repo["capabilities"]["readOnly"] != false
            || repo["capabilities"]["workingTree"] != true
            || repo["bare"] != false
        {
            return Err("This repository cannot be written from here.".into());
        }
        let metadata = &status["metadata"];
        let head = &metadata["head"];
        if head["detached"] != false
            || head["unborn"] != false
            || !metadata["integration"].is_null()
        {
            return Err(
                "Finish the current integration and select an existing local branch.".into(),
            );
        }
        let branch = reference(&head["name"])
            .and_then(|name| name.strip_prefix("refs/heads/"))
            .filter(|name| !name.is_empty())
            .ok_or("A valid local branch is required.")?;
        let upstream = &metadata["upstream"];
        let remote = reference(&upstream["remote"])
            .filter(|name| !name.is_empty() && *name != ".")
            .ok_or("This branch has no remote upstream. Configure one in the branch menu, or use Remotes.")?;
        let destination = reference(&upstream["reference"])
            .and_then(|name| name.strip_prefix("refs/heads/"))
            .filter(|name| !name.is_empty())
            .ok_or("The upstream destination is not a branch.")?;
        let oid = head["oid"]["hex"]
            .as_str()
            .filter(|oid| {
                matches!(oid.len(), 40 | 64)
                    && oid.bytes().all(|c| c.is_ascii_hexdigit())
                    && oid.bytes().any(|c| c != b'0')
            })
            .ok_or("The current branch tip is unavailable.")?;
        let snapshot = status["snapshot"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("Refresh repository status first.")?;
        Ok(Self {
            snapshot: snapshot.into(),
            remote: remote.into(),
            destination: destination.into(),
            branch: branch.into(),
            oid: oid.into(),
            ahead: metadata["ahead"].as_u64().unwrap_or(0),
            behind: metadata["behind"].as_u64().unwrap_or(0),
        })
    }
    pub fn preferred(&self) -> Kind {
        if self.behind > 0 {
            Kind::Pull
        } else if self.ahead > 0 {
            Kind::Push
        } else {
            Kind::Fetch
        }
    }
    pub fn action(&self, kind: Kind, remote: &Value) -> Result<Value, String> {
        if remote["name"].as_str() != Some(self.remote.as_str()) {
            return Err("Remote identity changed. Review the transfer again.".into());
        }
        let token = remote["token"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("Remote configuration is unavailable.")?;
        Ok(match kind {
            Kind::Fetch => json!({"kind":"fetch","remote":self.remote,"expectedToken":token}),
            Kind::Pull => {
                json!({"kind":"pull.fast_forward","remote":self.remote,"expectedToken":token,"remoteBranch":self.destination})
            }
            Kind::Push => {
                json!({"kind":"push","remote":self.remote,"expectedToken":token,"branch":self.branch,"expectedOid":self.oid,"destinationBranch":self.destination})
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn reference(name: &str) -> Value {
        json!({"display":name,"bytesB64":base64::engine::general_purpose::STANDARD.encode(name)})
    }
    fn fixture() -> (Value, Value) {
        (
            json!({"bare":false,"capabilities":{"workingTree":true,"readOnly":false}}),
            json!({"snapshot":"snapshot", "metadata":{"head":{"detached":false,"unborn":false,"name":reference("refs/heads/topic"),"oid":{"hex":"a".repeat(40)}},"upstreamRef":reference("refs/remotes/origin/release/next"),"upstream":{"remote":reference("origin"),"reference":reference("refs/heads/release/next")},"ahead":2,"behind":3}}),
        )
    }
    #[test]
    fn transfers_use_upstream_destination_and_exact_reviewed_identities() {
        let (repo, mut status) = fixture();
        let target = Target::from_status(&repo, &status).unwrap();
        assert_eq!(target.preferred(), Kind::Pull);
        let remote = json!({"name":"origin","token":"config-token"});
        let push = target.action(Kind::Push, &remote).unwrap();
        assert_eq!(push["branch"], "topic");
        assert_eq!(push["destinationBranch"], "release/next");
        assert_eq!(push["expectedOid"], "a".repeat(40));
        assert_eq!(
            target.action(Kind::Pull, &remote).unwrap()["remoteBranch"],
            "release/next"
        );
        assert_eq!(
            target.action(Kind::Fetch, &remote).unwrap()["expectedToken"],
            "config-token"
        );
        assert!(
            target
                .action(Kind::Push, &json!({"name":"elsewhere","token":"token"}))
                .is_err()
        );
        assert!(
            target
                .action(Kind::Push, &json!({"name":"origin"}))
                .is_err()
        );
        status["metadata"]["behind"] = json!(0);
        assert_eq!(
            Target::from_status(&repo, &status).unwrap().preferred(),
            Kind::Push
        );
        status["metadata"]["ahead"] = json!(0);
        assert_eq!(
            Target::from_status(&repo, &status).unwrap().preferred(),
            Kind::Fetch
        );
    }
    #[test]
    fn custom_refspec_and_remote_name_use_git_resolved_destination() {
        let (repo, mut status) = fixture();
        status["metadata"]["upstreamRef"] = reference("refs/remotes/cache/local-alias");
        status["metadata"]["upstream"]["remote"] = reference("team/upstream");
        let target = Target::from_status(&repo, &status).unwrap();
        assert_eq!(target.remote, "team/upstream");
        assert_eq!(target.destination, "release/next");
        for kind in [Kind::Fetch, Kind::Pull, Kind::Push] {
            let action = target
                .action(kind, &json!({"name":"team/upstream","token":"reviewed"}))
                .unwrap();
            assert_eq!(action["remote"], "team/upstream");
        }
        // Without the resolved upstream the UI never guesses from a tracking path.
        status["metadata"]
            .as_object_mut()
            .unwrap()
            .remove("upstream");
        assert!(Target::from_status(&repo, &status).is_err());
    }
    #[test]
    fn read_only_unborn_detached_integration_and_lossy_refs_are_rejected() {
        let (mut repo, status) = fixture();
        repo["capabilities"]["readOnly"] = json!(true);
        assert!(Target::from_status(&repo, &status).is_err());
        repo["capabilities"]["readOnly"] = json!(false);
        for (pointer, value) in [
            ("/metadata/head/detached", json!(true)),
            ("/metadata/head/unborn", json!(true)),
            ("/metadata/head/name/bytesB64", json!("/w==")),
            ("/metadata/upstream/remote/bytesB64", json!("/w==")),
            ("/metadata/upstream/reference/bytesB64", json!("/w==")),
            ("/metadata/upstream/remote", reference(".")),
            ("/snapshot", json!("")),
        ] {
            let mut invalid = status.clone();
            *invalid.pointer_mut(pointer).unwrap() = value;
            assert!(Target::from_status(&repo, &invalid).is_err(), "{pointer}");
        }
        let mut invalid = status;
        invalid["metadata"]["integration"] = json!({"kind":"merge"});
        assert!(Target::from_status(&repo, &invalid).is_err());
    }
}
