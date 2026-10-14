// SPDX-License-Identifier: MPL-2.0

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::Value;
use url::Url;

pub struct ResponseSpec {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
    delay_before_headers: Duration,
    delay_before_body: Duration,
    before_headers: Option<Sender<()>>,
    before_body: Option<Sender<()>>,
}

impl ResponseSpec {
    pub fn json(status: u16, body: Value) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: serde_json::to_vec(&body).unwrap(),
            delay_before_headers: Duration::ZERO,
            delay_before_body: Duration::ZERO,
            before_headers: None,
            before_body: None,
        }
    }

    pub fn ndjson(lines: Vec<Value>) -> Self {
        let mut body = Vec::new();
        for line in lines {
            serde_json::to_writer(&mut body, &line).unwrap();
            body.push(b'\n');
        }
        Self {
            status: 200,
            content_type: "application/x-ndjson",
            body,
            delay_before_headers: Duration::ZERO,
            delay_before_body: Duration::ZERO,
            before_headers: None,
            before_body: None,
        }
    }

    pub fn sse(events: Vec<Value>) -> Self {
        let mut body = Vec::new();
        for event in events {
            body.extend_from_slice(b"data: ");
            serde_json::to_writer(&mut body, &event).unwrap();
            body.extend_from_slice(b"\n\n");
        }
        Self {
            status: 200,
            content_type: "text/event-stream",
            body,
            delay_before_headers: Duration::ZERO,
            delay_before_body: Duration::ZERO,
            before_headers: None,
            before_body: None,
        }
    }

    pub fn with_delay_before_headers(mut self, delay: Duration) -> Self {
        self.delay_before_headers = delay;
        self
    }

    pub fn with_delay_before_body(mut self, delay: Duration) -> Self {
        self.delay_before_body = delay;
        self
    }

    /// Signal only once the request has arrived and the header wait begins.
    pub fn notify_before_headers(mut self, notification: Sender<()>) -> Self {
        self.before_headers = Some(notification);
        self
    }

    /// Signal after headers are flushed, before the delayed response body.
    pub fn notify_before_body(mut self, notification: Sender<()>) -> Self {
        self.before_body = Some(notification);
        self
    }
}

#[derive(Debug)]
pub struct CapturedRequest {
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

pub struct TestServer {
    url: Url,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    join: Option<JoinHandle<()>>,
}

impl TestServer {
    pub fn start(responses: Vec<ResponseSpec>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let thread_requests = requests.clone();
        let expected = responses.len();
        let join = thread::spawn(move || {
            let mut responses = VecDeque::from(responses);
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while !responses.is_empty() && std::time::Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        let request = read_request(&mut stream);
                        thread_requests
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push(request);
                        write_response(&mut stream, responses.pop_front().unwrap());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("test server accept failed: {error}"),
                }
            }
            assert!(
                responses.is_empty(),
                "test server received fewer than {expected} requests"
            );
        });
        Self {
            url: Url::parse(&format!("http://{address}/")).unwrap(),
            requests,
            join: Some(join),
        }
    }

    pub fn url(&self) -> Url {
        self.url.clone()
    }

    pub fn finish(mut self) -> Vec<CapturedRequest> {
        self.join.take().unwrap().join().unwrap();
        let mut requests = self
            .requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        std::mem::take(&mut *requests)
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn read_request(stream: &mut TcpStream) -> CapturedRequest {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end = loop {
        let read = stream.read(&mut buffer).unwrap();
        assert!(read > 0, "connection ended before request headers");
        bytes.extend_from_slice(&buffer[..read]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers_text = std::str::from_utf8(&bytes[..header_end]).unwrap();
    let mut lines = headers_text.split("\r\n");
    let request_line = lines.next().unwrap();
    let path = request_line.split_whitespace().nth(1).unwrap().to_owned();
    let mut headers = HashMap::new();
    for line in lines.filter(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').unwrap();
        headers.insert(name.to_ascii_lowercase(), value.trim().to_owned());
    }
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    while bytes.len() - header_end < content_length {
        let read = stream.read(&mut buffer).unwrap();
        assert!(read > 0, "connection ended before request body");
        bytes.extend_from_slice(&buffer[..read]);
    }
    CapturedRequest {
        path,
        headers,
        body: bytes[header_end..header_end + content_length].to_vec(),
    }
}

fn write_response(stream: &mut TcpStream, response: ResponseSpec) {
    if let Some(notification) = response.before_headers {
        notification
            .send(())
            .expect("header wait observer disconnected");
    }
    thread::sleep(response.delay_before_headers);
    let reason = match response.status {
        200 => "OK",
        401 => "Unauthorized",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Response",
    };
    if write!(
        stream,
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.status,
        reason,
        response.content_type,
        response.body.len()
    )
    .is_err()
    {
        return;
    }
    if stream.flush().is_err() {
        return;
    }
    if let Some(notification) = response.before_body {
        notification
            .send(())
            .expect("body wait observer disconnected");
    }
    thread::sleep(response.delay_before_body);
    if stream.write_all(&response.body).is_ok() {
        let _ = stream.flush();
    }
}
