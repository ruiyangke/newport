use super::*;
use std::collections::BTreeMap;
#[derive(Clone)]
struct Setting {
    value: Option<Vec<u8>>,
    level: String,
    depth: usize,
}
pub(super) struct Config(BTreeMap<Vec<u8>, Vec<Setting>>);
impl Config {
    fn read(repo: &Repo) -> Result<Self, Error> {
        let bytes = repo.run(&[
            "config",
            "--null",
            "--list",
            "--show-scope",
            "--show-origin",
        ])?;
        let mut records = bytes.split(|b| *b == 0);
        let mut entries: BTreeMap<Vec<u8>, Vec<Setting>> = BTreeMap::new();
        while let Some(scope) = records.next() {
            if scope.is_empty() {
                break;
            }
            let origin = records.next().ok_or_else(failure)?;
            let data = records.next().ok_or_else(failure)?;
            let mut kv = data.splitn(2, |b| *b == b'\n');
            let key = kv.next().ok_or_else(failure)?.to_vec();
            let value = kv.next().map(Vec::from);
            let level = match scope {
                b"system" => "System",
                b"global" => "Global",
                b"local" => "Local",
                b"worktree" => "Worktree",
                _ => "App",
            }
            .to_string();
            let direct = origin.strip_prefix(b"file:").is_some_and(|p| {
                let path = Path::new(OsStr::from_bytes(p));
                repo.root.join(path).canonicalize().ok()
                    == repo.common.join("config").canonicalize().ok()
            });
            entries.entry(key).or_default().push(Setting {
                value,
                level,
                depth: usize::from(!direct),
            });
        }
        Ok(Self(entries))
    }
    fn values(&self, key: &str) -> Vec<Vec<u8>> {
        self.0
            .get(key.as_bytes())
            .into_iter()
            .flatten()
            .filter_map(|s| s.value.clone())
            .collect()
    }
    fn tracking(&self, branch: &str) -> Result<Value, Error> {
        let mut fields = serde_json::Map::new();
        let mut editable = true;
        for suffix in ["remote", "merge"] {
            let key = format!("branch.{branch}.{suffix}");
            let values = self.0.get(key.as_bytes()).cloned().unwrap_or_default();
            editable &= values.len() <= 1
                && values
                    .iter()
                    .all(|s| s.level == "Local" && s.depth == 0 && s.value.is_some());
            fields.insert(suffix.into(),json!(values.iter().map(|s|json!({"value":s.value.as_deref().map(WirePath::new),"level":s.level,"includeDepth":s.depth})).collect::<Vec<_>>()));
        }
        let configuration = Value::Object(fields);
        let token = pages::hash(&configuration)?;
        Ok(json!({"token":token,"editable":editable,"configuration":configuration}))
    }
}
pub(super) fn branches(
    repo: &Repo,
    filter: String,
    kind: Option<String>,
    count: usize,
    cursor: Option<String>,
) -> Result<Value, Error> {
    let kind = kind.as_deref().unwrap_or("all");
    if filter.len() > 1024 || filter.contains('\0') || !["all", "local", "remote"].contains(&kind) {
        return Err(Error::invalid("Invalid branch filter."));
    }
    let config = Config::read(repo)?;
    let bytes = repo.run(&[
        "for-each-ref",
        "--sort=refname",
        "--format=%(refname)%00%(objectname)%00%(HEAD)%00%(upstream)",
        "refs/heads/",
        "refs/remotes/",
    ])?;
    let mut rows = Vec::new();
    for line in bytes.split(|b| *b == b'\n').filter(|r| !r.is_empty()) {
        let f = line.split(|b| *b == 0).collect::<Vec<_>>();
        if f.len() != 4 {
            return Err(failure());
        }
        let local = f[0].starts_with(b"refs/heads/");
        let name = &f[0][if local { 11 } else { 13 }..];
        if (kind == "local" && !local)
            || (kind == "remote" && local)
            || !String::from_utf8_lossy(name)
                .to_lowercase()
                .contains(&filter.to_lowercase())
        {
            continue;
        }
        rows.push(json!({"name":WirePath::new(name),"reference":WirePath::new(f[0]),"oid":oid(f[1])?,"remote":!local,"current":f[2]==b"*","upstream":if f[3].is_empty(){None}else{Some(WirePath::new(f[3]))},"tracking":if local{Some(config.tracking(std::str::from_utf8(name).map_err(|_|Error::new("UNSUPPORTED_CAPABILITY","Non-UTF-8 branch configuration is unsupported."))?)?)}else{None}}));
    }
    let fingerprint = pages::hash(&json!(rows))?;
    let total = rows.len();
    pages::page(
        repo,
        format!("cli.branches:{}", json!([filter, kind])),
        fingerprint,
        rows,
        count,
        cursor,
        json!({"totalEntries":total}),
    )
}
pub(super) fn tracking(repo: &Repo, branch: &str) -> Result<Value, Error> {
    Config::read(repo)?.tracking(branch)
}
/// Git resolves branch configuration and fetch refspecs. A tracking ref's path
/// is not sufficient to recover either the remote name or its source branch.
pub(super) fn upstream(repo: &Repo, head: &Value) -> Result<Value, Error> {
    if head["detached"] == true || head["unborn"] == true {
        return Ok(Value::Null);
    }
    let reference: WirePath =
        serde_json::from_value(head["name"].clone()).map_err(|_| failure())?;
    let reference = reference.decode()?;
    let Some(reference) = std::str::from_utf8(&reference).ok() else {
        return Ok(Value::Null);
    };
    let bytes = repo.run(&[
        "for-each-ref",
        "--format=%(refname)%00%(upstream)%00%(upstream:remotename)%00%(upstream:remoteref)",
        "--",
        reference,
    ])?;
    // for-each-ref also matches descendants. Only the exact current ref owns
    // this configuration; never select a similarly named branch.
    for row in bytes.split(|b| *b == b'\n').filter(|row| !row.is_empty()) {
        let fields: Vec<_> = row.split(|b| *b == 0).collect();
        if fields.len() != 4 {
            return Err(failure());
        }
        if fields[0] != reference.as_bytes() {
            continue;
        }
        if fields[1..].iter().any(|field| field.is_empty()) {
            return Ok(Value::Null);
        }
        return Ok(
            json!({"upstream":WirePath::new(fields[1]),"remote":WirePath::new(fields[2]),"reference":WirePath::new(fields[3])}),
        );
    }
    Ok(Value::Null)
}
/// Repository-level settings for the settings page: the prune-on-fetch
/// default and the commit identity, with one token over their effective
/// values so a concurrent configuration edit fails the write instead of
/// racing it.
pub(super) fn configuration(config: &Config) -> Result<Value, Error> {
    let effective = |key: &str| config.values(key).last().cloned();
    // git-config boolean semantics; an unrecognized value reads as false.
    let fetch_prune = effective("fetch.prune").is_some_and(|value| {
        matches!(
            value.to_ascii_lowercase().as_slice(),
            b"true" | b"yes" | b"on" | b"1"
        )
    });
    let text = |key: &str| {
        effective(key).and_then(|value| std::str::from_utf8(&value).ok().map(str::to_owned))
    };
    let token = pages::hash(&json!({
        "fetchPrune": config.values("fetch.prune"),
        "userName": config.values("user.name"),
        "userEmail": config.values("user.email"),
    }))?;
    Ok(json!({
        "fetchPrune": fetch_prune,
        "userName": text("user.name"),
        "userEmail": text("user.email"),
        "token": token,
    }))
}
/// Re-reads the repository configuration and rejects a write whose token no
/// longer matches, as remote writes do for their own tokens.
pub(super) fn configuration_token(repo: &Repo, expected: &str) -> Result<(), Error> {
    if configuration(&Config::read(repo)?)?["token"] != expected {
        return Err(Error::new(
            "STALE_CONFIGURATION",
            "Repository configuration changed. Refresh before continuing.",
        ));
    }
    Ok(())
}
fn display(value: Option<&Vec<u8>>) -> Option<String> {
    super::super::remotes::display_url(value.and_then(|v| std::str::from_utf8(v).ok()))
}

