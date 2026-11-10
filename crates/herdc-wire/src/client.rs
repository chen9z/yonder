//! One framed connection to a core: JSON-RPC on channel 0, attach and event
//! streams on their own channels. Any byte pipe works (an SSH exec channel,
//! `hc connect`'s stdio), so the transport stays outside.

use crate::frame::{self, AUTOMATIC_INPUT, CLOSE, CREDIT, DATA, FACTS, FOCUS, RESIZE};
use crate::Error;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, watch};

/// How long a call waits for its reply unless it says otherwise.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(10);
/// What an attach stream starts with, per the core (schema/README.md).
pub const ATTACH_CREDIT: usize = 256 * 1024;
/// Extra credit for the event stream, as Herdr gives it.
pub const EVENT_CREDIT: usize = 1024 * 1024;
/// The least a paced stream credits back at once.
const PIECE: usize = 16 * 1024;
/// How long a paced stream's output may outrun it before it's behind.
const BEHIND: Duration = Duration::from_secs(1);

/// What arrives on a stream channel.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Data(Vec<u8>),
    /// The terminal's grid changed (observers that asked for notifications).
    Resize { cols: u16, rows: u16 },
    /// The core ended the stream; the payload says why (`{"reason":"exited",...}`).
    Closed(Value),
    /// The connection dropped. The program most likely still runs: reattach later.
    Lost,
}

#[derive(Default)]
struct Routes {
    closed: bool,
    replies: HashMap<u64, oneshot::Sender<Result<Value, Value>>>,
    streams: HashMap<u32, async_channel::Sender<Item>>,
    /// Frames for a channel whose reply hasn't been handled yet.
    early: HashMap<u32, Vec<Item>>,
    automatic_input: HashSet<u32>,
}

struct Shared {
    hello: Value,
    out: mpsc::UnboundedSender<Vec<u8>>,
    routes: Mutex<Routes>,
    next_id: AtomicU64,
    lost: watch::Receiver<bool>,
}

/// A connection to one core. Cheap to clone; the connection ends when the
/// pipe does (or every clone and stream is dropped).
#[derive(Clone)]
pub struct Client {
    shared: Arc<Shared>,
}

