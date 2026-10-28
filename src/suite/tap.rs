//! Records the RPC conversation between a test suite and the validator.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::Value;

#[derive(Debug, Default, Clone)]
pub struct Capture {
    /// The execution error, when simulation got as far as executing.
    pub err: Option<Value>,
    /// The JSON-RPC error itself (code, message), for failures that carry no
    /// simulation result at all — a bad blockhash, a signature that did not verify.
    pub rpc_error: Option<Value>,
    /// `None` means the validator reported no logs; `Some(vec![])` means it
    /// reported an empty list.
    pub logs: Option<Vec<String>>,
    /// `None` means unmeasured, `Some(0)` means measured as zero.
    pub cu: Option<u64>,
    /// The fee the simulation reported, when it reported one.
    pub fee: Option<u64>,
    /// The serialized transaction from the request, still encoded.
    pub transaction: Option<(String, String)>,
}

pub struct Tap {
    port: u16,
    captures: Arc<Mutex<Vec<Capture>>>,
    stop: Arc<AtomicBool>,
}

/// Where the port scan starts. The pair is `P` (http) and `P+1` (websocket),
/// because web3.js derives the latter from the former.
const BASE: u16 = 18899;

impl Tap {
    pub fn start(upstream_http: &str, upstream_ws: &str) -> std::io::Result<Self> {
        let (http, ws, port) = bind_pair()?;
        let captures: Arc<Mutex<Vec<Capture>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));

        let upstream = upstream_http.to_string();
        let (recorded, stopped) = (Arc::clone(&captures), Arc::clone(&stop));
        std::thread::spawn(move || {
            for stream in http.incoming() {
                if stopped.load(Ordering::Relaxed) {
                    return;
                }
                let Ok(stream) = stream else { continue };
                let (upstream, recorded) = (upstream.clone(), Arc::clone(&recorded));
                std::thread::spawn(move || serve(stream, &upstream, &recorded));
            }
        });

        let target = upstream_ws.to_string();
        let stopped = Arc::clone(&stop);
        std::thread::spawn(move || {
            for stream in ws.incoming() {
                if stopped.load(Ordering::Relaxed) {
                    return;
                }
                let Ok(client) = stream else { continue };
                let Ok(server) = TcpStream::connect(&target) else {
                    continue;
                };
                relay(client, server);
            }
        });

        Ok(Tap { port, captures, stop })
    }

    /// The url to point the suite at.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Every failed call seen so far, oldest first.
    pub fn failures(&self) -> Vec<Capture> {
        self.captures.lock().expect("captures").clone()
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // `accept` is blocking, so knock on both doors to let the loops notice.
        for port in [self.port, self.port + 1] {
            let _ = TcpStream::connect(("127.0.0.1", port));
        }
    }
}

fn bind_pair() -> std::io::Result<(TcpListener, TcpListener, u16)> {
    let mut last = None;
    for port in BASE..BASE + 50 {
        let Ok(http) = TcpListener::bind(("127.0.0.1", port)) else {
            continue;
        };
        match TcpListener::bind(("127.0.0.1", port + 1)) {
            Ok(ws) => return Ok((http, ws, port)),
            Err(err) => last = Some(err), // keep `http` bound until we drop it here
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("no free port pair for the rpc tap")))
}

/// Byte-for-byte both ways. The websocket handshake and frames are none of our
/// business; we only need `confirmTransaction` to work.
fn relay(client: TcpStream, server: TcpStream) {
    for (mut from, mut to) in [
        (client.try_clone(), server.try_clone()),
        (server.try_clone(), client.try_clone()),
    ]
    .into_iter()
    .filter_map(|(from, to)| Some((from.ok()?, to.ok()?)))
    {
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut from, &mut to);
            // Close the other half too, or the peer waits forever.
            let _ = to.shutdown(std::net::Shutdown::Both);
        });
    }
}

/// Reads one request, forwards it, writes the reply back. One request per
/// connection, closed explicitly so a keep-alive client does not wait on a
/// socket we are finished with.
fn serve(stream: TcpStream, upstream: &str, recorded: &Mutex<Vec<Capture>>) {
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(stream) => stream,
        Err(_) => return,
    });
    let Some((method_line, length)) = read_head(&mut reader) else {
        return;
    };
    let mut body = vec![0u8; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }

    let (status, reply) = forward(upstream, &body);
    let mut stream = stream;
    let head = format!(
        "HTTP/1.1 {status} \r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        reply.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&reply);
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Write);

    // Forward first, record after: the suite is waiting on us.
    if method_line.starts_with("POST")
        && let Some(capture) = capture(&body, &reply)
    {
        recorded.lock().expect("captures").push(capture);
    }
}

