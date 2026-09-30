use super::*;
pub(super) fn rows(repo: &Repo) -> Result<(Vec<Value>, String), Error> {
    let bytes = command::run(
        &repo.root,
        &[
            "log",
            "-g",
            "-z",
            "--format=%H%x00%gs%x00%ct",
            "refs/stash",
            "--",
        ],
    )?;
    let bytes = bytes.unwrap_or_default();
    let fields = bytes.split(|b| *b == 0).collect::<Vec<_>>();
    let mut rows = Vec::new();
    for chunk in fields.chunks(3) {
        if chunk.len() == 1 && chunk[0].is_empty() {
            break;
        }
        if chunk.len() != 3 {
            return Err(failure());
        }
        let id = std::str::from_utf8(chunk[0]).map_err(|_| failure())?;
        oid(id.as_bytes())?;
        rows.push(json!({"index":rows.len(),"oid":id,"previousOid":"","message":String::from_utf8_lossy(&chunk[1][..chunk[1].len().min(1024)]),"messageTruncated":chunk[1].len()>1024,"time":std::str::from_utf8(chunk[2]).map_err(|_|failure())?.parse::<i64>().map_err(|_|failure())?}));
    }
    for i in 0..rows.len() {
        rows[i]["previousOid"] = if i + 1 < rows.len() {
            rows[i + 1]["oid"].clone()
        } else {
            json!("0".repeat(rows[i]["oid"].as_str().unwrap().len()))
        };
    }
    Ok((rows, hex(&Sha256::digest(bytes))))
}
pub(super) fn page(repo: &Repo, count: usize, cursor: Option<String>) -> Result<Value, Error> {
    let (rows, token) = rows(repo)?;
    let total = rows.len();
    pages::page(
        repo,
        "cli.stashes".into(),
        token.clone(),
        rows,
        count,
        cursor,
        json!({"listToken":token,"totalEntries":total}),
    )
}