impl Client {
    /// Says hello on a fresh pipe and switches it to framed mode. Must run
    /// inside a tokio runtime: the reader and writer are tasks.
    pub async fn handshake<R, W>(reader: R, mut writer: W) -> Result<Client, Error>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let mut reader = BufReader::new(reader);
        writer.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"hello\",\"params\":{\"framed\":true}}\n").await?;
        writer.flush().await?;
        let mut line = Vec::new();
        reader.read_until(b'\n', &mut line).await?;
        if line.is_empty() {
            return Err(Error::Lost);
        }
        let reply: Value = serde_json::from_slice(&line)
            .map_err(|_| Error::Protocol(format!("not a core: {}", String::from_utf8_lossy(&line).trim())))?;
        if let Some(error) = reply.get("error") {
            return Err(Error::rpc("hello", error));
        }
        let hello = reply.get("result").cloned().unwrap_or(Value::Null);
        if hello["framed"] != true {
            return Err(Error::Protocol("the core did not switch to framed mode".into()));
        }

        let (out, mut frames) = mpsc::unbounded_channel::<Vec<u8>>();
        let (lost_tx, lost) = watch::channel(false);
        let shared = Arc::new(Shared { hello, out, routes: Mutex::default(), next_id: AtomicU64::new(1), lost });

        tokio::spawn(async move {
            while let Some(bytes) = frames.recv().await {
                if writer.write_all(&bytes).await.is_err() || writer.flush().await.is_err() {
                    break;
                }
            }
            let _ = writer.shutdown().await;
        });

        let routes = Arc::downgrade(&shared);
        tokio::spawn(async move {
            loop {
                let frame = match frame::read(&mut reader).await {
                    Ok(Some(f)) => f,
                    Ok(None) => break,
                    Err(e) => {
                        tracing::warn!("core connection: {e}");
                        break;
                    }
                };
                let Some(shared) = routes.upgrade() else { break };
                shared.dispatch(frame);
            }
            if let Some(shared) = routes.upgrade() {
                shared.lose();
            }
            let _ = lost_tx.send(true);
        });

        Ok(Client { shared })
    }

    /// The core's hello: `generation`, `version`, `core`, `methods`, `features`.
    pub fn hello(&self) -> &Value {
        &self.shared.hello
    }

    pub fn has_feature(&self, feature: &str) -> bool {
        self.shared.hello["features"].as_array().is_some_and(|f| f.iter().any(|x| x == feature))
    }

    pub fn is_closed(&self) -> bool {
        *self.shared.lost.borrow()
    }

    /// Resolves once the connection is gone.
    pub async fn closed(&self) {
        let mut lost = self.shared.lost.clone();
        let _ = lost.wait_for(|l| *l).await;
    }

    pub async fn call(&self, method: &str, params: Value) -> Result<Value, Error> {
        self.call_timeout(method, params, CALL_TIMEOUT).await
    }

    pub async fn call_timeout(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, Error> {
        let id = self.shared.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let mut routes = self.shared.routes.lock().unwrap();
            if routes.closed {
                return Err(Error::Lost);
            }
            routes.replies.insert(id, tx);
        }
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.send(0, DATA, request.to_string().as_bytes());
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => Err(Error::rpc(method, &e)),
            Ok(Err(_)) => Err(Error::Lost),
            Err(_) => {
                self.shared.routes.lock().unwrap().replies.remove(&id);
                Err(Error::Timeout(method.to_owned()))
            }
        }
    }

    /// Opens a stream to a live terminal: its first paint, then its output.
    pub async fn attach(&self, request: Attach) -> Result<Attached, Error> {
        let mut params = json!({
            "terminal": request.terminal,
            "cols": request.cols,
            "rows": request.rows,
            "history": request.history,
            "observe": request.observe,
            "resize_notifications": request.resize_notifications,
            "resumed": request.resumed,
        });
        if !request.display.is_null() {
            params["display"] = request.display;
        }
        let result = self.call("terminal.attach", params).await?;
        let channel = channel_of(&result)?;
        let automatic = self.has_feature("automatic_input") && result["automatic_input"] == true;
        let mut stream = self.open_stream(channel, automatic);
        stream.pace = request.pace.map(|rate| Mutex::new(Pace::new(rate)));
        Ok(Attached { stream, terminal: result["terminal"].clone(), resize_notifications: result["resize_notifications"] == true })
    }

    /// Subscribes to terminal facts: the snapshot (`seq`, `terminals`) and
    /// the stream of JSON lines to apply to it (see [`crate::Terminals`]).
    pub async fn subscribe(&self, params: Value) -> Result<(Stream, Value), Error> {
        let result = self.call("events.subscribe", params).await?;
        let channel = channel_of(&result)?;
        let stream = self.open_stream(channel, false);
        self.send(channel, CREDIT, &(EVENT_CREDIT as u32).to_le_bytes());
        Ok((stream, result))
    }

    fn open_stream(&self, channel: u32, automatic_input: bool) -> Stream {
        let (tx, rx) = async_channel::unbounded();
        let mut routes = self.shared.routes.lock().unwrap();
        for item in routes.early.remove(&channel).unwrap_or_default() {
            let _ = tx.try_send(item);
        }
        if routes.closed {
            let _ = tx.try_send(Item::Lost);
        } else {
            routes.streams.insert(channel, tx);
        }
        if automatic_input {
            routes.automatic_input.insert(channel);
        }
        Stream { client: self.clone(), channel, rx, ended: AtomicBool::new(false), pace: None }
    }

    fn send(&self, channel: u32, kind: u8, payload: &[u8]) {
        let _ = self.shared.out.send(frame::encode(channel, kind, payload));
    }
}

fn channel_of(result: &Value) -> Result<u32, Error> {
    result["channel"].as_u64().map(|c| c as u32).ok_or_else(|| Error::Protocol(format!("no channel in {result}")))
}