/// Request line plus `Content-Length`. Other headers are the client's business.
fn read_head(reader: &mut impl BufRead) -> Option<(String, usize)> {
    let mut first = String::new();
    reader.read_line(&mut first).ok()?;
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            return Some((first.trim_end().to_string(), length));
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
}

fn forward(upstream: &str, body: &[u8]) -> (u16, Vec<u8>) {
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    match agent
        .post(upstream)
        .header("Content-Type", "application/json")
        .send(body)
    {
        Ok(mut response) => {
            let status = response.status().as_u16();
            let reply = response.body_mut().read_to_vec().unwrap_or_default();
            (status, reply)
        }
        Err(err) => (
            502,
            format!(
                r#"{{"jsonrpc":"2.0","id":0,"error":{{"code":-32603,"message":"rpc tap could not reach the validator: {err}"}}}}"#
            )
            .into_bytes(),
        ),
    }
}

/// Pulls the evidence out of one request/response pair. `None` for calls we do
/// not care about and for calls that did not fail.
pub fn capture(request: &[u8], response: &[u8]) -> Option<Capture> {
    let request: Value = serde_json::from_slice(request).ok()?;
    let response: Value = serde_json::from_slice(response).ok()?;
    let method = request["method"].as_str()?;
    if !matches!(method, "sendTransaction" | "simulateTransaction") {
        return None;
    }

    let mut capture = Capture {
        transaction: request["params"][0].as_str().map(|tx| {
            let encoding = request["params"][1]["encoding"]
                .as_str()
                .unwrap_or("base58") // the rpc's own default
                .to_string();
            (tx.to_string(), encoding)
        }),
        ..Capture::default()
    };

    // Two envelopes for the same evidence: `sendTransaction` reports a preflight
    // failure as a JSON-RPC error with the simulation result in `error.data`,
    // while `simulateTransaction` reports it as a successful call whose
    // `result.value` carries the error.
    let simulation = if let Some(error) = response.get("error") {
        capture.rpc_error = Some(error.clone());
        error.get("data").cloned()
    } else {
        response.pointer("/result/value").cloned()
    };

    if let Some(simulation) = simulation {
        capture.err = simulation.get("err").filter(|err| !err.is_null()).cloned();
        capture.logs = simulation.get("logs").and_then(Value::as_array).map(|logs| {
            logs.iter()
                .filter_map(|line| line.as_str().map(str::to_string))
                .collect()
        });
        capture.cu = simulation.get("unitsConsumed").and_then(Value::as_u64);
        capture.fee = simulation.get("fee").and_then(Value::as_u64);
    }

    (capture.err.is_some() || capture.rpc_error.is_some()).then_some(capture)
}

pub fn upstreams(http_url: &str) -> Option<(String, String)> {
    let authority = http_url.split("//").nth(1)?.trim_end_matches('/');
    let (host, port) = authority.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    Some((http_url.to_string(), format!("{host}:{}", port + 1)))
}

#[cfg(test)]
pub(crate) const SEND_REQUEST: &str = r#"{"method":"sendTransaction","jsonrpc":"2.0","params":["AfSy75UYfC5/xmFP1Hd8gO8Vu0geDdQzuj5WLI4qJYYsQEn0XwZTpgjEqd9ckVHTpNRba0/6GyXownQlfG/0yg8BAAEDLx4ltv4cNg/di2S86vuA4E3qJNuvihourXXY3U52DuPbag3ET21NbKxooWvwVPaDLUhHK/DSbdV2hZhptB5cOBgsSWdaqyP+U9/PEL8k34Jz6LX9gHInnuv62DpMHeaUMP8QR//7LU0+16rrsfg9LI/ADJFUcrUNV4NFjtU1XtgBAgIBABDyI8aJUuHytgAAAAAAAAAA",{"encoding":"base64","preflightCommitment":"processed"}],"id":"61e95c87-2bdd-4ade-8cb7-ad5e7c7cf229"}"#;
#[cfg(test)]
pub(crate) const SEND_ZERO_AMOUNT: &str = r#"{"jsonrpc":"2.0","error":{"code":-32002,"message":"Transaction simulation failed: Error processing Instruction 0: custom program error: 0x1770","data":{"accounts":null,"err":{"InstructionError":[0,{"Custom":6000}]},"fee":5000,"innerInstructions":null,"loadedAccountsDataSize":203593,"loadedAddresses":null,"logs":["Program 2dMxE4PyZtESZXUzKktvUUUd27WoYyhs1Pb6L3aPYGf5 invoke [1]","Program log: Instruction: Deposit","Program log: Instruction: Deposit","Program log: AnchorError thrown in programs/vault/src/lib.rs:20. Error Code: ZeroAmount. Error Number: 6000. Error Message: deposit amount must be greater than zero.","Program 2dMxE4PyZtESZXUzKktvUUUd27WoYyhs1Pb6L3aPYGf5 consumed 4340 of 200000 compute units","Program 2dMxE4PyZtESZXUzKktvUUUd27WoYyhs1Pb6L3aPYGf5 failed: custom program error: 0x1770"],"postBalances":null,"postTokenBalances":null,"preBalances":null,"preTokenBalances":null,"replacementBlockhash":null,"returnData":null,"unitsConsumed":4340}},"id":"61e95c87-2bdd-4ade-8cb7-ad5e7c7cf229"}"#;

