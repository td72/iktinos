//! `iktinos serve`: a server on this machine that shows the Claude Code
//! sessions in a browser on the same machine.
//!
//! A library with two ways in: [`run`], which the `iktinos` command calls for
//! its `serve` subcommand, and [`Server`], which a test starts.
//!
//! It is one more consumer of the model: it follows the transcripts
//! (`iktinos-core`) and, unless told not to, listens for live events on a
//! socket of its own (`iktinos-live`). What it adds is the way out, and that
//! is kept narrow:
//!
//! - **Local only.** It listens on `127.0.0.1` and nowhere else; no option
//!   makes it listen elsewhere. A request whose `Host` is not this server's
//!   own address is refused, so a page elsewhere cannot reach it by a name
//!   that resolves here.
//! - **A token.** The URL printed at start carries one. The first visit
//!   exchanges it for a cookie (`HttpOnly`, `SameSite=Strict`) and is
//!   redirected to the same address without it, so the token stays out of
//!   the address bar and the history. Without the cookie nothing is
//!   answered.
//! - **Read-only.** `GET` and nothing else. What reads a file again (a tool
//!   call's whole output) takes a session id and a call id: the path is the
//!   server's to find.
//! - **Modelled shapes only.** The JSON is built from the model ([`json`]),
//!   never from a raw record.
//! - **No async runtime.** A thread per connection, its own HTTP/1.1
//!   ([`http`]), and server-sent events for "something changed".
//!
//! It reads the part of the terminal viewer's configuration file that
//! means something to a server ([`config`]), and only reads it.

pub mod config;
pub mod http;
pub mod json;

use std::io::{self, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use iktinos_core::discover::default_projects_dir;
use iktinos_core::follow::Follower;
use iktinos_core::output::{read_response, read_tool_output};
use iktinos_core::place::Place;
use iktinos_core::record::Timestamp;
use iktinos_core::session::{Session, SessionStore};

use http::{Malformed, Request, Response};

/// The port unless `--port` names another.
pub const DEFAULT_PORT: u16 = 4747;

/// The cookie the token is exchanged for.
const COOKIE: &str = "iktinos";

/// How long a client may take to send its request.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// The most connections served at once. A browser holds a few; beyond this
/// something else is going on.
const MAX_CONNECTIONS: usize = 64;

/// How often a quiet event stream says it is still there, which is also how
/// a stream whose browser went away is noticed.
const PING_EVERY: Duration = Duration::from_secs(15);

// `ASSETS`: the page, embedded by the build script. What `web/` built if
// it was built, else the placeholder under `assets/`.
include!(concat!(env!("OUT_DIR"), "/assets.rs"));

/// The file of the page served at `address`, with its type. The addresses
/// are a fixed list made when the binary was built: none is looked up on
/// disk.
fn asset(address: &str) -> Option<(&'static str, &'static [u8])> {
    let address = match address {
        "/" => "/index.html",
        other => other,
    };
    ASSETS
        .iter()
        .find(|(served, _, _)| *served == address)
        .map(|(_, content_type, bytes)| (*content_type, *bytes))
}

const USAGE: &str = "\
Show the Claude Code sessions on this machine in a browser on this machine.

Usage: iktinos serve [OPTIONS]

It listens on 127.0.0.1 only and prints the URL to open. The URL carries a
token: without it, or the cookie the first visit gets for it, nothing is
answered.

Options:
      --port <PORT>         Listen on PORT instead of 4747; 0 picks a free one
      --config <FILE>       Read the configuration from FILE instead of
                            ~/.config/iktinos/config.toml; an argument goes
                            before the file
      --projects-dir <DIR>  Read transcripts from DIR instead of ~/.claude/projects
      --socket-dir <DIR>    Listen for live events in DIR instead of the shared default
      --no-live             Do not listen for live events from the bridge mod
      --cwd                 Show only the sessions that run in the current directory,
                            below it, or in a worktree of its git repository
  -h, --help                Print help
";

/// What the server was asked to show, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub projects_dir: PathBuf,
    /// 0 picks a free port.
    pub port: u16,
    /// Listen for live events.
    pub live: bool,
    /// Where to listen for them; the shared default when `None`.
    pub socket_dir: Option<PathBuf>,
    /// Show only the sessions of this directory (`--cwd`).
    pub within: Option<PathBuf>,
    /// How often the transcripts are read again.
    pub refresh: Duration,
    /// How recent a record makes a session active when no live event says
    /// more.
    pub active_within: Duration,
    /// How many of its latest tool calls each session keeps.
    pub recent_tools: usize,
    /// How many characters of a tool call's output a session keeps as its
    /// excerpt; 0 keeps none.
    pub output_chars: usize,
}

