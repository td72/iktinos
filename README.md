# iktinos

Claude Code sessions as data.

Claude Code keeps one JSON Lines transcript per session under
`~/.claude/projects/`. This repository turns those files — and, optionally,
live events from a Claude Code mod — into a model of every session on the
machine: where it runs, what it is doing, what it has used, which tools and
subagents it has called. It is a library first (`iktinos-core`);
[vig](https://github.com/td72/vig) renders it as a page, and the `iktinos`
command is a terminal viewer over it.

![demo](https://raw.githubusercontent.com/td72/iktinos/main/assets/demo.gif)

(Recorded from invented sessions: see `tape/`.)

*Iktinos* (ἴκτινος) is ancient Greek for the kite, a bird that circles high
and watches the ground; it was also the name of an architect of the Parthenon.

> Status: the crates (`iktinos-core`, `iktinos-live`, `iktinos-tui`, `iktinos`)
> and the mod are implemented. Up to 0.2 the library was the crate named
> `iktinos` and the command came with `iktinos-tui`: from 0.3 the library is
> `iktinos-core` and `cargo install iktinos` installs the command. The project was called `claude-code-sessions` in its
> first days.

## Using the crate

```rust
use iktinos_core::discover::default_projects_dir;
use iktinos_core::follow::Follower;
use iktinos_core::session::SessionStore;

let mut store = SessionStore::new();
let mut follower = Follower::new(default_projects_dir().ok_or("no home directory")?);

// Call this again whenever the files may have grown: the first poll reads
// everything, each one after it only what was appended.
follower.poll(&mut store)?;

for session in store.sessions() {
    println!("{} {:?} {:?}", session.label(), session.model, session.cost.usd);
}
```

`Follower` is the loop every consumer runs: it finds the sessions, reads what
was appended to each transcript, and makes a session start over when one of
its files was replaced. The parts it is made of (`discover`, `Tail`,
`SessionStore::apply`) are there for a consumer that wants its own loop:
`crates/iktinos-core/examples/sessions.rs` is one, with a table at the end,
and `examples/drift.rs` beside it reports transcript shapes the crate does not
know yet.

## Crates

| crate | role |
| --- | --- |
| [`iktinos-core`](crates/iktinos-core) | the library: `record` (typed transcript lines, raw kept, unknown shapes surveyed), `discover` (sessions, subagents, cwd resolution), `tail` (offset-based reading and append following), `session` (`Session` / `SessionStore`, and what is derived from a session: its status, its name, where it runs), `follow` (the loop that keeps a store current), `trace` (the latest interactions as trees of spans), `output` (a tool call's whole output, read on demand), `place` (whether a session runs in a directory or in a worktree of its repository), `pricing` (embedded price table) |
| [`iktinos-live`](crates/iktinos-live) | the live-event protocol, the Unix-socket server that receives it, and the merge into `Session`. Listen only; never sends |
| [`iktinos-tui`](crates/iktinos-tui) | the terminal viewer, as a library |
| [`iktinos-web`](crates/iktinos-web) | the sessions in a browser, as a library: the page, and the local server `iktinos serve` starts |
| [`iktinos`](crates/iktinos) | the `iktinos` command: the way in. Without a subcommand it starts the viewer; `iktinos serve` starts the server |
| [`mods/iktinos-bridge`](mods/iktinos-bridge) | the Claude Code mod that POSTs live events to every consumer's socket |

## The viewer

```bash
cargo install iktinos         # installs the `iktinos` command
iktinos                       # the sessions on this machine
iktinos --projects-dir tests/fixtures/2.1.325/projects --no-live
iktinos --cwd                 # only the sessions of where it was started
```

`--cwd` narrows every page but Usage to the sessions that run in the current
directory, below it (by whole names: `foo` is not `foobar`), or in another
worktree of the same git repository. A session whose location is not known is
left out, and so is one of a worktree that has been removed. The lists'
titles say where (`in …/td72/iktinos`), the header counts what is shown, and
the Usage page still totals every session. To tell worktrees of one
repository the viewer reads the repository's `.git` (a directory, or a
worktree's file and the `commondir` it leads to); it starts no `git` and
writes nothing.

Five pages, in the shape and with the keys of [vig](https://github.com/td72/vig).
The viewer starts on the first.
Each list page is a list over a detail pane, in fixed proportions:

- **1:Trace** — three panes. The latest interactions (a prompt and
  everything done to answer it), the latest first; under them the
  interaction under the cursor as a tree: its LLM requests and tool calls,
  the subagents the calls spawned and what they did, each with a bar on a
  time axis that is as long as the interaction; and beside the list what is
  chosen: where the interaction's time went, or, in the tree, a request's
  tokens and cost or a call's command.
  A request's row says what its response did: the first line of what it
  said, or, where it said nothing, how long it thought and the tools it
  called (`thinking 12.0s → Read, Bash`); the calls are under it. What sets
  a request apart comes first on its row (`context rebuilt`,
  `shape changed`), and a stop reason other than `tool_use` and `end_turn`
  is red.
  The interactions are listed under their sessions, a session's row
  starting with where it runs; `t` lists them all together instead, the
  latest first. A row says what was asked (the first line of the prompt),
  or, where the prompt is not known, the tools that were used. On a
  session's row the pane beside the list describes the session, and there
  is no tree.

  ![trace demo](https://raw.githubusercontent.com/td72/iktinos/main/assets/demo-trace.gif)

- **2:Sessions** — every session: title, repository, branch, state, last
  update, tokens, cost, and under them a row with what the sessions listed
  add up to (those a search matches, where there is one). The detail pane
  shows the selected session's context, its tokens and cost by model and by
  source, and its rate limits.
- **3:Tool calls** — recent tool calls, the latest first: what ran, how long
  it took, how it ended. The detail pane shows the whole command, every line
  of it, a shell command in colour, and under it the start of what the call
  returned (`o` reads all of it).

  ![tool calls demo](https://raw.githubusercontent.com/td72/iktinos/main/assets/demo-tools.gif)

- **4:Subagents** — subagents with their type, task, model, tokens and cost;
  the detail is two panes, the subagent with its own tool calls and, beside
  it, the call chosen there with its whole command.

  ![subagents demo](https://raw.githubusercontent.com/td72/iktinos/main/assets/demo-subagents.gif)

- **5:Usage** — tokens and estimated cost by model and by source over every
  session, and sessions, tokens and cost by repository.

The Trace, Tool calls and Subagents pages cover every session until `Enter`
is pressed on one in the Sessions page; then they are narrowed to it. `Enter` on a subagent narrows the
Tool calls page further, to that subagent. `Esc` undoes one step at a time.

| key | |
| --- | --- |
| `1` – `5` | switch page |
| `j` / `k`, `gg` / `G`, `Ctrl+d` / `Ctrl+u` | move in the list (or scroll the detail pane) |
| `Enter` on a session | narrow the Trace, Tool calls and Subagents pages to it and show its tool calls |
| `Enter` on a subagent | narrow the Tool calls page to it and show its tool calls |
| `i` / `Enter`, `Tab`; then `h` / `Esc` | into the detail pane and back, a pane at a time |
| `j` / `k` in a subagent's detail, or in an interaction's tree; then `i` / `Enter` | choose a tool call, or a span; into the pane that shows it |
| `t` in the trace | list the interactions all together, the latest first, or under their sessions again |
| `Enter` on a session in that list, or on a subagent in an interaction's tree | unfold it, fold it |
| `l`, `h` in the trace | the pane on the right, and back (`h` in the tree or under a session goes to the row above first) |
| `/`, `n` / `N` | search: the word stands out on the rows that match, the cursor goes to the first, and `n` / `N` to the next and the one before. A row that matches by something it does not show (a later line of a command, text cut off at the edge) has a mark on the frame beside it. With the focus on an interaction's tree the search is of the tree, where a call is found by every line of its command, a response by what it said and a subagent by its task; a folded subagent is unfolded. Anywhere else it is of the page's list, whose rows are found by what they show; in the trace it unfolds the sessions |
| `Esc` on a list | clear the search, then the repository, then the narrowing |
| `Esc` in an interaction's tree | clear the tree's search, then leave the tree |
| `o` on a tool call | read its whole output from the transcript and show it; again, the excerpt |
| `o` on a request in an interaction's tree | read all its response said from the transcript and show it; again, the first line |
| `T` | show times as a local date and time, or as how long ago (`--time clock` or `--time relative` sets which to start with; with `clock`, what is newer than `--ago`, an hour unless told otherwise, is still shown as how long ago) |
| `s`, `f` | sort sessions (updated, cost, tokens); filter them (all, live, open) |
| `F` on a session | list only the sessions of its repository; again, or `Esc`, all of them |
| `r`, `?`, `q` | refresh, help, quit |

It reads the transcripts once a second and, unless `--no-live` is given,
listens for live events on its own socket. The end of the header says what
that socket has heard, since an open socket says nothing about a mod that
sends:

| header | |
| --- | --- |
| `live off` | not listening (`--no-live`) |
| `live failed` | the socket could not be opened; the footer says why |
| `live on, no events yet` | listening, and nothing has arrived: the mod is not installed, is disabled, or posts to another directory |
| `live 128 events, last 3s ago` | receiving. It turns yellow when a session is busy and nothing has arrived for a minute |

A session's detail says when its own last event arrived, which tells a
session the mod does not report on from one that is idle. The help (`?`) ends
with the socket's path, to compare with where the mod posts, and with how
many events were of a type the viewer does not know and how many requests it
turned away: the signs of a mod and a viewer that do not match.

A cost shown as `~$1.23` contains
an estimate; `$1.23` is Claude Code's own figure.

## In a browser

```bash
iktinos serve                 # prints the URL to open
iktinos serve --port 8080 --cwd
```

`iktinos serve` shows the sessions in a browser **on the same machine**. It
is one more consumer of the model, like the viewer; what it adds is a way
out, and that is kept narrow:

- **Local only.** It listens on `127.0.0.1`, and no option makes it listen
  anywhere else. A request whose `Host` is not the server's own address is
  refused, so a page elsewhere cannot reach it under a name that resolves to
  this machine.
- **A token.** The URL it prints carries one, new on every run. The first
  visit exchanges it for a cookie (`HttpOnly`, `SameSite=Strict`) and is
  redirected to the same address without it, which keeps the token out of the
  address bar and the history. Without the cookie nothing is answered.
- **Read-only.** `GET` and nothing else. What reads a file again (the whole
  output of a tool call) is asked for by a session id and a call id; the
  path is the server's to find, and nothing in an address is ever part of
  one.
- **Modelled shapes only.** The JSON is built from the model, never from a
  raw record, and the page may load and run only what the server itself
  serves (a Content-Security-Policy without inline scripts and without
  connections elsewhere).

The port is 4747 unless `--port` names another (`0` picks a free one); a port
in use is an error. `--projects-dir`, `--socket-dir`, `--no-live` and `--cwd`
mean what they mean for the viewer.

The page shows the sessions on the left and, for the one chosen, its latest
interactions on the right: each a tree with a bar per row on a time axis, and
the row chosen in full below it (a command and what it returned, a request's
tokens and what its response said, read whole on demand). The session that is
shown is also copied to the top of the list, where it stays while the others
move. The pane beside the list has tabs: **Trace**, **Session** (what the
session is and has used: its context, its tokens and cost by model and by
source, its rate limits) and **Tool calls** (its latest 200, the latest
first, subagents' included, and the one chosen in full) and **Subagents**
(each with its type, task, model, tokens and cost, and the calls of the one
chosen). **Usage** in the bar at the top shows, in place of that pane, what
the sessions together have used: by model, by source and by repository.

Above the list, a box finds sessions by what their rows show (every word
typed must be there), a choice narrows them to the live or the open ones, and
the headings of the updated, tokens and cost columns order them. The Tool
calls tab has a box of its own. The order and the choice are kept in the
browser; nothing typed is sent to the server.

The page is drawn from JSON the server returns (`/api/sessions`,
`/api/sessions/<id>`, `/api/sessions/<id>/trace`, `/api/usage`, a call's whole output, a
response's whole text) and reads it again when the server says something
changed (server-sent events, `/api/events`).

### Building the page

The page (`crates/iktinos-web/web`) is built with Vite+, TypeScript and
React. Node comes from mise and Vite+ from the page's own `package.json`,
through npm:

```bash
mise run web:build    # type-check and build into web/dist
mise run web:check    # format, lint and type-check
mise run serve        # build the page, then run `iktinos serve`
```

`cargo build` embeds `web/dist` in the binary if it is there, and a
placeholder page if it is not: cargo never runs npm, and a checkout without
Node still builds. The crate published to crates.io holds the built page.

## Configuration

The viewer reads `~/.config/iktinos/config.toml` (under `XDG_CONFIG_HOME`
where that is set), or the file `--config` names. Without a file everything
is at its default. An argument goes before the file, the file before the
defaults. The viewer only reads the file.

```toml
projects_dir = "~/.claude/projects"
cwd = false
refresh = "1s"

[pages]
order = ["trace", "sessions", "tools", "subagents", "usage"]
start = "trace"

[time]
style = "clock"
ago = "1h"

[live]
enabled = true
quiet_after = "1m"

[sessions]
sort = "updated"
filter = "all"
active_within = "1m"

[trace]
by_session = true
interactions = 20
slow = ["10s", "1m"]

[tools]
recent = 200
output_chars = 2000
```

| key | default | |
| --- | --- | --- |
| `projects_dir` | `~/.claude/projects` | where the transcripts are (`--projects-dir`) |
| `cwd` | `false` | show only the sessions of where the viewer was started (`--cwd`) |
| `refresh` | `1s` | how often the transcripts are read again |
| `pages.order` | all five | the pages shown, in the order of their keys from `1`: `trace`, `sessions`, `tools`, `subagents`, `usage`. A page left out is not shown |
| `pages.start` | the first of `order` | the page the viewer starts on |
| `time.style` | `clock` | `clock` or `relative` (`--time`) |
| `time.ago` | `1h` | with `clock`, what is newer is still shown as how long ago (`--ago`) |
| `live.enabled` | `true` | listen for live events (`--no-live` turns it off) |
| `live.socket_dir` | the shared default | where to listen (`--socket-dir`) |
| `live.quiet_after` | `1m` | a session is busy and nothing has arrived for this long: the header turns yellow |
| `sessions.sort` | `updated` | `updated`, `cost` or `tokens`, to begin with |
| `sessions.filter` | `all` | `all`, `live` or `open`, to begin with |
| `sessions.active_within` | `1m` | how recent a record makes a session active when no live event says more |
| `trace.by_session` | `true` | list the interactions under their sessions to begin with |
| `trace.interactions` | `20` | how many of its latest interactions a session keeps |
| `trace.slow` | `["10s", "1m"]` | how long something takes before it is yellow, then red |
| `tools.recent` | `200` | how many of its latest tool calls a session keeps |
| `tools.output_chars` | `2000` | how much of a call's output is kept as its excerpt, in characters; `0` keeps none |

A length of time is written as `90s`, `30m`, `1h` or `2d`. A leading `~` in a
path is the home directory.

### What `iktinos serve` reads of it

The same file (`--config` names another), and the keys that mean something
to a server: `projects_dir`, `cwd`, `refresh`, `live.enabled`,
`live.socket_dir`, `sessions.active_within`, `tools.recent` and
`tools.output_chars`. The port is not in the file: `--port` sets it.

The rest is the viewer's and the server passes over it. So the server does
not stop at a key it does not know, as the viewer does, and a key misspelt
for it goes unnoticed; a key it knows with a value it cannot read stops it
with the line it is on.

### Colours

The viewer draws in the terminal's own sixteen colours, so it follows the
terminal's palette. `[colors]` gives a role another colour; a role that is
not set keeps its own.

```toml
[colors]
warn = "light-red"
error = "#ff5555"
dim = "gray"
```

| role | default | what is drawn in it |
| --- | --- | --- |
| `label` | `cyan` | the names of fields, the headings inside a pane, the keys in the help |
| `headline` | `yellow` | the first line of a detail |
| `focus` | `cyan` | the frame of the pane that has the focus |
| `session` | `cyan` | a session's name over its interactions; the mark of the session the lists are narrowed to |
| `cwd` | `yellow` | where a session runs |
| `tool` | `cyan` | a tool's name |
| `agent` | `magenta` | a subagent |
| `request` | `light-blue` | an LLM request |
| `ok` | `green` | finished and well, working, plenty of room |
| `warn` | `yellow` | running, slow, waiting for approval, nearly full |
| `error` | `red` | failed, very slow, full, cannot be read |
| `waiting` | `cyan` | a session that waits for its user |
| `dim` | `dark-gray` | captions, what is absent or unknown |
| `search` | `yellow` | the word a search found, the mark beside a row whose match does not show, and the search being typed |

A colour is `black`, `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`,
`gray`, `dark-gray`, `white`, the `light-` form of the six in the middle,
`default` (the terminal's own foreground), or `#rrggbb`. The colours of a
shell command's syntax are not roles and cannot be set.

A key the viewer does not know, a value of the wrong type, a length of time,
a name or a colour it cannot read, a page listed twice, an empty list of
pages: each stops the viewer at startup, with the file, the line and what is
wrong. A setting that is silently ignored is one nobody notices was misspelt.

## Two inputs, one state

| | transcripts (JSONL) | mod (live events) |
| --- | --- | --- |
| setup | none | install the mod |
| past sessions | yes | no |
| latency | seconds, a turn at a time | immediate |
| owns | existence, tokens, cost, history, title, branch, cwd | running tool, waiting for input, context fill, rate limits |

Both feed a `SessionStore` incrementally. Snapshot fields are last-write-wins by
timestamp; accumulated fields are folded once from the transcript offsets. No
field is written by both inputs, so arrival order never changes the result.

![live demo](https://raw.githubusercontent.com/td72/iktinos/main/assets/demo-live.gif)

In the viewer, what only the events can say: the tool that runs now, and a
permission that is waited for.

Text is modelled where a view needs it (a title, the last prompt, the first
line of each recent prompt and of each recent response, a subagent's
description, what a tool call was about and, in a store that asks for it, the
start of what the call returned).
The crates never log record contents themselves, and a session keeps bounded
recent activity, not its whole history: its latest tool calls, and its latest
interactions as a trace.

What a tool returned is kept in two tiers. A store that asks for it
(`SessionStore::with_tool_output`) keeps the first characters of each call's
output; a store that does not keeps none. The whole output is never kept:
`output::read_tool_output` reads it again from the session's files when
someone wants to see it, including one Claude Code moved to a file of its own,
and hands it to the caller.

## The trace

`Session::trace` holds a session's latest interactions (20 unless the store is
told otherwise), and `Session::walk` reads one as a tree. The names and the
hierarchy are those of the traces Claude Code exports itself through
OpenTelemetry (`claude_code.interaction`, `claude_code.llm_request`,
`claude_code.tool`), rebuilt here from the transcript:

```
interaction
├─ LLM request
│  └─ tool call         (the response that called it)
│     └─ subagent       (the call that spawned it)
│        ├─ LLM request
│        │  └─ tool call
│        └─ LLM request
└─ LLM request
```

This is where the tree departs from the exported traces, in which LLM
requests and tool calls are siblings (and a thread still keeps them side by
side, in order): `Session::walk` hangs a tool call under the response that
called it, so that a request's row says what its response did. "Under" means
"called by": a call runs after its response has ended. A call whose response
was not read stays a sibling.

An LLM request carries the first line of what its response said, how long it
thought (from the request to the end of its thinking block; what it thought
is not modelled), and two marks: `rebuilt`, a request in the middle of a
thread that does not continue the one before it (the conversation was sent
anew, as after a compaction), and `reshaped`, one whose system prompt or
tools differ from the request before. The whole text of a response is read
again on demand (`output::read_response`) and kept nowhere.

An interaction runs from a prompt to Claude Code's own `turn_duration` record;
one without it is still running or was cut off, and the transcript cannot tell
which. (The transcript calls this a turn. The Agent SDK's `max_turns` counts
something else: one pass of the agent loop.) A tool call runs from its block to
its result. An LLM request runs from its `api-request` record to the last
block of its response, which is an approximation: the transcript records when
blocks were written, not when the API answered. A subagent hangs under the
tool call named in its metadata file, or, without one, under the interaction
it started in. The time a call waited for approval, hooks and retries are in
the exported traces and not in the transcript.

Only the transcripts write the trace; what is running right now still comes
from the live events.

## Cost

Claude Code writes its own running total into the transcript when it exits.
That total includes requests the transcript never records, so it is used where
it exists. But it stops at the last clean exit, and a process that was killed
leaves it far behind what the transcript shows. A session's cost is therefore
the larger of "reported total plus the price-table estimate of everything after
it" and "the estimate of everything", and it carries its source: reported,
reported plus estimate, or estimate. Token counts always come from the
transcript, one count per API response.

## Prices

`pricing` embeds a snapshot of Anthropic's first-party price list
(`crates/iktinos-core/assets/pricing.json`: input, output, 5 minute and
1 hour cache writes, cache reads and the context window per model, plus fast
mode and Claude Haiku 5.5's long-prompt prices). A model the table does not
know has no price; it is shown as `—`, never as `$0`.

To refresh the snapshot, copy the rows of the
[pricing page](https://platform.claude.com/docs/en/about-claude/pricing) into
the JSON, set `as_of`, and run `cargo test -p iktinos-core pricing`:
the tests reject a table whose rates break the published multipliers (cache
writes at 1.25× and 2× the input price, output at 5×). Automatic refresh is
future work.

## Why a crate

The popular tools in this space (ccusage, Claude-Code-Usage-Monitor,
claudelytics, ccboard) are applications, and the library crates on crates.io
are weeks old. The format is undocumented and moves with every Claude Code
release, so the parser keeps the raw record, treats unknown types and keys as
data for a drift report, and never fails a whole file on one bad line.

## License

MIT.