// A direct simulateTransaction reports the same evidence one envelope out,
// under `result.value`, so the parser has to read both.
#[cfg(test)]
pub(crate) const SEND_INSUFFICIENT_FUNDS: &str = r#"{"jsonrpc":"2.0","error":{"code":-32002,"message":"Transaction simulation failed: Error processing Instruction 0: custom program error: 0x1772","data":{"accounts":null,"err":{"InstructionError":[0,{"Custom":6002}]},"fee":5000,"innerInstructions":null,"loadedAccountsDataSize":203593,"loadedAddresses":null,"logs":["Program 2dMxE4PyZtESZXUzKktvUUUd27WoYyhs1Pb6L3aPYGf5 invoke [1]","Program log: Instruction: Withdraw","Program log: Instruction: Withdraw","Program log: AnchorError thrown in programs/vault/src/lib.rs:32. Error Code: InsufficientFunds. Error Number: 6002. Error Message: vault balance is lower than the requested amount.","Program 2dMxE4PyZtESZXUzKktvUUUd27WoYyhs1Pb6L3aPYGf5 consumed 4367 of 200000 compute units","Program 2dMxE4PyZtESZXUzKktvUUUd27WoYyhs1Pb6L3aPYGf5 failed: custom program error: 0x1772"],"postBalances":null,"postTokenBalances":null,"preBalances":null,"preTokenBalances":null,"replacementBlockhash":null,"returnData":null,"unitsConsumed":4367}},"id":"f8a857b0-51e4-47cb-bc91-97ad289deb45"}"#;

#[cfg(test)]
pub(crate) const SEND_NOT_INITIALIZED: &str = r#"{"jsonrpc":"2.0","error":{"code":-32002,"message":"Transaction simulation failed: Error processing Instruction 0: custom program error: 0xbc4","data":{"accounts":null,"err":{"InstructionError":[0,{"Custom":3012}]},"fee":5000,"innerInstructions":null,"loadedAccountsDataSize":203481,"loadedAddresses":null,"logs":["Program 2dMxE4PyZtESZXUzKktvUUUd27WoYyhs1Pb6L3aPYGf5 invoke [1]","Program log: Instruction: Deposit","Program log: AnchorError caused by account: vault. Error Code: AccountNotInitialized. Error Number: 3012. Error Message: The program expected this account to be already initialized.","Program 2dMxE4PyZtESZXUzKktvUUUd27WoYyhs1Pb6L3aPYGf5 consumed 2471 of 200000 compute units","Program 2dMxE4PyZtESZXUzKktvUUUd27WoYyhs1Pb6L3aPYGf5 failed: custom program error: 0xbc4"],"postBalances":null,"postTokenBalances":null,"preBalances":null,"preTokenBalances":null,"replacementBlockhash":null,"returnData":null,"unitsConsumed":2471}},"id":"915c2c10-aca1-4525-aa53-3d11dc244d59"}"#;

#[cfg(test)]
pub(crate) const SIMULATE: &str = r#"{"jsonrpc":"2.0","result":{"context":{"slot":42},"value":{"accounts":null,"err":{"InstructionError":[0,{"Custom":6002}]},"logs":["Program Vau1t invoke [1]","Program log: Instruction: Withdraw","Program Vau1t failed: custom program error: 0x1772"],"unitsConsumed":4367,"fee":5000}},"id":1}"#;