impl Config {
    /// Every setting at its default, for the transcripts in `projects_dir`.
    pub fn new(projects_dir: PathBuf) -> Self {
        Self {
            projects_dir,
            port: DEFAULT_PORT,
            live: true,
            socket_dir: None,
            within: None,
            refresh: Duration::from_secs(1),
            active_within: Duration::from_secs(60),
            recent_tools: 200,
            output_chars: 2000,
        }
    }
}

/// The sessions, kept current by the watcher thread and read by the
/// connections.
struct World {
    store: SessionStore,
    follower: Follower,
    place: Option<Place>,
    /// Why the transcripts could not be listed, if they could not.
    notice: Option<String>,
}

impl World {
    /// The session, unless the server is narrowed to a directory it does
    /// not run in.
    fn session(&mut self, id: &str) -> Option<&Session> {
        let session = self.store.get(id)?;
        match &mut self.place {
            Some(place) => session
                .location()
                .is_some_and(|path| place.holds(path))
                .then_some(session),
            None => Some(session),
        }
    }
}

struct Shared {
    world: RwLock<World>,
    /// Raised each time the sessions changed; the event streams wait on it.
    version: Mutex<u64>,
    changed: Condvar,
    stopping: AtomicBool,
    connections: AtomicUsize,
    token: String,
    /// `127.0.0.1:<port>` and `localhost:<port>`: the only hosts answered.
    hosts: [String; 2],
    active_within: Duration,
}

/// A running server. Dropping it stops it.
pub struct Server {
    addr: SocketAddr,
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
    /// Kept so that the socket is removed when the server stops.
    #[cfg(unix)]
    _live: Option<iktinos_live::server::Server>,
}

