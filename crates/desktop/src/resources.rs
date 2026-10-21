//! Themis's own resource use: CPU and memory of its process tree in three
//! groups (the app, the built-in browser, and tools such as dev servers and MCP
//! servers), sampled every few seconds, kept for an hour, logged as metadata
//! every minute and checked for steady memory growth (a leak).
//!
//! The macOS WebKit process that draws the window is started by the system, not
//! by Themis, so it is not part of the tree and is not counted.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;

/// How often the server samples.
pub const SAMPLE_EVERY: Duration = Duration::from_secs(5);
/// One hour of samples.
const HISTORY: usize = 720;
/// Samples between diagnostics records (one a minute).
pub const LOG_EVERY: u64 = 12;
/// Memory must have grown this much across the window to count as growth.
const GROWTH_BYTES: u64 = 64 * 1024 * 1024;
/// The growth check compares the first and last ten minutes of half an hour.
const GROWTH_WINDOW_MS: u64 = 30 * 60 * 1000;
const GROWTH_EDGE_MS: u64 = 10 * 60 * 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    App,
    Browser,
    Tools,
}

pub const GROUPS: [Group; 3] = [Group::App, Group::Browser, Group::Tools];

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Usage {
    /// Percent of one CPU core, as Activity Monitor shows it (can exceed 100).
    pub cpu_percent: f32,
    pub memory_bytes: u64,
    pub processes: u32,
}

