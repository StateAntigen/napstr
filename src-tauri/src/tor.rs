use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::{
    fs,
    io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader},
    net::TcpStream,
    process::{Child, Command},
    sync::{Mutex, RwLock},
    time::{sleep, timeout},
};
use tokio_socks::tcp::Socks5Stream;
use tokio_util::sync::CancellationToken;

struct RunningTor {
    child: Child,
    socks_port: u16,
    control_port: u16,
    cookie: Vec<u8>,
    data_dir: PathBuf,
    /// Whether `data_dir` belongs to this run alone, and so may be removed when it
    /// ends. The cache directory may not: it is what makes the next start quick.
    private_data_dir: bool,
    /// The handle that keeps Tor inside this application's job object. Held for as
    /// long as Tor should live: closing the last handle to the job is what kills
    /// everything in it, so this must not be dropped early.
    #[cfg(windows)]
    _job: Option<std::os::windows::io::OwnedHandle>,
}

pub struct OnionLease {
    pub onion: String,
    /// The Tor process this onion was published on.
    ///
    /// An onion is ephemeral: it exists on one process and dies with it, because
    /// what keeps it alive is the control connection held here. Tor is restarted
    /// after this machine wakes, so a lease can outlive its onion very easily - and
    /// a stale one is worse than none at all, because every offer built from it
    /// names an address nobody can reach while everything else about this computer
    /// looks healthy.
    generation: u64,
    _control: TcpStream,
}

pub struct TorManager {
    app_data: PathBuf,
    resource_dir: PathBuf,
    runtime: Mutex<Option<RunningTor>>,
    /// Held for the length of one bootstrap, and only for that.
    ///
    /// It is what stops two callers from starting two Tor processes; the runtime
    /// itself is deliberately not held for that long, because every request that
    /// wanted Tor's port would otherwise wait for a consensus download.
    bootstrap_lock: Mutex<()>,
    /// Which Tor process is running, counted rather than described.
    ///
    /// It moves every time the process is published or taken away, so anything
    /// that belongs to one process - an onion lease above all - can tell whether
    /// the process it was made for is still the one in hand.
    generation: AtomicU64,
    starting: AtomicBool,
    cancel_start: AtomicBool,
    bootstrap_progress: AtomicU8,
    last_error: RwLock<String>,
    last_diagnostics: Arc<RwLock<Vec<String>>>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TorStatus {
    pub running: bool,
    pub starting: bool,
    pub bootstrap_progress: u8,
    pub error: String,
}

pub fn is_v3_onion(host: &str) -> bool {
    host.strip_suffix(".onion")
        .map(|service| {
            service.len() == 56
                && service
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || (b'2'..=b'7').contains(&byte))
        })
        .unwrap_or(false)
}

/// What went wrong when Tor was asked to start, and whether another attempt is
/// worth making with a directory of its own.
struct BootstrapFailure {
    message: String,
    /// True when the process exited before it reached the network. That is what a
    /// second Napstr on the same machine looks like from here: Tor refuses to
    /// share a data directory and gives up at once.
    own_directory: bool,
}

impl BootstrapFailure {
    fn other(message: String) -> Self {
        Self {
            message,
            own_directory: false,
        }
    }
}

impl From<String> for BootstrapFailure {
    /// A plain message from inside the start-up code is never evidence that the
    /// directory was the problem, so it may not ask for one of its own.
    fn from(message: String) -> Self {
        Self::other(message)
    }
}

/// How often the bootstrap loop looks at Tor's progress, and how long it is given
/// in total.
///
/// Five minutes, where this used to allow two. A cold start fetches a consensus
/// and some nine thousand relay descriptors, and a start that is cut off is thrown
/// away and begun again - so a cap that is too tight does not fail a start, it
/// fails every start, and the retry begins from a directory with nothing in it.
const BOOTSTRAP_CHECK_MILLIS: u64 = 250;
const BOOTSTRAP_CHECKS: usize = 1200;