fn describe(config: &Config, name: &str) -> Result<Value, Error> {
    if name.is_empty() || name.len() > 1024 || name.starts_with('-') || name.contains(['\0', '\n'])
    {
        return Err(Error::invalid("Invalid remote name."));
    }
    let urls = config.values(&format!("remote.{name}.url"));
    let push = config.values(&format!("remote.{name}.pushurl"));
    if urls.is_empty() {
        return Err(Error::new(
            "REMOTE_NOT_FOUND",
            "The remote no longer exists.",
        ));
    }
    let token = pages::hash(
        &json!({"url":urls,"pushUrl":push,"fetch":config.values(&format!("remote.{name}.fetch")),"push":config.values(&format!("remote.{name}.push"))}),
    )?;
    Ok(
        json!({"name":name,"url":display(urls.first()),"pushUrl":display(push.first()),"token":token}),
    )
}
pub(super) fn remote(repo: &Repo, name: &str) -> Result<Value, Error> {
    describe(&Config::read(repo)?, name)
}
pub(super) fn names(repo: &Repo) -> Result<Vec<String>, Error> {
    let bytes = repo.run(&["remote"])?;
    std::str::from_utf8(&bytes)
        .map_err(|_| failure())
        .map(|v| v.lines().map(str::to_owned).collect())
}
pub(super) fn remotes(repo: &Repo) -> Result<Value, Error> {
    let config = Config::read(repo)?;
    let rows = names(repo)?
        .iter()
        .map(|n| describe(&config, n))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(
        json!({"entries":rows,"authentication":{"ssh":"server_agent","https":"server_helpers"},"configuration":configuration(&config)?}),
    )
}
pub(super) fn remote_names(
    repo: &Repo,
    filter: String,
    count: usize,
    cursor: Option<String>,
) -> Result<Value, Error> {
    if filter.len() > 1024 || filter.contains('\0') {
        return Err(Error::invalid("Invalid remote filter."));
    }
    let rows = names(repo)?
        .into_iter()
        .filter(|n| n.to_lowercase().contains(&filter.to_lowercase()))
        .map(|name| json!({"name":name}))
        .collect::<Vec<_>>();
    let total = rows.len();
    pages::page(
        repo,
        format!("cli.remote_names:{filter}"),
        pages::hash(&json!(rows))?,
        rows,
        count,
        cursor,
        json!({"totalEntries":total}),
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn remote_refs(
    repo: &Repo,
    name: &str,
    token: &str,
    push: bool,
    filter: String,
    count: usize,
    cursor: Option<String>,
) -> Result<Value, Error> {
    if remote(repo, name)?["token"] != token {
        return Err(Error::new("STALE_REMOTE", "Remote configuration changed."));
    }
    if filter.len() > 1024 || filter.contains('\0') {
        return Err(Error::invalid("Invalid remote ref filter."));
    }
    let destination = if push {
        let url = repo.run(&["remote", "get-url", "--push", name])?;
        std::str::from_utf8(trim_line(&url))
            .map_err(|_| failure())?
            .to_owned()
    } else {
        name.to_owned()
    };
    let data = repo.run(&["ls-remote", "--symref", "--", &destination])?;
    let mut symbols = BTreeMap::new();
    let mut rows = Vec::new();
    for line in data.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        let at = line.iter().position(|b| *b == b'\t').ok_or_else(failure)?;
        let (left, right) = (&line[..at], &line[at + 1..]);
        if let Some(target) = left.strip_prefix(b"ref: ") {
            symbols.insert(right.to_vec(), target.to_vec());
            continue;
        }
        if !String::from_utf8_lossy(right)
            .to_lowercase()
            .contains(&filter.to_lowercase())
        {
            continue;
        }
        rows.push(json!({"reference":WirePath::new(right),"kind":if right==b"HEAD"{"head"}else if right.starts_with(b"refs/heads/"){"branch"}else if right.ends_with(b"^{}"){"peeled_tag"}else if right.starts_with(b"refs/tags/"){"tag"}else{"other"},"oid":oid(left)?,"symbolicTarget":symbols.get(right).map(|b|WirePath::new(b))}));
    }
    let total = rows.len();
    pages::page(
        repo,
        format!("cli.remote_refs:{}", json!([name, token, push, filter])),
        pages::hash(&json!(rows))?,
        rows,
        count,
        cursor,
        json!({"remote":name,"remoteToken":token,"forPush":push,"basis":"remote_advertisement","truncated":false,"totalEntries":total}),
    )
}
