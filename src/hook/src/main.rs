use std::io::Read;
use std::io::Write;
use std::net::TcpStream;
use std::time::Duration;

use anyhow::{anyhow, Context};
use serde_json::Value;

#[derive(Debug, Default)]
struct Args {
    agent: String,
    event: String,
    launch_id: String,
    profile_id: Option<String>,
    launch_target: Option<String>,
    server: String,
}

fn main() {
    // Hooks should never interrupt the coding agent. Codex treats some stdout
    // as additional prompt context, so keep success and failure paths silent.
    if let Err(error) = run() {
        if std::env::var_os("VIBEAROUND_HOOK_DEBUG").is_some() {
            eprintln!("[vibearound-hook] {error:#}");
        }
    }
}

fn run() -> anyhow::Result<()> {
    let args = parse_args(std::env::args().skip(1))?;
    if args.agent != "codex" {
        return Err(anyhow!("unsupported agent hook '{}'", args.agent));
    }

    let mut stdin = String::new();
    std::io::stdin()
        .read_to_string(&mut stdin)
        .context("read hook stdin")?;
    let payload = if stdin.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&stdin).context("parse hook stdin json")?
    };

    let body = serde_json::to_string(&serde_json::json!({
        "launch_id": args.launch_id,
        "profile_id": args.profile_id,
        "launch_target": args.launch_target,
        "event": args.event,
        "payload": payload,
    }))
    .context("encode hook envelope")?;
    post_json(
        &args.server,
        "/va/internal/agent-hooks/codex",
        body.as_bytes(),
    )
    .context("post hook event")?;

    Ok(())
}

fn post_json(server_url: &str, path: &str, body: &[u8]) -> anyhow::Result<()> {
    let endpoint = parse_http_endpoint(server_url)?;
    let mut stream = TcpStream::connect((&*endpoint.host, endpoint.port))
        .with_context(|| format!("connect {}:{}", endpoint.host, endpoint.port))?;
    let timeout = Some(Duration::from_secs(2));
    stream.set_read_timeout(timeout).ok();
    stream.set_write_timeout(timeout).ok();

    let request_path = format!("{}{}", endpoint.base_path, path);
    write!(
        stream,
        "POST {request_path} HTTP/1.1\r\n\
         Host: {}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n",
        endpoint.host,
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()?;

    let mut response = String::new();
    stream.read_to_string(&mut response).ok();
    if response.starts_with("HTTP/1.1 2") || response.starts_with("HTTP/1.0 2") {
        return Ok(());
    }

    Err(anyhow!("server returned non-2xx response"))
}

struct HttpEndpoint {
    host: String,
    port: u16,
    base_path: String,
}

fn parse_http_endpoint(server_url: &str) -> anyhow::Result<HttpEndpoint> {
    let without_scheme = server_url
        .strip_prefix("http://")
        .ok_or_else(|| anyhow!("only http:// hook servers are supported"))?;
    let (authority, path) = without_scheme
        .split_once('/')
        .map(|(authority, path)| (authority, format!("/{path}")))
        .unwrap_or((without_scheme, String::new()));
    let (host, port) = authority
        .rsplit_once(':')
        .ok_or_else(|| anyhow!("hook server must include a port"))?;
    let port = port.parse::<u16>().context("parse hook server port")?;
    let base_path = path.trim_end_matches('/').to_string();
    Ok(HttpEndpoint {
        host: host.to_string(),
        port,
        base_path,
    })
}

fn parse_args<I>(mut args: I) -> anyhow::Result<Args>
where
    I: Iterator<Item = String>,
{
    let mut parsed = Args::default();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--agent" => parsed.agent = take_value(&mut args, "--agent")?,
            "--event" => parsed.event = take_value(&mut args, "--event")?,
            "--launch-id" => parsed.launch_id = take_value(&mut args, "--launch-id")?,
            "--profile-id" => parsed.profile_id = Some(take_value(&mut args, "--profile-id")?),
            "--launch-target" => {
                parsed.launch_target = Some(take_value(&mut args, "--launch-target")?)
            }
            "--server" => parsed.server = take_value(&mut args, "--server")?,
            _ => return Err(anyhow!("unknown argument '{}'", arg)),
        }
    }

    if parsed.agent.is_empty() {
        return Err(anyhow!("missing --agent"));
    }
    if parsed.event.is_empty() {
        return Err(anyhow!("missing --event"));
    }
    if parsed.launch_id.is_empty() {
        return Err(anyhow!("missing --launch-id"));
    }
    if parsed.server.is_empty() {
        return Err(anyhow!("missing --server"));
    }

    Ok(parsed)
}

fn take_value<I>(args: &mut I, flag: &str) -> anyhow::Result<String>
where
    I: Iterator<Item = String>,
{
    args.next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("missing value for {flag}"))
}
