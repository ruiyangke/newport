use super::*;
fn wire(id: &str) -> Result<Value, Error> {
    let mut value = oid(id.as_bytes())?;
    value["format"] = value["algorithm"].take();
    value.as_object_mut().unwrap().remove("algorithm");
    Ok(value)
}
pub(super) fn detail(repo: &Repo, id: &str, budget: usize) -> Result<Value, Error> {
    oid(id.as_bytes())?;
    let mut current = id.to_owned();
    let mut row = json!({"oid":wire(id)?,"annotated":false,"detailsOmitted":false});
    for depth in 0..16 {
        let kind = repo.run(&["cat-file", "-t", &current])?;
        let kind = std::str::from_utf8(trim_line(&kind)).map_err(|_| failure())?;
        if depth == 0 {
            row["objectType"] = json!(kind);
            row["annotated"] = json!(kind == "tag");
            if kind != "tag" {
                row["targetOid"] = wire(&current)?;
            }
        }
        if kind != "tag" {
            row["peeledOid"] = wire(&current)?;
            row["peeledType"] = json!(kind);
            break;
        }
        let size = repo.run(&["cat-file", "-s", &current])?;
        if std::str::from_utf8(trim_line(&size))
            .map_err(|_| failure())?
            .parse::<usize>()
            .map_err(|_| failure())?
            > 2 * 1024 * 1024
        {
            row["detailsOmitted"] = json!(true);
            break;
        }
        let raw = repo.run(&["cat-file", "tag", &current])?;
        let split = raw
            .windows(2)
            .position(|b| b == b"\n\n")
            .unwrap_or(raw.len());
        let mut target = None;
        let mut tagger = Value::Null;
        for line in raw[..split].split(|b| *b == b'\n') {
            if let Some(value) = line.strip_prefix(b"object ") {
                target = Some(
                    std::str::from_utf8(value)
                        .map_err(|_| failure())?
                        .to_owned(),
                );
            }
            if let Some(value) = line.strip_prefix(b"tagger ") {
                let end = value.iter().rposition(|b| *b == b'>').ok_or_else(failure)?;
                let start = value[..end]
                    .iter()
                    .rposition(|b| *b == b'<')
                    .ok_or_else(failure)?;
                let time = std::str::from_utf8(&value[end + 1..])
                    .map_err(|_| failure())?
                    .split_whitespace()
                    .collect::<Vec<_>>();
                if time.len() != 2 {
                    return Err(failure());
                }
                let zone = time[1].parse::<i32>().map_err(|_| failure())?;
                tagger = json!({"name":String::from_utf8_lossy(value[..start].strip_suffix(b" ").unwrap_or(&value[..start])),"email":String::from_utf8_lossy(&value[start+1..end]),"time":time[0].parse::<i64>().map_err(|_|failure())?,"offsetMinutes":zone/100*60+zone%100});
            }
        }
        current = target.ok_or_else(failure)?;
        oid(current.as_bytes())?;
        if depth == 0 {
            let message = raw.get(split + 2..).unwrap_or_default();
            row["targetOid"] = wire(&current)?;
            row["message"] = json!(WirePath::new(&message[..message.len().min(budget)]));
            row["messageTruncated"] = json!(message.len() > budget);
            row["tagger"] = tagger;
        }
        if depth == 15 {
            row["detailsOmitted"] = json!(true);
        }
    }
    Ok(row)
}
pub(super) fn page(
    repo: &Repo,
    count: usize,
    cursor: Option<String>,
    budget: Option<usize>,
) -> Result<Value, Error> {
    let budget = budget.unwrap_or(16384);
    if !(1..=16384).contains(&budget) || !(1..=200).contains(&count) {
        return Err(Error::invalid("Invalid tag page parameters."));
    }
    let bytes = repo.run(&[
        "for-each-ref",
        "--sort=refname",
        "--format=%(refname)%00%(objectname)%00%(symref)",
        "refs/tags/",
    ])?;
    let rows=bytes.split(|b|*b==b'\n').filter(|b|!b.is_empty()).map(|line|{let f=line.split(|b|*b==0).collect::<Vec<_>>();if f.len()!=3{return Err(failure());}Ok(json!({"name":WirePath::new(&f[0][10..]),"reference":WirePath::new(f[0]),"rawOid":std::str::from_utf8(f[1]).map_err(|_|failure())?,"symbolicTarget":if f[2].is_empty(){None}else{Some(WirePath::new(f[2]))}}))}).collect::<Result<Vec<_>,_>>()?;
    let total = rows.len();
    let mut page = pages::page(
        repo,
        format!("cli.tags:{budget}"),
        hex(&Sha256::digest(&bytes)),
        rows,
        count.min(256 * 1024 / (budget * 2 + 2048)),
        cursor,
        json!({"totalEntries":total}),
    )?;
    for row in page["entries"].as_array_mut().ok_or_else(failure)? {
        let details = detail(repo, row["rawOid"].as_str().ok_or_else(failure)?, budget)?;
        let map = row.as_object_mut().unwrap();
        map.remove("rawOid");
        map.extend(details.as_object().unwrap().clone());
    }
    Ok(page)
}