impl Server {
    /// Binds `127.0.0.1:<port>`, reads the transcripts once, and starts
    /// answering.
    pub fn start(config: Config) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, config.port))?;
        let addr = listener.local_addr()?;
        let mut world = World {
            store: SessionStore::new()
                .with_recent_tools(config.recent_tools)
                .with_tool_output(config.output_chars),
            follower: Follower::new(&config.projects_dir),
            place: config.within.as_deref().map(Place::new),
            notice: None,
        };
        world.notice = world
            .follower
            .poll(&mut world.store)
            .err()
            .map(|e| e.to_string());
        let shared = Arc::new(Shared {
            world: RwLock::new(world),
            version: Mutex::new(0),
            changed: Condvar::new(),
            stopping: AtomicBool::new(false),
            connections: AtomicUsize::new(0),
            token: new_token()?,
            hosts: [
                format!("127.0.0.1:{}", addr.port()),
                format!("localhost:{}", addr.port()),
            ],
            active_within: config.active_within,
        });

        #[cfg(unix)]
        let (live, events) = match config.live {
            true => {
                let started = match &config.socket_dir {
                    Some(dir) => iktinos_live::server::Server::start_in(dir),
                    None => iktinos_live::server::Server::start(),
                };
                // Without the socket the transcripts are still followed.
                match started {
                    Ok((server, events)) => (Some(server), Some(events)),
                    Err(_) => (None, None),
                }
            }
            false => (None, None),
        };

        let watcher = thread::Builder::new()
            .name("iktinos-web-watch".to_owned())
            .spawn({
                let shared = Arc::clone(&shared);
                let refresh = config.refresh;
                move || loop {
                    thread::sleep(refresh);
                    if shared.stopping.load(Ordering::SeqCst) {
                        return;
                    }
                    let mut changed = false;
                    {
                        let mut world = shared.world.write().unwrap_or_else(|e| e.into_inner());
                        let world = &mut *world;
                        match world.follower.poll(&mut world.store) {
                            Ok(followed) => {
                                changed |= followed.records > 0 || world.notice.take().is_some();
                            }
                            Err(e) => {
                                let notice = Some(e.to_string());
                                changed |= world.notice != notice;
                                world.notice = notice;
                            }
                        }
                        #[cfg(unix)]
                        if let Some(events) = &events {
                            for event in events.try_iter() {
                                iktinos_live::merge::apply_event(&mut world.store, &event);
                                changed = true;
                            }
                        }
                    }
                    if changed {
                        *shared.version.lock().unwrap_or_else(|e| e.into_inner()) += 1;
                        shared.changed.notify_all();
                    }
                }
            })?;

        let acceptor = thread::Builder::new()
            .name("iktinos-web-accept".to_owned())
            .spawn({
                let shared = Arc::clone(&shared);
                move || {
                    for stream in listener.incoming() {
                        if shared.stopping.load(Ordering::SeqCst) {
                            return;
                        }
                        let Ok(stream) = stream else {
                            continue;
                        };
                        let shared = Arc::clone(&shared);
                        // A connection that cannot get a thread is dropped.
                        let _ = thread::Builder::new()
                            .name("iktinos-web-conn".to_owned())
                            .spawn(move || connection(stream, &shared));
                    }
                }
            })?;

        Ok(Self {
            addr,
            shared,
            threads: vec![watcher, acceptor],
            #[cfg(unix)]
            _live: live,
        })
    }

    /// The address it listens on: `127.0.0.1` and the port in effect.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The token a first visit must carry.
    pub fn token(&self) -> &str {
        &self.shared.token
    }

    /// The URL to open: the address, with the token.
    pub fn url(&self) -> String {
        format!("http://{}/?token={}", self.addr, self.shared.token)
    }

    /// Why the transcripts could not be listed, if they could not.
    pub fn notice(&self) -> Option<String> {
        let world = self.shared.world.read().unwrap_or_else(|e| e.into_inner());
        world.notice.clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shared.stopping.store(true, Ordering::SeqCst);
        // Wake the acceptor out of `accept`, and the event streams.
        let _ = TcpStream::connect(self.addr);
        self.shared.changed.notify_all();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// 32 random bytes, in hex.
fn new_token() -> io::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Whether `given` is the token, in time that does not depend on where they
/// first differ.
fn is_token(token: &str, given: &str) -> bool {
    let (token, given) = (token.as_bytes(), given.as_bytes());
    token.len() == given.len()
        && token
            .iter()
            .zip(given)
            .fold(0u8, |differs, (a, b)| differs | (a ^ b))
            == 0
}

fn now() -> Timestamp {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    Timestamp::from_millis(millis)
}

/// Serves one connection: one request, one response.
fn connection(mut stream: TcpStream, shared: &Shared) {
    struct Counted<'a>(&'a AtomicUsize);
    impl Drop for Counted<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    let busy = shared.connections.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS;
    let _counted = Counted(&shared.connections);

    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(READ_TIMEOUT));
    let request = match http::read_request(&mut stream) {
        Ok(request) => request,
        // Nothing was sent: the wake-up of a stopping server, or a probe.
        Err(Malformed::Empty) => return,
        Err(Malformed::TooLarge) => {
            let _ = Response::text(431, "the request's headers are too long").write(&mut stream);
            return;
        }
        Err(Malformed::Bad) => {
            let _ = Response::text(400, "not a request this server reads").write(&mut stream);
            return;
        }
    };
    if busy {
        let _ = Response::text(503, "too many connections").write(&mut stream);
        return;
    }
    match route(&request, shared) {
        Routed::Respond(response) => {
            let _ = response.write(&mut stream);
        }
        Routed::Events => events(&mut stream, shared),
    }
}

enum Routed {
    Respond(Response),
    /// The stream of "something changed".
    Events,
}