/// How long one connect to a seeder's onion is given, end to end.
///
/// A budget counted in attempts is the wrong shape for this. The early attempts
/// against a fresh onion fail *fast* - Tor answers "host unreachable" while the
/// descriptor is still being published - so eight of them can be spent in under a
/// minute, and a fetch that was merely late was reported as a seeder that was not
/// there. A seeder whose own Tor is still bootstrapping may need minutes, and that
/// is the case this has to survive.
const ONION_CONNECT_BUDGET: Duration = Duration::from_secs(180);
/// The longest wait between attempts, so a seeder that is really gone still ends
/// inside the budget rather than at the end of it.
const ONION_CONNECT_MAX_GAP: Duration = Duration::from_secs(10);

impl TorManager {
    pub fn new(app_data: PathBuf, resource_dir: PathBuf) -> Self {
        Self {
            app_data,
            resource_dir,
            runtime: Mutex::new(None),
            bootstrap_lock: Mutex::new(()),
            generation: AtomicU64::new(0),
            starting: AtomicBool::new(false),
            cancel_start: AtomicBool::new(false),
            bootstrap_progress: AtomicU8::new(0),
            last_error: RwLock::new(String::new()),
            last_diagnostics: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub async fn start(&self) -> Result<u16, String> {
        // One bootstrap at a time, and the runtime itself is not held across it: a
        // caller that only wants Tor's port, or its status, is not made to wait
        // behind a consensus download it did not ask for.
        let _bootstrap = self.bootstrap_lock.lock().await;
        if let Some(port) = self.live_socks_port().await {
            return Ok(port);
        }
        self.cancel_start.store(false, Ordering::SeqCst);
        self.starting.store(true, Ordering::SeqCst);
        self.bootstrap_progress.store(0, Ordering::SeqCst);
        self.last_error.write().await.clear();
        self.last_diagnostics.write().await.clear();

        // The cache directory first. Tor keeps the consensus and the relay
        // descriptors it has fetched in its data directory, so a directory that
        // survives a restart is the difference between a start of a few seconds and
        // one of minutes - and Tor is asked to start before anything can be offered
        // or fetched at all. A second Napstr on the same machine cannot share it,
        // because Tor refuses to share one; that case gets a directory of its own,
        // which is also the only kind this file ever removes.
        let result = match self.bootstrap(&self.cached_data_dir(), false).await {
            Ok(port) => Ok(port),
            Err(failure) if failure.own_directory => {
                match self.bootstrap(&self.own_data_dir(), true).await {
                    Ok(port) => Ok(port),
                    Err(failure) => Err(failure.message),
                }
            }
            Err(failure) => Err(failure.message),
        };

        self.starting.store(false, Ordering::SeqCst);
        match &result {
            Ok(_) => {
                self.bootstrap_progress.store(100, Ordering::SeqCst);
                self.last_error.write().await.clear();
            }
            Err(error) => *self.last_error.write().await = error.clone(),
        }
        result
    }

    /// The port of a Tor that is already running, or `None`.
    ///
    /// A process that has exited is forgotten here rather than reported, so that
    /// the next start does not find a dead one in its way.
    async fn live_socks_port(&self) -> Option<u16> {
        let mut guard = self.runtime.lock().await;
        let exited = guard
            .as_mut()
            .map(|runtime| runtime.child.try_wait().ok().flatten().is_some())
            .unwrap_or(false);
        if exited {
            *guard = None;
            return None;
        }
        guard.as_ref().map(|runtime| runtime.socks_port)
    }

    /// Where Tor keeps the consensus and the descriptors it has already fetched.
    fn cached_data_dir(&self) -> PathBuf {
        self.app_data.join("tor-data")
    }

    /// A directory that belongs to one run, for a machine already using the cache.
    fn own_data_dir(&self) -> PathBuf {
        self.app_data
            .join("tor-sessions")
            .join(uuid::Uuid::new_v4().to_string())
    }

    /// Start one Tor process in `data_dir` and wait for it to reach the network.
    async fn bootstrap(&self, data_dir: &Path, private: bool) -> Result<u16, BootstrapFailure> {
        let result = self.bootstrap_inner(data_dir, private).await;
        if result.is_err() && private {
            // Only a directory that belongs to one run is thrown away: the cache is
            // the very thing the next attempt needs.
            let _ = fs::remove_dir_all(data_dir).await;
        }
        result
    }

    async fn bootstrap_inner(
        &self,
        data_dir: &Path,
        private: bool,
    ) -> Result<u16, BootstrapFailure> {
        let tor_data = data_dir.to_path_buf();
        let result: Result<u16, BootstrapFailure> = async {
            let tor = self.find_tor_binary();
            fs::create_dir_all(&tor_data)
                .await
                .map_err(|error| error.to_string())?;
            let cookie_path = tor_data.join("control_auth_cookie");
            let control_port_path = tor_data.join("control-port");
            let _ = fs::remove_file(&cookie_path).await;
            let _ = fs::remove_file(&control_port_path).await;

            let mut command = Command::new(&tor);
            command
                .arg("--DataDirectory")
                .arg(&tor_data)
                .arg("--SocksPort")
                .arg("auto")
                .arg("--ControlPort")
                .arg("auto")
                .arg("--ControlPortWriteToFile")
                .arg(&control_port_path)
                .arg("--CookieAuthentication")
                .arg("1")
                .arg("--CookieAuthFile")
                .arg(&cookie_path)
                .arg("--ClientOnly")
                .arg("1")
                // `--AvoidDiskWrites` used to be set here, and it is why a start had
                // nothing to reuse: it tells Tor to keep no consensus, no descriptors
                // and no service state on disk, so every launch downloaded the whole
                // network again.
                .arg("--Log")
                .arg("notice stdout")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());

            if let Some(parent) = tor.parent() {
                prepend_library_path(&mut command, parent);
            }
            hide_child_process_window(&mut command);
            command.kill_on_drop(true);
            // `kill_on_drop` covers an orderly exit and nothing else: it runs when
            // this process is still there to run it. So the operating system is
            // asked as well, because it outlives us by definition.
            prepare_child_to_die_with_us(&mut command);

            let mut child = command
                .spawn()
                .map_err(|error| format!("could not start Tor at {}: {error}", tor.display()))?;
            #[cfg(windows)]
            let job = own_child_for_this_application(&child);
            if let Some(stdout) = child.stdout.take() {
                capture_process_output(stdout, self.last_diagnostics.clone());
            }
            if let Some(stderr) = child.stderr.take() {
                capture_process_output(stderr, self.last_diagnostics.clone());
            }
            let mut ready = None;
            for _ in 0..BOOTSTRAP_CHECKS {
                if self.cancel_start.load(Ordering::SeqCst) {
                    let _ = child.start_kill();
                    return Err(BootstrapFailure::other(
                        "Tor startup was cancelled for network recovery".into(),
                    ));
                }
                if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
                    sleep(Duration::from_millis(50)).await;
                    let diagnostic = useful_diagnostic(&self.last_diagnostics.read().await);
                    let detail = if diagnostic.is_empty() {
                        String::new()
                    } else {
                        format!(": {diagnostic}")
                    };
                    // An early exit on a shared directory is exactly what a second
                    // Napstr on this machine looks like, so this is the failure worth
                    // retrying with a directory of its own.
                    return Err(BootstrapFailure {
                        message: format!(
                            "Tor exited before bootstrap completed ({status}){detail}"
                        ),
                        own_directory: true,
                    });
                }
                if let (Ok(bytes), Ok(control_address)) = (
                    fs::read(&cookie_path).await,
                    fs::read_to_string(&control_port_path).await,
                ) {
                    let Some(control_port) = listener_port(&control_address) else {
                        sleep(Duration::from_millis(250)).await;
                        continue;
                    };
                    if TcpStream::connect(("127.0.0.1", control_port))
                        .await
                        .is_ok()
                    {
                        let mut control = TcpStream::connect(("127.0.0.1", control_port))
                            .await
                            .map_err(|error| BootstrapFailure::other(error.to_string()))?;
                        if control_command(
                            &mut control,
                            &format!("AUTHENTICATE {}", hex::encode(&bytes)),
                        )
                        .await
                        .is_ok()
                        {
                            let socks_port =
                                control_command(&mut control, "GETINFO net/listeners/socks")
                                    .await
                                    .ok()
                                    .and_then(|lines| listener_port_from_control(&lines));
                            if let Ok(lines) =
                                control_command(&mut control, "GETINFO status/bootstrap-phase")
                                    .await
                            {
                                if let Some(progress) = bootstrap_progress(&lines) {
                                    self.bootstrap_progress.store(progress, Ordering::SeqCst);
                                    // Both facts at once, because a SOCKS port of its
                                    // own is what makes "bootstrapped" usable.
                                    if progress == 100 {
                                        if let Some(socks_port) = socks_port {
                                            ready = Some((bytes, control_port, socks_port));
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                sleep(Duration::from_millis(250)).await;
            }
            let (cookie, control_port, socks_port) = ready.ok_or_else(|| {
                BootstrapFailure::other(format!(
                    "Tor did not complete bootstrap within {} seconds",
                    BOOTSTRAP_CHECKS as u64 * BOOTSTRAP_CHECK_MILLIS / 1000
                ))
            })?;
            // Recorded under the bootstrap lock, so exactly one process is ever held
            // here, and never while it is starting. A new process is a new identity:
            // nothing published on the last one is offered again.
            *self.runtime.lock().await = Some(RunningTor {
                child,
                socks_port,
                control_port,
                cookie,
                data_dir: tor_data,
                private_data_dir: private,
                #[cfg(windows)]
                _job: job,
            });
            self.note_tor_replaced();
            Ok(socks_port)
        }
        .await;
        result
    }

    /// Note that the process in hand is not the one that was there before.
    ///
    /// Called wherever that changes - a process published, a process taken away -
    /// so that everything belonging to the old one can see that it is old. An onion
    /// lease is the thing that matters: an ephemeral onion dies with its Tor, and
    /// Tor is restarted after the machine wakes.
    fn note_tor_replaced(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Which Tor process is in hand.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Whether an onion published on some Tor process still has that process.
    pub fn lease_is_current(&self, lease: &OnionLease) -> bool {
        lease.generation == self.generation()
    }

    pub async fn stop(&self) {
        self.cancel_start.store(true, Ordering::SeqCst);
        // A start that is already in flight is waited for, briefly, before the
        // runtime is taken: it may be a moment away from publishing the process it
        // spawned, and a runtime read before that point would leave that process
        // behind. The cancel above is what makes the wait short - the bootstrap loop
        // sees it within a tick, kills its own child and gives up - and the bound is
        // there because this is an exit path and a bootstrap may be allowed minutes.
        let _ = timeout(Duration::from_secs(10), self.bootstrap_lock.lock()).await;
        if let Some(mut runtime) = self.runtime.lock().await.take() {
            // The process is going, so anything published on it goes with it: this is
            // the moment an onion lease stops being true.
            self.note_tor_replaced();
            let _ = runtime.child.start_kill();
            let _ = timeout(Duration::from_secs(5), runtime.child.wait()).await;
            // Only a directory that belongs to one run is removed. The cache is what
            // makes the next start quick, and what is in it is public network data:
            // the consensus, the relay descriptors and the guards Tor chose.
            if runtime.private_data_dir {
                let _ = fs::remove_dir_all(runtime.data_dir).await;
            }
        }
    }

    pub async fn restart(&self) -> Result<u16, String> {
        // Keep status in "starting" throughout the handover so the regular
        // health poll cannot launch a competing Tor process between stop/start.
        self.starting.store(true, Ordering::SeqCst);
        self.cancel_start.store(true, Ordering::SeqCst);
        self.stop().await;
        self.start().await
    }

    pub async fn status(&self) -> TorStatus {
        let running = self
            .runtime
            .try_lock()
            .ok()
            .and_then(|mut guard| {
                guard
                    .as_mut()
                    .map(|runtime| runtime.child.try_wait().ok().flatten().is_none())
            })
            .unwrap_or(false);
        TorStatus {
            running,
            starting: self.starting.load(Ordering::SeqCst),
            bootstrap_progress: self.bootstrap_progress.load(Ordering::SeqCst),
            error: self.last_error.read().await.clone(),
        }
    }

    pub async fn create_onion(&self, target_port: u16) -> Result<Arc<OnionLease>, String> {
        self.start().await?;
        // Read after the start, so the lease is stamped with the process it is about
        // to be published on rather than the one that may have just been replaced.
        let generation = self.generation();
        let (control_port, cookie) = {
            let guard = self.runtime.lock().await;
            let runtime = guard.as_ref().ok_or("Tor runtime disappeared")?;
            (runtime.control_port, runtime.cookie.clone())
        };
        let mut stream = TcpStream::connect(("127.0.0.1", control_port))
            .await
            .map_err(|error| error.to_string())?;
        control_command(
            &mut stream,
            &format!("AUTHENTICATE {}", hex::encode(cookie)),
        )
        .await?;
        let response = control_command(
            &mut stream,
            &format!("ADD_ONION NEW:BEST Flags=DiscardPK Port=80,127.0.0.1:{target_port}"),
        )
        .await?;
        let service_id = response
            .iter()
            .find_map(|line| line.strip_prefix("250-ServiceID="))
            .ok_or("Tor did not return a ServiceID")?
            .to_string();
        Ok(Arc::new(OnionLease {
            onion: format!("{service_id}.onion"),
            generation,
            _control: stream,
        }))
    }

    pub async fn connect_onion(
        &self,
        onion: &str,
        port: u16,
    ) -> Result<Socks5Stream<TcpStream>, String> {
        if !is_v3_onion(onion) {
            return Err("refusing a destination that is not a valid Tor v3 onion".into());
        }
        let socks_port = self.start().await?;
        timeout(
            Duration::from_secs(20),
            Socks5Stream::connect(("127.0.0.1", socks_port), (onion, port)),
        )
        .await
        .map_err(|_| "Tor connection attempt timed out".to_string())?
        .map_err(|error| format!("Tor connection failed: {error}"))
    }

    pub async fn connect_onion_with_retry(
        &self,
        onion: &str,
        port: u16,
        cancel: &CancellationToken,
    ) -> Result<Socks5Stream<TcpStream>, String> {
        let deadline = Instant::now() + ONION_CONNECT_BUDGET;
        let mut attempt = 0u32;
        // The reason the last attempt failed is carried out of the loop, so there
        // is no path on which a failure is reported without one.
        let failure = loop {
            if cancel.is_cancelled() {
                return Err("cancelled".into());
            }
            attempt += 1;
            match self.connect_onion(onion, port).await {
                Ok(stream) => return Ok(stream),
                Err(error) => {
                    if Instant::now() >= deadline {
                        break error;
                    }
                    // Growing, then capped: a descriptor that is still spreading is
                    // worth asking about again quickly, and one that is not coming
                    // should not be asked about every second for three minutes.
                    let gap =
                        Duration::from_secs(u64::from(attempt).min(ONION_CONNECT_MAX_GAP.as_secs()));
                    tokio::select! {
                        _ = cancel.cancelled() => return Err("cancelled".into()),
                        _ = sleep(gap) => {}
                    }
                }
            }
        };
        Err(format!(
            "the onion service did not answer within {} seconds: {failure}",
            ONION_CONNECT_BUDGET.as_secs()
        ))
    }

    fn find_tor_binary(&self) -> PathBuf {
        if let Ok(value) = std::env::var("NAPSTR_TOR_PATH") {
            return PathBuf::from(value);
        }
        let executable = if cfg!(windows) { "tor.exe" } else { "tor" };
        let platform = if cfg!(target_os = "windows") {
            "windows"
        } else if cfg!(target_os = "macos") {
            "macos"
        } else {
            "linux"
        };
        for candidate in [
            self.resource_dir
                .join("resources")
                .join("tor")
                .join(platform)
                .join("tor")
                .join(executable),
            self.resource_dir
                .join("resources")
                .join("tor")
                .join(platform)
                .join(executable),
            self.resource_dir
                .join("tor")
                .join(platform)
                .join("tor")
                .join(executable),
            self.resource_dir
                .join("tor")
                .join(platform)
                .join(executable),
            self.resource_dir.join("tor").join(executable),
        ] {
            if candidate.is_file() {
                return candidate;
            }
        }
        PathBuf::from(executable)
    }
}

#[cfg(target_os = "windows")]
fn hide_child_process_window(command: &mut Command) {
    // Tor is a console executable. Napstr captures its output through pipes, so
    // it does not need a visible terminal window when launched by the GUI app.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(target_os = "windows"))]
fn hide_child_process_window(_command: &mut Command) {}

/// Ask the operating system to take Tor down when this application goes.
///
/// `kill_on_drop` only ever covers an orderly exit, because it runs when this
/// process is still there to run it. An application that is killed - by the task
/// manager, by a rebuild, by a crash - leaves its Tor behind, running, holding a
/// hidden service and its circuits; the machine then collects one of them per run
/// (measured: 32 of them, 1.9 GB, on the machine this was written for).
///
/// A death signal is set inside the child before it becomes Tor, which is the only
/// moment it can be set: the kernel signals it the instant the parent ends, for any
/// reason at all. macOS has no equivalent, so there the process is still only as
/// safe as the exit path - see `own_child_for_this_application` for the Windows
/// answer and [`TorManager::stop`] for the orderly one.
#[cfg(target_os = "linux")]
fn prepare_child_to_die_with_us(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    let _ = unsafe {
        command.as_std_mut().pre_exec(|| {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        })
    };
}

#[cfg(not(target_os = "linux"))]
fn prepare_child_to_die_with_us(_command: &mut Command) {}

/// The handle that keeps Tor inside a job object belonging to this application.
///
/// Windows has no death signal, so the rule is a job instead: a process in a job
/// with `KILL_ON_JOB_CLOSE` is terminated when the last handle to that job closes,
/// and the last handle is the one returned here. That covers every way this
/// application can stop - the window, the task bar, a rebuild, a crash - because
/// the kernel acts rather than an exit handler.
///
/// `None` is not fatal: it means this Tor is covered only by the exit paths, which
/// is what every platform had before this existed.
#[cfg(windows)]
fn own_child_for_this_application(child: &Child) -> Option<std::os::windows::io::OwnedHandle> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    // SAFETY: every call is a Win32 call with the arguments its signature asks for,
    // every handle is either checked before it is used or owned from the moment it
    // exists, and the limits structure is a plain C struct that is zeroed first.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return None;
        }
        // Owned at once, so a failure below still closes the job rather than leaking
        // the handle for the life of the process.
        let job = OwnedHandle::from_raw_handle(job);
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if configured == 0 {
            return None;
        }
        let process = child.raw_handle()?;
        if AssignProcessToJobObject(job.as_raw_handle(), process) == 0 {
            return None;
        }
        Some(job)
    }
}

fn capture_process_output<R>(stream: R, destination: Arc<RwLock<Vec<String>>>)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let line = line.trim();
            if !line.is_empty() {
                let mut diagnostics = destination.write().await;
                diagnostics.push(line.chars().take(500).collect());
                if diagnostics.len() > 16 {
                    diagnostics.remove(0);
                }
            }
        }
    });
}

fn useful_diagnostic(lines: &[String]) -> String {
    lines
        .iter()
        .rev()
        .find(|line| line.contains("[warn]") && !line.contains("Fixing permissions on directory"))
        .or_else(|| {
            lines.iter().rev().find(|line| {
                line.contains("[err]")
                    && !line.contains("Reading config failed--see warnings above")
                    && !line.contains("set_options(): Bug:")
            })
        })
        .or_else(|| lines.last())
        .cloned()
        .unwrap_or_default()
}

fn bootstrap_progress(lines: &[String]) -> Option<u8> {
    lines.iter().find_map(|line| {
        line.split_whitespace().find_map(|field| {
            field
                .strip_prefix("PROGRESS=")
                .and_then(|value| value.parse::<u8>().ok())
                .filter(|value| *value <= 100)
        })
    })
}

async fn control_command(stream: &mut TcpStream, command: &str) -> Result<Vec<String>, String> {
    stream
        .write_all(format!("{command}\r\n").as_bytes())
        .await
        .map_err(|error| error.to_string())?;
    stream.flush().await.map_err(|error| error.to_string())?;
    let mut reader = BufReader::new(stream);
    let mut lines = Vec::new();
    loop {
        let mut line = String::new();
        let read = reader
            .read_line(&mut line)
            .await
            .map_err(|error| error.to_string())?;
        if read == 0 {
            return Err("Tor control connection closed unexpectedly".into());
        }
        let line = line.trim_end().to_string();
        if line.starts_with("250") {
            let complete =
                line == "250 OK" || (line.starts_with("250 ") && !line.starts_with("250-"));
            lines.push(line);
            if complete {
                return Ok(lines);
            }
        } else if line.len() >= 3 {
            return Err(format!("Tor control error: {line}"));
        }
    }
}

fn listener_port(value: &str) -> Option<u16> {
    value
        .trim()
        .trim_matches('"')
        .rsplit_once(':')
        .and_then(|(_, port)| port.trim_matches('"').parse().ok())
}

fn listener_port_from_control(lines: &[String]) -> Option<u16> {
    lines.iter().find_map(|line| {
        line.strip_prefix("250-net/listeners/socks=")
            .or_else(|| line.strip_prefix("250 net/listeners/socks="))
            .and_then(|listeners| listeners.split_whitespace().find_map(listener_port))
    })
}

#[cfg(target_os = "linux")]
fn prepend_library_path(command: &mut Command, directory: &Path) {
    let value = std::env::var_os("LD_LIBRARY_PATH")
        .map(|existing| format!("{}:{}", directory.display(), existing.to_string_lossy()))
        .unwrap_or_else(|| directory.display().to_string());
    command.env("LD_LIBRARY_PATH", value);
}

#[cfg(target_os = "macos")]
fn prepend_library_path(command: &mut Command, directory: &Path) {
    let value = std::env::var_os("DYLD_LIBRARY_PATH")
        .map(|existing| format!("{}:{}", directory.display(), existing.to_string_lossy()))
        .unwrap_or_else(|| directory.display().to_string());
    command.env("DYLD_LIBRARY_PATH", value);
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn prepend_library_path(_command: &mut Command, _directory: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    /// The whole point of the job object is that a process in it dies when the
    /// application that holds the job dies. A handle that failed to be created, or
    /// a failed assignment, would look exactly like a working fix until somebody
    /// checked Task Manager - so the mechanism itself is what this checks.
    #[cfg(windows)]
    #[tokio::test]
    async fn a_child_in_this_applications_job_dies_when_the_job_is_closed() {
        // A stand-in for Tor: something that will still be there in a minute.
        let mut child = Command::new("cmd")
            .args(["/c", "ping", "-n", "120", "127.0.0.1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("could not start a stand-in child process");
        let job = own_child_for_this_application(&child);
        assert!(job.is_some(), "the child must end up in a job of ours");
        assert!(
            child.try_wait().unwrap().is_none(),
            "the stand-in should still be running, or this proves nothing"
        );
        drop(job);
        let mut gone = false;
        for _ in 0..60 {
            if child.try_wait().unwrap().is_some() {
                gone = true;
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
        if !gone {
            let _ = child.start_kill();
        }
        assert!(gone, "closing the job must kill the process inside it");
    }

    #[test]
    fn accepts_only_v3_onion_hostnames() {
        assert!(is_v3_onion(&format!("{}.onion", "a".repeat(56))));
        assert!(!is_v3_onion("example.com"));
        assert!(!is_v3_onion("short.onion"));
        assert!(!is_v3_onion(&format!("{}.onion.example", "a".repeat(56))));
    }

    #[test]
    fn reads_control_port_bootstrap_progress() {
        assert_eq!(
            bootstrap_progress(&[
                "250-status/bootstrap-phase=NOTICE BOOTSTRAP PROGRESS=75 TAG=enough_dirinfo".into()
            ]),
            Some(75)
        );
        assert_eq!(bootstrap_progress(&["250 OK".into()]), None);
    }

    #[test]
    fn keeps_the_actionable_tor_warning() {
        assert_eq!(
            useful_diagnostic(&[
                "Aug 21 14:33:17 [warn] Failed to lock data directory: another Tor process is running".into(),
                "Aug 21 14:33:22 [err] set_options(): Bug: Acting on config options left us in a broken state. Dying.".into(),
                "Aug 21 14:33:17 [err] Reading config failed--see warnings above.".into(),
            ]),
            "Aug 21 14:33:17 [warn] Failed to lock data directory: another Tor process is running"
        );
    }

    #[test]
    fn reads_tor_selected_listener_ports() {
        assert_eq!(listener_port("PORT=127.0.0.1:49152\n"), Some(49152));
        assert_eq!(
            listener_port_from_control(&[
                "250-net/listeners/socks=\"127.0.0.1:49153\"".into(),
                "250 OK".into(),
            ]),
            Some(49153)
        );
    }

    #[tokio::test]
    #[ignore = "requires a Tor binary and external Tor network access"]
    async fn concurrent_instances_do_not_share_a_tor_data_directory() {
        let directory =
            std::env::temp_dir().join(format!("napstr-tor-concurrent-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).await.unwrap();
        let first = TorManager::new(directory.clone(), directory.clone());
        let second = TorManager::new(directory.clone(), directory.clone());
        let (first_port, second_port) = tokio::join!(first.start(), second.start());
        assert_ne!(first_port.unwrap(), second_port.unwrap());
        first.stop().await;
        second.stop().await;
        let _ = fs::remove_dir_all(directory).await;
    }

    #[tokio::test]
    #[ignore = "requires a Tor binary and external Tor network access"]
    async fn ephemeral_onion_round_trip() {
        let directory =
            std::env::temp_dir().join(format!("napstr-tor-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).await.unwrap();
        let manager = TorManager::new(directory.clone(), directory.clone());
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let lease = manager.create_onion(port).await.unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4];
            stream.read_exact(&mut request).await.unwrap();
            assert_eq!(&request, b"ping");
            stream.write_all(b"pong").await.unwrap();
        });
        let cancel = CancellationToken::new();
        let mut client = manager
            .connect_onion_with_retry(&lease.onion, 80, &cancel)
            .await
            .unwrap();
        client.write_all(b"ping").await.unwrap();
        let mut response = [0u8; 4];
        client.read_exact(&mut response).await.unwrap();
        assert_eq!(&response, b"pong");
        server.await.unwrap();
        // The lease belongs to the process that published it, and stopping that
        // process is exactly what must stop it being offered again.
        assert!(manager.lease_is_current(&lease));
        let held = lease.clone();
        drop(lease);
        manager.stop().await;
        assert!(!manager.lease_is_current(&held));
        let _ = fs::remove_dir_all(directory).await;
    }

    /// An ephemeral onion belongs to one Tor process, and the manager has to be able
    /// to say which one. Everything else about a computer holding a stale lease looks
    /// healthy: it heartbeats, it answers download requests, and the address it hands
    /// out cannot be reached by anybody.
    #[tokio::test]
    async fn a_lease_stops_being_current_when_its_tor_is_replaced() {
        let directory =
            std::env::temp_dir().join(format!("napstr-lease-{}", uuid::Uuid::new_v4()));
        let manager = TorManager::new(directory.clone(), directory.clone());
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let lease = Arc::new(OnionLease {
            onion: format!("{}.onion", "a".repeat(56)),
            generation: manager.generation(),
            _control: stream,
        });
        assert!(manager.lease_is_current(&lease));
        // Whatever took that Tor away - a restart after the machine woke, the health
        // poll, a start that had to be made again - the lease goes with it.
        manager.note_tor_replaced();
        assert!(!manager.lease_is_current(&lease));
    }
}
