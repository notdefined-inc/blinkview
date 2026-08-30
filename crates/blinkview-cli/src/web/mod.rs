//! A single-purpose localhost server for the review page.
//!
//! Deliberately dependency-free: it serves one page, accepts one POST, and exits.
//! Binding to 127.0.0.1 on an ephemeral port keeps it off the network entirely, and
//! the process is gone the moment the user hits Save.

use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};

const PAGE: &str = include_str!("review.html");

/// Inline the payload into the page so it is a single self-contained document.
pub fn render_page(payload_json: &str) -> String {
    PAGE.replace("window.__REVIEW__", &format!("({payload_json})"))
}

/// Serve the review page until the user submits, then return their choices.
pub fn serve_review(payload_json: &str) -> Result<String> {
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .context("binding localhost port")?;
    let port = listener.local_addr()?.port();
    let url = format!("http://127.0.0.1:{port}/");

    println!("  review page: {url}");
    let _ = std::process::Command::new("open").arg(&url).status();
    println!("  waiting for you to save in the browser… (Ctrl-C to cancel)");

    let page = render_page(payload_json);

    for stream in listener.incoming() {
        let mut stream = stream?;
        match handle(&mut stream, &page)? {
            Some(body) => return Ok(body),
            None => continue,
        }
    }
    anyhow::bail!("review server stopped before anything was saved")
}

/// Returns `Some(body)` once the page POSTs its result.
fn handle(stream: &mut TcpStream, page: &str) -> Result<Option<String>> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(None);
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("/");

    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }

    match (method, path) {
        ("GET", "/") => {
            respond(stream, "200 OK", "text/html; charset=utf-8", page.as_bytes())?;
            Ok(None)
        }
        ("POST", "/save") => {
            let mut body = vec![0u8; content_length];
            reader.read_exact(&mut body)?;
            respond(stream, "200 OK", "application/json", b"{\"ok\":true}")?;
            Ok(Some(String::from_utf8_lossy(&body).to_string()))
        }
        _ => {
            respond(stream, "404 Not Found", "text/plain", b"not found")?;
            Ok(None)
        }
    }
}

fn respond(stream: &mut TcpStream, status: &str, ctype: &str, body: &[u8]) -> Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()?;
    Ok(())
}