// Hand-written, not captured: a signature that does not verify is rejected
// before anything executes, so there is no simulation result at all.
#[cfg(test)]
pub(crate) const SIG_VERIFY: &str =
    r#"{"jsonrpc":"2.0","error":{"code":-32003,"message":"Transaction signature verification failure"},"id":1}"#;

#[cfg(test)]
pub(crate) const BARE_REQUEST: &str = r#"{"jsonrpc":"2.0","id":1,"method":"sendTransaction","params":[]}"#;

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};

    #[test]
    fn both_rpc_envelopes_yield_the_same_evidence() {
        let send =
            super::capture(super::SEND_REQUEST.as_bytes(), super::SEND_ZERO_AMOUNT.as_bytes()).expect("captured");
        assert_eq!(send.err.as_ref().unwrap()["InstructionError"][1]["Custom"], 6000);
        assert_eq!(send.cu, Some(4340));
        assert_eq!(send.fee, Some(5000));
        assert_eq!(send.logs.as_deref().map(<[String]>::len), Some(6));
        assert!(send.rpc_error.is_some(), "the top-level error is kept too");
        assert_eq!(
            send.transaction.as_ref().map(|(_, encoding)| encoding.as_str()),
            Some("base64")
        );

        let simulated = super::capture(super::SEND_REQUEST.as_bytes(), super::SIMULATE.as_bytes()).expect("captured");
        assert_eq!(simulated.err.as_ref().unwrap()["InstructionError"][1]["Custom"], 6002);
        assert_eq!(simulated.cu, Some(4367));
        assert_eq!(simulated.logs.as_deref().map(<[String]>::len), Some(3));
        assert!(
            simulated.rpc_error.is_none(),
            "a simulate that reports an error is still a successful call"
        );
    }

    #[test]
    fn a_failure_before_execution_claims_no_logs_or_compute() {
        let capture = super::capture(super::BARE_REQUEST.as_bytes(), super::SIG_VERIFY.as_bytes()).expect("captured");
        assert!(capture.logs.is_none(), "absent, not empty");
        assert_eq!(capture.cu, None, "nothing ran, so nothing was measured");
        assert_eq!(capture.err, None);
        assert_eq!(capture.rpc_error.as_ref().unwrap()["code"], -32003);
        assert!(capture.transaction.is_none());

        // calls that did not fail, and methods we do not care about, are not kept
        let ok = r#"{"jsonrpc":"2.0","result":"3Zx1","id":1}"#;
        assert!(super::capture(super::BARE_REQUEST.as_bytes(), ok.as_bytes()).is_none());
        let other = r#"{"jsonrpc":"2.0","id":1,"method":"getBalance","params":[]}"#;
        assert!(super::capture(other.as_bytes(), super::SIG_VERIFY.as_bytes()).is_none());
    }

    #[test]
    fn the_tap_forwards_and_records() {
        // a stand-in validator that answers every post with one preflight failure
        let upstream = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = upstream.local_addr().expect("addr").port();
        std::thread::spawn(move || {
            for stream in upstream.incoming().take(1) {
                let Ok(mut stream) = stream else { continue };
                // Drain the request before replying: closing a socket with
                // unread bytes still in it resets the connection.
                let mut reader = std::io::BufReader::new(stream.try_clone().expect("clone"));
                let (_, length) = super::read_head(&mut reader).expect("head");
                let mut body = vec![0u8; length];
                let _ = reader.read_exact(&mut body);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    super::SEND_ZERO_AMOUNT.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(super::SEND_ZERO_AMOUNT.as_bytes());
                let _ = stream.flush();
            }
        });

        let tap = super::Tap::start(&format!("http://127.0.0.1:{port}"), &format!("127.0.0.1:{port}")).expect("tap");
        let mut response = ureq::post(tap.url())
            .header("Content-Type", "application/json")
            .send(super::SEND_REQUEST)
            .expect("forwarded");
        let status = response.status().as_u16();
        let text = response.body_mut().read_to_string().expect("body");
        let reply: serde_json::Value =
            serde_json::from_str(&text).unwrap_or_else(|err| panic!("status {status}, body {text:?}: {err}"));

        assert_eq!(
            reply["error"]["data"]["unitsConsumed"], 4340,
            "the reply reaches the caller unchanged"
        );
        let failures = tap.failures();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].cu, Some(4340));
    }
}