/// Decides what a request gets. Who may ask comes first, then what for.
fn route(request: &Request, shared: &Shared) -> Routed {
    let respond = Routed::Respond;
    // A page elsewhere can make a browser ask this server by another name
    // that resolves to this machine: only its own address is answered.
    let host = request.header("host").unwrap_or_default();
    if !shared.hosts.iter().any(|own| own == host) {
        return respond(Response::text(
            403,
            "this server answers to its own address only",
        ));
    }
    if request.method != "GET" {
        return respond(Response::text(405, "this server only reads: GET").with("Allow", "GET"));
    }
    // The first visit: the token becomes a cookie, and leaves the address.
    if let Some(given) = request.param("token") {
        if !is_token(&shared.token, given) {
            return respond(Response::text(401, "that is not this server's token"));
        }
        return respond(
            Response::new(303, "text/plain; charset=utf-8", Vec::new())
                .with("Location", request.path.clone())
                .with(
                    "Set-Cookie",
                    format!(
                        "{COOKIE}={}; Path=/; HttpOnly; SameSite=Strict",
                        shared.token
                    ),
                ),
        );
    }
    if !request
        .cookie(COOKIE)
        .is_some_and(|given| is_token(&shared.token, given))
    {
        return respond(Response::text(
            401,
            "open the URL `iktinos serve` printed: it carries the token",
        ));
    }

    let segments: Vec<&str> = request.path.split('/').skip(1).collect();
    // Ids are plain names: nothing in a path is ever part of a file's path.
    let plain = |segment: &str| {
        !segment.is_empty()
            && segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    };
    match segments[..] {
        ["api", "events"] => Routed::Events,
        ["api", "sessions"] => respond(sessions(shared)),
        ["api", "usage"] => respond(usage(shared)),
        ["api", "sessions", id] if plain(id) => respond(session(shared, id, View::Detail)),
        ["api", "sessions", id, "trace"] if plain(id) => respond(session(shared, id, View::Trace)),
        ["api", "sessions", id, "calls", call, "output"] if plain(id) && plain(call) => {
            respond(whole(shared, id, Whole::Output(call)))
        }
        ["api", "sessions", id, "responses", message] if plain(id) && plain(message) => {
            respond(whole(shared, id, Whole::Response(message)))
        }
        _ => respond(match asset(&request.path) {
            Some((content_type, bytes)) => Response::new(200, content_type, bytes),
            None => Response::text(404, "nothing is served here"),
        }),
    }
}

fn to_json(value: &impl serde::Serialize) -> Response {
    match serde_json::to_vec(value) {
        Ok(body) => Response::json(body),
        Err(_) => Response::text(500, "the answer could not be written"),
    }
}

/// The list of sessions, the latest first, with why the transcripts could
/// not be listed if they could not.
fn sessions(shared: &Shared) -> Response {
    #[derive(serde::Serialize)]
    struct Sessions {
        sessions: Vec<json::SessionSummary>,
        notice: Option<String>,
    }
    let now = now();
    let mut world = shared.world.write().unwrap_or_else(|e| e.into_inner());
    let world = &mut *world;
    let place = &mut world.place;
    let mut listed: Vec<&Session> = world
        .store
        .sessions()
        .filter(|session| match place {
            Some(place) => session.location().is_some_and(|path| place.holds(path)),
            None => true,
        })
        .collect();
    listed.sort_by(|a, b| {
        b.last_seen()
            .cmp(&a.last_seen())
            .then_with(|| a.id.cmp(&b.id))
    });
    to_json(&Sessions {
        sessions: listed
            .into_iter()
            .map(|session| json::summary(session, now, shared.active_within))
            .collect(),
        notice: world.notice.clone(),
    })
}

/// What the sessions together have used. Every session is in it, even on
/// a server narrowed to a directory, as in the terminal viewer: the totals
/// name repositories, never a session.
fn usage(shared: &Shared) -> Response {
    let world = shared.world.read().unwrap_or_else(|e| e.into_inner());
    to_json(&json::totals(world.store.sessions()))
}

enum View {
    Detail,
    Trace,
}