impl Usage {
    fn add(&mut self, cpu: f32, memory: u64) {
        self.cpu_percent += cpu;
        self.memory_bytes += memory;
        self.processes += 1;
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Sample {
    pub at_ms: u64,
    pub app: Usage,
    pub browser: Usage,
    pub tools: Usage,
}

impl Sample {
    pub fn group(&self, group: Group) -> &Usage {
        match group {
            Group::App => &self.app,
            Group::Browser => &self.browser,
            Group::Tools => &self.tools,
        }
    }

    pub fn total(&self) -> Usage {
        let mut total = Usage::default();
        for group in GROUPS {
            let usage = self.group(group);
            total.cpu_percent += usage.cpu_percent;
            total.memory_bytes += usage.memory_bytes;
            total.processes += usage.processes;
        }
        total
    }
}

/// One process as the sampler sees it.
#[derive(Clone, Debug)]
pub struct Proc {
    pub pid: u32,
    pub parent: Option<u32>,
    pub name: String,
    pub cpu: f32,
    pub memory: u64,
}

fn is_browser(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    ["chrome", "chromium", "msedge", "microsoft edge"]
        .iter()
        .any(|browser| name.contains(browser))
}

/// The usage of `root` and everything under it. Each process belongs to the
/// group of its nearest classified ancestor: Themis's own processes are the
/// app, a browser and its helpers are the browser, anything else (and its
/// children) is tools.
pub fn tally(root: u32, procs: &[Proc], at_ms: u64) -> Sample {
    let mut children: HashMap<u32, Vec<&Proc>> = HashMap::new();
    let mut by_pid: HashMap<u32, &Proc> = HashMap::new();
    for proc in procs {
        by_pid.insert(proc.pid, proc);
        if let Some(parent) = proc.parent.filter(|parent| *parent != proc.pid) {
            children.entry(parent).or_default().push(proc);
        }
    }
    let mut sample = Sample {
        at_ms,
        app: Usage::default(),
        browser: Usage::default(),
        tools: Usage::default(),
    };
    let Some(root_proc) = by_pid.get(&root) else {
        return sample;
    };
    let mut seen = HashSet::new();
    let mut queue = VecDeque::from([(*root_proc, Group::App)]);
    while let Some((proc, inherited)) = queue.pop_front() {
        if !seen.insert(proc.pid) {
            continue;
        }
        let group = if proc.pid == root {
            Group::App
        } else if is_browser(&proc.name) {
            Group::Browser
        } else if inherited == Group::App && proc.name.to_ascii_lowercase().contains("themis") {
            Group::App
        } else if inherited == Group::App {
            Group::Tools
        } else {
            inherited
        };
        match group {
            Group::App => sample.app.add(proc.cpu, proc.memory),
            Group::Browser => sample.browser.add(proc.cpu, proc.memory),
            Group::Tools => sample.tools.add(proc.cpu, proc.memory),
        }
        for child in children.get(&proc.pid).into_iter().flatten() {
            queue.push_back((child, group));
        }
    }
    sample
}

/// How much `group`'s memory grew over the last half hour, when it grew
/// steadily: every sample of the last ten minutes is above every sample of the
/// first ten, and the averages differ by at least 64 MiB. A one-off spike or a
/// sawtooth (garbage collection) does not count.
pub fn growth(history: &[Sample], group: Group) -> Option<u64> {
    let last = history.last()?;
    let start = last.at_ms.checked_sub(GROWTH_WINDOW_MS)?;
    let first_at = history.first()?.at_ms;
    if first_at > start {
        return None;
    }
    let window: Vec<(u64, u64)> = history
        .iter()
        .filter(|sample| sample.at_ms >= start)
        .map(|sample| (sample.at_ms, sample.group(group).memory_bytes))
        .collect();
    let early: Vec<u64> = window
        .iter()
        .filter(|(at, _)| *at < start + GROWTH_EDGE_MS)
        .map(|(_, bytes)| *bytes)
        .collect();
    let late: Vec<u64> = window
        .iter()
        .filter(|(at, _)| *at + GROWTH_EDGE_MS > last.at_ms)
        .map(|(_, bytes)| *bytes)
        .collect();
    if early.is_empty() || late.is_empty() {
        return None;
    }
    let mean = |values: &[u64]| values.iter().sum::<u64>() / values.len() as u64;
    let (early_max, late_min) = (*early.iter().max()?, *late.iter().min()?);
    let grown = mean(&late).saturating_sub(mean(&early));
    (late_min > early_max && grown >= GROWTH_BYTES).then_some(grown)
}

/// What the title bar and `themis call resource_usage` show.
#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub sampled_at_ms: u64,
    pub cpu_percent: f32,
    pub memory_bytes: u64,
    pub cores: usize,
    pub groups: Vec<GroupSnapshot>,
}

#[derive(Clone, Debug, Serialize)]
pub struct GroupSnapshot {
    pub group: Group,
    #[serde(flatten)]
    pub usage: Usage,
    /// Bytes the group's memory grew steadily over the last half hour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub growing_bytes: Option<u64>,
}

pub fn snapshot_of(history: &[Sample], cores: usize) -> Option<Snapshot> {
    let last = history.last()?;
    let total = last.total();
    Some(Snapshot {
        sampled_at_ms: last.at_ms,
        cpu_percent: total.cpu_percent,
        memory_bytes: total.memory_bytes,
        cores,
        groups: GROUPS
            .iter()
            .map(|group| GroupSnapshot {
                group: *group,
                usage: *last.group(*group),
                growing_bytes: growth(history, *group),
            })
            .collect(),
    })
}

/// A process's total CPU time and its memory, read from the OS.
///
/// macOS: `proc_pid_rusage` gives CPU time in machine ticks (converted with
/// the timebase; Apple silicon ticks are not nanoseconds) and the physical
/// footprint, which is what Activity Monitor and `top` show. Summing resident
/// sizes instead counts memory a browser's processes share several times.
#[cfg(target_os = "macos")]
#[allow(deprecated)] // libc's timebase call; the `mach2` crate is not worth adding for it.
fn os_usage(pid: u32) -> Option<(u64, u64)> {
    static TIMEBASE: std::sync::OnceLock<(u64, u64)> = std::sync::OnceLock::new();
    let (numer, denom) = *TIMEBASE.get_or_init(|| {
        let mut info = libc::mach_timebase_info { numer: 0, denom: 0 };
        // SAFETY: `info` is a valid, writable timebase struct.
        let ok = unsafe { libc::mach_timebase_info(&mut info) } == 0;
        if ok && info.denom != 0 {
            (u64::from(info.numer), u64::from(info.denom))
        } else {
            (1, 1)
        }
    });
    // SAFETY: an all-zero rusage_info_v2 is a valid value to be overwritten.
    let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
    // SAFETY: the buffer is a rusage_info_v2, as RUSAGE_INFO_V2 asks for.
    let status = unsafe {
        libc::proc_pid_rusage(
            pid as libc::c_int,
            libc::RUSAGE_INFO_V2,
            std::ptr::addr_of_mut!(info).cast::<libc::rusage_info_t>(),
        )
    };
    if status != 0 {
        return None;
    }
    let ticks = u128::from(info.ri_user_time) + u128::from(info.ri_system_time);
    let nanos = ticks * u128::from(numer) / u128::from(denom);
    Some((
        u64::try_from(nanos).unwrap_or(u64::MAX),
        info.ri_phys_footprint,
    ))
}

#[cfg(not(target_os = "macos"))]
fn os_usage(_pid: u32) -> Option<(u64, u64)> {
    None
}

/// Samples the server's process tree.
pub struct Monitor {
    system: Mutex<sysinfo::System>,
    /// Each process's CPU time at the previous sample, and when that was.
    cpu_times: Mutex<(HashMap<u32, u64>, Option<std::time::Instant>)>,
    history: Mutex<VecDeque<Sample>>,
    cores: usize,
    logged: Mutex<Logged>,
}

/// What the diagnostics log was last told.
#[derive(Default)]
struct Logged {
    samples: u64,
    stream: themis_core::browser::StreamStats,
    /// When each group's growth was last reported.
    growth_at: HashMap<Group, u64>,
}

impl Default for Monitor {
    fn default() -> Self {
        Self {
            system: Mutex::new(sysinfo::System::new()),
            cpu_times: Mutex::new((HashMap::new(), None)),
            history: Mutex::new(VecDeque::with_capacity(HISTORY)),
            cores: std::thread::available_parallelism().map_or(1, usize::from),
            logged: Mutex::new(Logged::default()),
        }
    }
}

impl Monitor {
    /// Takes one sample and adds it to the history. The first sample's CPU is
    /// zero: CPU use is the CPU time spent between two samples over the wall
    /// time between them.
    pub fn sample(&self) -> Sample {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate};
        let me = std::process::id();
        let mut procs: Vec<Proc> = {
            let mut system = self.system.lock().unwrap_or_else(|e| e.into_inner());
            system.refresh_processes_specifics(
                ProcessesToUpdate::All,
                true,
                ProcessRefreshKind::nothing().with_cpu().with_memory(),
            );
            system
                .processes()
                .values()
                .map(|process| Proc {
                    pid: process.pid().as_u32(),
                    parent: process.parent().map(|parent| parent.as_u32()),
                    name: process.name().to_string_lossy().into_owned(),
                    cpu: process.cpu_usage(),
                    memory: process.memory(),
                })
                .collect()
        };
        if cfg!(target_os = "macos") {
            let mut times = self.cpu_times.lock().unwrap_or_else(|e| e.into_inner());
            let now = std::time::Instant::now();
            let elapsed = times
                .1
                .map(|then| now.duration_since(then).as_nanos() as f64);
            let mut next = HashMap::new();
            for proc in &mut procs {
                let Some((cpu_ns, footprint)) = os_usage(proc.pid) else {
                    continue;
                };
                proc.memory = footprint;
                proc.cpu = match (times.0.get(&proc.pid), elapsed) {
                    (Some(before), Some(elapsed)) if elapsed > 0.0 => {
                        (cpu_ns.saturating_sub(*before) as f64 / elapsed * 100.0) as f32
                    }
                    _ => 0.0,
                };
                next.insert(proc.pid, cpu_ns);
            }
            *times = (next, Some(now));
        }
        // The desktop window starts the server; count from the window when it does.
        let parent = procs
            .iter()
            .find(|proc| proc.pid == me)
            .and_then(|proc| proc.parent);
        let root = parent
            .and_then(|parent| procs.iter().find(|proc| proc.pid == parent))
            .filter(|proc| proc.name.to_ascii_lowercase().contains("themis"))
            .map_or(me, |proc| proc.pid);
        let at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let sample = tally(root, &procs, at_ms);
        let mut history = self.history.lock().unwrap_or_else(|e| e.into_inner());
        if history.len() == HISTORY {
            history.pop_front();
        }
        history.push_back(sample.clone());
        sample
    }

