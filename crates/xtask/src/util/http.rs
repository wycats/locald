use anyhow::Result;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

pub fn get_status_code(host: &str, port: &str, host_header: &str, path: &str) -> Result<u16> {
    let addr = format!("{host}:{port}");
    let mut stream = TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;

    let req = format!("GET {path} HTTP/1.1\r\nHost: {host_header}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes())?;
    stream.flush()?;

    let mut buf = Vec::new();
    stream.read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf);
    let first_line = text
        .lines()
        .next()
        .ok_or_else(|| anyhow::anyhow!("empty HTTP response"))?;

    // e.g. HTTP/1.1 200 OK
    let mut parts = first_line.split_whitespace();
    let _http = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("bad status line"))?;
    let code = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("bad status line"))?
        .parse::<u16>()?;

    Ok(code)
}