fn session(shared: &Shared, id: &str, view: View) -> Response {
    let now = now();
    let mut world = shared.world.write().unwrap_or_else(|e| e.into_inner());
    let Some(session) = world.session(id) else {
        return Response::text(404, "no such session");
    };
    match view {
        View::Detail => to_json(&json::detail(session, now, shared.active_within)),
        View::Trace => to_json(&json::trace(session, now, shared.active_within)),
    }
}

enum Whole<'a> {
    /// A tool call's whole output, by the call's id.
    Output(&'a str),
    /// A response's whole text, by its `message.id`.
    Response(&'a str),
}

/// Reads something of a session again from its files. The files are those
/// the follower found for the session: the request names ids, never a path.
fn whole(shared: &Shared, id: &str, what: Whole) -> Response {
    let files = {
        let mut world = shared.world.write().unwrap_or_else(|e| e.into_inner());
        if world.session(id).is_none() {
            return Response::text(404, "no such session");
        }
        world.follower.files(id).cloned()
    };
    let Some(files) = files else {
        return Response::text(404, "no such session");
    };
    // The lock is not held while the file is read.
    let read = match what {
        Whole::Output(call) => read_tool_output(&files, call).map(|o| o.map(json::whole_output)),
        Whole::Response(message) => {
            read_response(&files, message).map(|r| r.map(json::whole_response))
        }
    };
    match read {
        Ok(Some(whole)) => to_json(&whole),
        Ok(None) => Response::text(404, "the session's files hold nothing of that"),
        Err(_) => Response::text(500, "the session's files could not be read"),
    }
}

/// The event stream: `changed` with the version, once at the start and then
/// whenever the sessions changed. The page reads again what it shows.
fn events(stream: &mut TcpStream, shared: &Shared) {
    let mut head = http::head(200, "text/event-stream");
    head.push_str("\r\n");
    if stream.write_all(head.as_bytes()).is_err() {
        return;
    }
    // A stream lives as long as its page. A write that does not get
    // through in time ends it: the browser is gone, or stuck.
    let _ = stream.set_write_timeout(Some(READ_TIMEOUT));
    let mut seen = None;
    loop {
        let version = {
            let guard = shared.version.lock().unwrap_or_else(|e| e.into_inner());
            let (guard, _) = shared
                .changed
                .wait_timeout_while(guard, PING_EVERY, |version| {
                    seen == Some(*version) && !shared.stopping.load(Ordering::SeqCst)
                })
                .unwrap_or_else(|e| e.into_inner());
            *guard
        };
        if shared.stopping.load(Ordering::SeqCst) {
            return;
        }
        let message = if seen == Some(version) {
            // Nothing changed: a comment, which the browser ignores.
            ": still here\n\n".to_owned()
        } else {
            seen = Some(version);
            format!("event: changed\ndata: {version}\n\n")
        };
        if stream.write_all(message.as_bytes()).is_err() || stream.flush().is_err() {
            return;
        }
    }
}

/// What `iktinos serve` was asked on its command line.
#[derive(Debug, PartialEq, Eq)]
enum Parsed {
    Run(config::Args),
    Help,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Parsed, String> {
    let mut asked = config::Args::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => {
                (flag.to_owned(), Some(value.to_owned()))
            }
            _ => (arg, None),
        };
        let mut value = |name: &str, what: &str| {
            inline
                .clone()
                .or_else(|| args.next())
                .filter(|v| !v.is_empty())
                .ok_or_else(|| format!("{name} needs a {what}"))
        };
        match flag.as_str() {
            "--port" => {
                let given = value("--port", "port")?;
                asked.port =
                    Some(given.parse().map_err(|_| {
                        format!("--port is a number from 0 to 65535, not '{given}'")
                    })?);
            }
            "--config" => asked.config = Some(PathBuf::from(value("--config", "file")?)),
            "--projects-dir" => {
                asked.projects_dir = Some(PathBuf::from(value("--projects-dir", "directory")?));
            }
            "--socket-dir" => {
                asked.socket_dir = Some(PathBuf::from(value("--socket-dir", "directory")?));
            }
            "--no-live" => asked.no_live = true,
            "--cwd" if inline.is_none() => asked.cwd = true,
            "--cwd" => return Err("--cwd takes no value".to_owned()),
            "-h" | "--help" => return Ok(Parsed::Help),
            other => return Err(format!("unexpected argument '{other}'")),
        }
    }
    Ok(Parsed::Run(asked))
}