    /// Every minute: one `resources`/`sample` record with each group's use and
    /// the Workspace stream's counts for the minute. A group whose memory grows
    /// steadily gets a `memory_growth` warning, at most every half hour.
    pub fn log(&self, log: &themis_core::diagnostics::DiagnosticLog) {
        let Some(snapshot) = self.snapshot() else {
            return;
        };
        let mut logged = self.logged.lock().unwrap_or_else(|e| e.into_inner());
        logged.samples += 1;
        for group in &snapshot.groups {
            let Some(grown) = group.growing_bytes else {
                continue;
            };
            let last = logged.growth_at.get(&group.group).copied().unwrap_or(0);
            if snapshot.sampled_at_ms.saturating_sub(last) >= GROWTH_WINDOW_MS {
                logged.growth_at.insert(group.group, snapshot.sampled_at_ms);
                let _ = log.append(
                    "resources",
                    "memory_growth",
                    "warn",
                    serde_json::json!({
                        "group": group.group,
                        "grown_bytes": grown,
                        "memory_bytes": group.usage.memory_bytes,
                        "window_minutes": GROWTH_WINDOW_MS / 60_000,
                    }),
                );
            }
        }
        if logged.samples % LOG_EVERY != 1 {
            return;
        }
        let stream = themis_core::browser::stream_stats();
        let previous = std::mem::replace(&mut logged.stream, stream);
        let _ = log.append(
            "resources",
            "sample",
            "info",
            serde_json::json!({
                "cpu_percent": snapshot.cpu_percent.round(),
                "memory_bytes": snapshot.memory_bytes,
                "cores": snapshot.cores,
                "groups": snapshot.groups.iter().map(|group| serde_json::json!({
                    "group": group.group,
                    "cpu_percent": group.usage.cpu_percent.round(),
                    "memory_bytes": group.usage.memory_bytes,
                    "processes": group.usage.processes,
                })).collect::<Vec<_>>(),
                "stream": {
                    "starts": stream.starts - previous.starts,
                    "stops": stream.stops - previous.stops,
                    "restarts_after_lag": stream.restarts_after_lag - previous.restarts_after_lag,
                    "frames": stream.frames - previous.frames,
                    "lagged_events": stream.lagged_events - previous.lagged_events,
                },
            }),
        );
    }

