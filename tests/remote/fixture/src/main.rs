use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    time::Duration,
};
mod shell;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn home() -> PathBuf {
    std::env::var_os("HOME").expect("HOME").into()
}
fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let action = args.next().ok_or("Missing fixture operation")?;
    let arg = args.next().unwrap_or_default();
    match action.as_str() {
        "shell-setup" => shell::run()?,
        "http" | "callback" => {
            let port: u16 = arg.parse()?;
            let listener = TcpListener::bind(("127.0.0.1", port))?;
            println!("ready");
            std::io::stdout().flush()?;
            for stream in listener.incoming() {
                let mut stream = stream?;
                let result = (|| -> Result<()> {
                    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
                    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
                    let mut header = Vec::new();
                    let mut byte = [0];
                    while !header.ends_with(b"\r\n\r\n") {
                        if stream.read(&mut byte)? == 0 {
                            return Err("Early HTTP EOF".into());
                        }
                        header.push(byte[0]);
                        if header.len() > 8192 {
                            return Err("HTTP header too large".into());
                        }
                    }
                    let body = if action == "callback" {
                        if !header.starts_with(b"GET /callback?code=fixture&state=nonce HTTP/") {
                            return Err("Unexpected callback".into());
                        }
                        b"logged-in".to_vec()
                    } else {
                        fs::read("/srv/fixture/index.html")?
                    };
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )?;
                    stream.write_all(&body)?;
                    Ok(())
                })();
                if action == "callback" {
                    result?;
                    break;
                }
            }
        }
        "http-status" => {
            let port: u16 = arg.parse()?;
            let mut stream = TcpStream::connect(("127.0.0.1", port))?;
            stream.set_read_timeout(Some(Duration::from_secs(5)))?;
            stream.write_all(b"GET / HTTP/1.0\r\nHost: localhost\r\n\r\n")?;
            let mut response = String::new();
            stream.take(65536).read_to_string(&mut response)?;
            println!(
                "{}",
                response
                    .split_whitespace()
                    .nth(1)
                    .ok_or("Invalid HTTP response")?
            );
        }
        "preview" => {
            let dir = Path::new(&arg);
            assert!(arg.starts_with("/home/fixture/preview-") && !arg.contains(".."));
            fs::create_dir(dir)?;
            fs::write(dir.join("hello 世界.html"), "<script>inert 世界</script>")?;
            fs::write(dir.join("binary.dat"), [0, 255, 1])?;
            fs::File::create(dir.join("large.bin"))?.set_len(17 * 1024 * 1024)?;
            fs::write(dir.join("long.txt"), vec![b'x'; 1024 * 1024 + 1])?;
            std::os::unix::fs::symlink(dir.join("hello 世界.html"), dir.join("link.txt"))?;
            std::os::unix::fs::symlink(dir.join("missing"), dir.join("broken"))?;
        }
        "fill-disk" => {
            assert!(arg.starts_with("/fault-disk/fill-") && !arg.contains(".."));
            let mut file = fs::File::create(arg)?;
            loop {
                if let Err(e) = file.write_all(&[b'x'; 65536]) {
                    if e.raw_os_error() != Some(libc::ENOSPC) {
                        return Err(e.into());
                    }
                    println!("ENOSPC");
                    break;
                }
            }
        }
        "interrupt-journal" => {
            let data: serde_json::Value = serde_json::from_reader(std::io::stdin())?;
            let file = data["file"].as_str().ok_or("Missing filename")?;
            assert!(file.ends_with(".json") && Path::new(file).file_name() == Some(file.as_ref()));
            let path = home().join(".local/state/newport/git/records").join(file);
            let mut record: serde_json::Value = serde_json::from_slice(&fs::read(&path)?)?;
            assert_eq!(record["state"], "succeeded");
            record["state"] = "running".into();
            fs::write(path, serde_json::to_vec(&record)?)?;
        }
        _ => return Err("Unknown fixture operation".into()),
    }
    Ok(())
}