/// The settings to run with: the arguments, then the configuration file,
/// then the defaults.
fn configure(args: config::Args) -> Result<Config, String> {
    let home = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from);
    let file = match &args.config {
        Some(path) => config::load(path, true)?,
        None => match config::default_path(std::env::var_os("XDG_CONFIG_HOME"), home.as_deref()) {
            Some(path) => config::load(&path, false)?,
            None => config::File::default(),
        },
    };
    let env = config::Env {
        home,
        current_dir: std::env::current_dir().ok(),
        default_projects_dir: default_projects_dir(),
    };
    config::resolve(args, file, &env)
}

/// Runs `iktinos serve` with its arguments (without `serve` itself) until
/// the process is stopped.
pub fn run(args: impl IntoIterator<Item = String>) -> ExitCode {
    let fail = |message: String| {
        eprintln!("iktinos serve: {message}");
        ExitCode::from(2)
    };
    let asked = match parse_args(args) {
        Ok(Parsed::Run(asked)) => asked,
        Ok(Parsed::Help) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(message) => return fail(format!("{message}\n\n{USAGE}")),
    };
    // What is wrong with a configuration file is said as it is: the usage
    // would not help.
    let config = match configure(asked) {
        Ok(config) => config,
        Err(message) => return fail(message),
    };
    let port = config.port;
    let server = match Server::start(config) {
        Ok(server) => server,
        Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
            return fail(format!(
                "port {port} is in use: stop what holds it, or pass --port (0 picks a free one)"
            ));
        }
        Err(e) => return fail(format!("cannot listen on 127.0.0.1:{port}: {e}")),
    };
    if let Some(notice) = server.notice() {
        eprintln!("iktinos serve: {notice}");
    }
    println!(
        "iktinos serve is listening on this machine only. Open:\n\n  {}\n",
        server.url()
    );
    println!("The token in the URL is this run's. Stop with Ctrl+C.");
    // Until the process is stopped: the threads do the work.
    loop {
        thread::park();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Parsed, String> {
        parse_args(args.iter().map(|a| (*a).to_owned()))
    }

    #[test]
    fn options_are_read_in_both_spellings() {
        assert_eq!(parse(&[]), Ok(Parsed::Run(config::Args::default())));
        assert_eq!(
            parse(&[
                "--port",
                "0",
                "--projects-dir=/synthetic/projects",
                "--socket-dir",
                "/synthetic/sockets",
                "--no-live",
                "--cwd",
                "--config=/synthetic/config.toml"
            ]),
            Ok(Parsed::Run(config::Args {
                config: Some("/synthetic/config.toml".into()),
                port: Some(0),
                projects_dir: Some("/synthetic/projects".into()),
                socket_dir: Some("/synthetic/sockets".into()),
                no_live: true,
                cwd: true,
            }))
        );
        assert_eq!(parse(&["--help"]), Ok(Parsed::Help));
        assert_eq!(parse(&["-h"]), Ok(Parsed::Help));
        for wrong in [
            &["--port"][..],
            &["--port", "http"],
            &["--port=70000"],
            &["--port=-1"],
            &["--config"],
            &["--cwd=/work"],
            &["--host", "0.0.0.0"],
            &["--bind=0.0.0.0"],
            &["extra"],
        ] {
            assert!(parse(wrong).is_err(), "{wrong:?}");
        }
    }

    #[test]
    fn a_token_is_long_random_and_compared_whole() {
        let (a, b) = (new_token().unwrap(), new_token().unwrap());
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
        assert!(is_token(&a, &a.clone()));
        assert!(!is_token(&a, &b));
        assert!(!is_token(&a, &a[..63]));
        assert!(!is_token(&a, ""));
        assert!(!is_token(&a, &format!("{a}0")));
    }
}