impl Shared {
    fn dispatch(&self, frame: frame::Frame) {
        let mut routes = self.routes.lock().unwrap();
        if frame.channel == 0 {
            let Ok(message) = serde_json::from_slice::<Value>(&frame.payload) else { return };
            let Some(id) = message.get("id").and_then(Value::as_u64) else { return };
            if let Some(tx) = routes.replies.remove(&id) {
                let _ = tx.send(match message.get("error") {
                    Some(e) => Err(e.clone()),
                    None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                });
            }
            return;
        }
        let item = match frame.kind {
            DATA => Item::Data(frame.payload),
            CLOSE => Item::Closed(serde_json::from_slice(&frame.payload).unwrap_or(Value::Null)),
            RESIZE => match frame::parse_size(&frame.payload) {
                Some((cols, rows)) => Item::Resize { cols, rows },
                None => return,
            },
            _ => return,
        };
        let closing = matches!(item, Item::Closed(_));
        match routes.streams.get(&frame.channel) {
            Some(tx) => {
                let _ = tx.try_send(item);
            }
            None => routes.early.entry(frame.channel).or_default().push(item),
        }
        if closing {
            routes.streams.remove(&frame.channel);
            routes.automatic_input.remove(&frame.channel);
        }
    }

    /// The connection is gone: tell every stream, fail every waiting call.
    fn lose(&self) {
        let mut routes = self.routes.lock().unwrap();
        routes.closed = true;
        routes.automatic_input.clear();
        for (_, tx) in routes.streams.drain() {
            let _ = tx.try_send(Item::Lost);
        }
        routes.replies.clear();
    }
}

/// What `terminal.attach` takes.
#[derive(Clone, Debug)]
pub struct Attach {
    pub terminal: String,
    pub cols: u16,
    pub rows: u16,
    /// Scrollback lines to paint first.
    pub history: u32,
    /// Watch only: never type or resize.
    pub observe: bool,
    pub resize_notifications: bool,
    /// Back after a dropped line: take the size only once active again.
    pub resumed: bool,
    /// The viewer's colors and cell size (`Display` in the schema).
    pub display: Value,
    /// Bytes a second credited back at most (None: all, as they're read).
    /// Output faster than that the core drops, sending the screen instead:
    /// a phone shows what's there now without taking every byte of a flood.
    pub pace: Option<usize>,
}

impl Attach {
    pub fn new(terminal: impl Into<String>, cols: u16, rows: u16) -> Self {
        Attach {
            terminal: terminal.into(),
            cols,
            rows,
            history: 2000,
            observe: false,
            resize_notifications: false,
            resumed: false,
            display: Value::Null,
            pace: None,
        }
    }
}

pub struct Attached {
    pub stream: Stream,
    /// The terminal's record when it attached.
    pub terminal: Value,
    pub resize_notifications: bool,
}

/// One channel of a connection. Reading data credits it back to the core
/// (at its pace, if it has one); dropping the stream detaches it.
pub struct Stream {
    client: Client,
    channel: u32,
    rx: async_channel::Receiver<Item>,
    ended: AtomicBool,
    pace: Option<Mutex<Pace>>,
}

/// Credit owed to the core, paid back no faster than `rate` bytes a second.
#[derive(Debug)]
struct Pace {
    rate: f64,
    owed: usize,
    /// What may be paid now: it grows with time, up to a window (none at
    /// first: the core starts with one).
    allowed: f64,
    at: tokio::time::Instant,
    /// Since when the core has had (almost) no credit left: output comes
    /// faster than the pace.
    starved: Option<tokio::time::Instant>,
}

impl Pace {
    fn new(rate: usize) -> Self {
        Pace { rate: rate.max(1) as f64, owed: 0, allowed: 0.0, at: tokio::time::Instant::now(), starved: None }
    }

    /// Owes `read` more; what may be paid now: in pieces of 16 KiB at
    /// least (or all that's owed), so the core sends no crumbs.
    fn pay(&mut self, read: usize) -> usize {
        let now = tokio::time::Instant::now();
        self.allowed = (self.allowed + now.duration_since(self.at).as_secs_f64() * self.rate).min(ATTACH_CREDIT as f64);
        self.at = now;
        self.owed += read;
        if self.owed >= ATTACH_CREDIT / 2 {
            self.starved.get_or_insert(now);
        } else if self.owed < ATTACH_CREDIT / 4 {
            self.starved = None;
        }
        if self.allowed < self.piece() as f64 {
            return 0;
        }
        let paid = self.owed.min(self.allowed as usize);
        self.owed -= paid;
        self.allowed -= paid as f64;
        paid
    }