    pub fn snapshot(&self) -> Option<Snapshot> {
        let mut history = self.history.lock().unwrap_or_else(|e| e.into_inner());
        snapshot_of(history.make_contiguous(), self.cores)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: u32, parent: u32, name: &str, cpu: f32, mb: u64) -> Proc {
        Proc {
            pid,
            parent: Some(parent),
            name: name.into(),
            cpu,
            memory: mb * 1024 * 1024,
        }
    }

    #[test]
    fn the_tree_is_split_into_app_browser_and_tools() {
        let procs = vec![
            proc(1, 0, "launchd", 1.0, 10),
            proc(10, 1, "themis-desktop", 2.0, 100),
            proc(11, 10, "themis-desktop", 20.0, 150),
            proc(12, 11, "Google Chrome", 40.0, 120),
            proc(13, 12, "Google Chrome Helper (GPU)", 20.0, 300),
            proc(14, 11, "sh", 0.0, 2),
            proc(15, 14, "node", 30.0, 200),
            proc(16, 15, "esbuild", 5.0, 40),
            proc(20, 1, "Google Chrome", 90.0, 999),
        ];
        let sample = tally(10, &procs, 7);
        assert_eq!(sample.app.processes, 2);
        assert_eq!(sample.app.cpu_percent, 22.0);
        assert_eq!(sample.browser.processes, 2);
        assert_eq!(sample.browser.memory_bytes, 420 * 1024 * 1024);
        assert_eq!(sample.tools.processes, 3);
        assert_eq!(sample.tools.cpu_percent, 35.0);
        // The person's own Chrome (pid 20) is not Themis's.
        assert_eq!(sample.total().processes, 7);
        assert_eq!(tally(99, &procs, 7).total().processes, 0);
    }