    fn piece(&self) -> usize {
        self.owed.min(PIECE)
    }

    /// How long until what's owed may be paid (None: nothing owed).
    fn due(&self) -> Option<Duration> {
        (self.owed > 0).then(|| Duration::from_secs_f64(((self.piece() as f64 - self.allowed) / self.rate).max(0.0)))
    }
}

impl Stream {
    pub fn channel(&self) -> u32 {
        self.channel
    }

    /// The next item; None once it closed or the connection was lost.
    pub async fn next(&self) -> Option<Item> {
        if self.ended.load(Ordering::Relaxed) {
            return None;
        }
        let item = loop {
            // Paced, owing: the credit is paid when due, whether or not more comes.
            let due = self.pace.as_ref().and_then(|p| p.lock().unwrap().due());
            match due {
                None => break self.rx.recv().await.unwrap_or(Item::Lost),
                Some(due) => tokio::select! {
                    item = self.rx.recv() => break item.unwrap_or(Item::Lost),
                    _ = tokio::time::sleep(due) => self.credit(0),
                },
            }
        };
        match &item {
            Item::Data(bytes) => self.credit(bytes.len()),
            Item::Closed(_) | Item::Lost => self.ended.store(true, Ordering::Relaxed),
            Item::Resize { .. } => {}
        }
        Some(item)
    }

    /// What has arrived so far, without waiting.
    pub fn try_next(&self) -> Option<Item> {
        if self.ended.load(Ordering::Relaxed) {
            return None;
        }
        let item = self.rx.try_recv().ok()?;
        match &item {
            Item::Data(bytes) => self.credit(bytes.len()),
            Item::Closed(_) | Item::Lost => self.ended.store(true, Ordering::Relaxed),
            Item::Resize { .. } => {}
        }
        Some(item)
    }

    /// Paced, and the output has outrun the pace for over a second: what
    /// the core holds for it is behind the screen (up to 4 MiB of it). A
    /// stream attached again starts at the screen as it is.
    pub fn behind(&self) -> bool {
        self.pace.as_ref().is_some_and(|p| p.lock().unwrap().starved.is_some_and(|t| t.elapsed() > BEHIND))
    }

    /// Keys a person typed.
    pub fn input(&self, bytes: &[u8]) {
        if !bytes.is_empty() {
            self.client.send(self.channel, DATA, bytes);
        }
    }

    /// Replies the terminal engine made (query answers): not typing.
    pub fn automatic_input(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let tagged = self.client.shared.routes.lock().unwrap().automatic_input.contains(&self.channel);
        self.client.send(self.channel, if tagged { AUTOMATIC_INPUT } else { DATA }, bytes);
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        self.client.send(self.channel, RESIZE, &frame::size_payload(cols, rows));
    }

    pub fn focus(&self, focused: bool) {
        self.client.send(self.channel, FOCUS, &[focused as u8]);
    }

    pub fn facts(&self, display: &Value) {
        self.client.send(self.channel, FACTS, display.to_string().as_bytes());
    }

    fn credit(&self, read: usize) {
        let n = match &self.pace {
            Some(pace) => pace.lock().unwrap().pay(read),
            None => read,
        };
        if n > 0 {
            self.client.send(self.channel, CREDIT, &(n as u32).to_le_bytes());
        }
    }

    /// Detaches: the program keeps running.
    pub fn close(self) {}
}

impl Drop for Stream {
    fn drop(&mut self) {
        let mut routes = self.client.shared.routes.lock().unwrap();
        routes.streams.remove(&self.channel);
        routes.automatic_input.remove(&self.channel);
        let open = !routes.closed && !self.ended.load(Ordering::Relaxed);
        drop(routes);
        if open {
            self.client.send(self.channel, CLOSE, &[]);
        }
    }
}