    fn history(memory_mb: impl Fn(u64) -> u64, minutes: u64) -> Vec<Sample> {
        (0..=minutes * 12)
            .map(|step| {
                let mut sample = Sample {
                    at_ms: step * 5_000,
                    app: Usage::default(),
                    browser: Usage::default(),
                    tools: Usage::default(),
                };
                sample.browser.memory_bytes = memory_mb(step) * 1024 * 1024;
                sample
            })
            .collect()
    }

    #[test]
    fn steady_growth_over_half_an_hour_is_reported() {
        // 300 MiB rising by 4 MiB a minute for 40 minutes.
        let rising = history(|step| 300 + step / 3, 40);
        let grown = growth(&rising, Group::Browser).expect("growth");
        assert!(grown >= 64 * 1024 * 1024, "{grown}");
        assert_eq!(growth(&rising, Group::App), None);
        // 1.5 MiB a minute stays under the 64 MiB the check needs.
        assert_eq!(
            growth(&history(|step| 300 + step / 8, 40), Group::Browser),
            None
        );
        // Too short a history says nothing yet.
        assert_eq!(
            growth(&history(|step| 300 + step, 20), Group::Browser),
            None
        );
        // A sawtooth (memory freed now and then) is not a leak.
        let sawtooth = history(|step| 300 + (step % 60) * 3, 40);
        assert_eq!(growth(&sawtooth, Group::Browser), None);
        // A flat line with one spike is not a leak.
        let spike = history(|step| if step == 400 { 2000 } else { 300 }, 40);
        assert_eq!(growth(&spike, Group::Browser), None);
    }

    #[test]
    fn the_snapshot_totals_the_latest_sample() {
        let mut samples = history(|_| 100, 1);
        samples.last_mut().unwrap().app = Usage {
            cpu_percent: 12.5,
            memory_bytes: 50,
            processes: 2,
        };
        let snapshot = snapshot_of(&samples, 8).unwrap();
        assert_eq!(snapshot.cpu_percent, 12.5);
        assert_eq!(snapshot.memory_bytes, 100 * 1024 * 1024 + 50);
        assert_eq!(snapshot.groups.len(), 3);
        assert!(snapshot_of(&[], 8).is_none());
    }

    proptest::proptest! {
        // Process lists come from the OS and can be in any state (cycles,
        // missing parents, a pid that is its own parent).
        #[test]
        fn any_process_list_tallies_each_process_at_most_once(
            raw in proptest::collection::vec((0u32..40, 0u32..40, 0u8..4), 0..60),
        ) {
            let procs: Vec<Proc> = raw
                .iter()
                .map(|(pid, parent, kind)| proc(*pid, *parent, ["themis", "Google Chrome", "node", "sh"][*kind as usize], 1.0, 1))
                .collect();
            let unique: HashSet<u32> = procs.iter().map(|p| p.pid).collect();
            for root in 0..40 {
                let total = tally(root, &procs, 0).total();
                proptest::prop_assert!(total.processes as usize <= unique.len());
            }
        }
    }
}
