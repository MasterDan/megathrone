//! Sidecar process plumbing (sing-box, byedpi).
//!
//! A minimal in-house replacement for the sidecar subset of
//! `tauri-plugin-shell` (which is desktop-only: on Android its process API
//! does not exist, and the CLI does not bundle `externalBin` into an APK at
//! all). Mirrors the plugin's semantics the rest of the codebase grew up
//! around: a synchronous `spawn()` returning an event channel (line-based
//! `Stdout`/`Stderr` chunks, a final `Terminated`) plus a `CommandChild`
//! handle, and an async `output()` collecting everything.
//!
//! Binary resolution per platform:
//! - desktop — next to the current executable (where the bundler puts
//!   `externalBin` sidecars; `cargo test` binaries live in `deps/`, so that
//!   parent is searched too);
//! - Android — inside the APK's `nativeLibraryDir`, where `build.rs` staged
//!   the binaries as fake `lib*.so` jniLibs (Android only extracts and chmods
//!   `+x` files named that way; exec from the data dir is forbidden).

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command as StdCommand, Stdio};
use std::sync::{Arc, RwLock};
use std::thread;

use tauri::async_runtime::{channel, Receiver};

#[cfg(target_os = "android")]
use std::path::Path;
#[cfg(target_os = "android")]
use std::sync::OnceLock;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Payload for the [`CommandEvent::Terminated`] command event.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TerminatedPayload {
    /// Exit code of the process.
    pub code: Option<i32>,
    /// If the process was terminated by a signal, represents that signal.
    pub signal: Option<i32>,
}

/// An event sent to the command event channel while the child runs.
#[derive(Debug, Clone)]
pub enum CommandEvent {
    /// Bytes until a newline (`\n`) from the child's stderr.
    Stderr(Vec<u8>),
    /// Bytes until a newline (`\n`) from the child's stdout.
    Stdout(Vec<u8>),
    /// An error happened waiting for the command to finish.
    Error(String),
    /// The command process terminated.
    Terminated(TerminatedPayload),
}

/// Describes the result of a process after it has terminated.
#[derive(Debug)]
pub struct ExitStatus {
    code: Option<i32>,
}

impl ExitStatus {
    /// True when the process exited with status zero. Death by signal is
    /// not a success.
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }
}

/// The output of a finished process.
#[derive(Debug)]
pub struct Output {
    /// The status (exit code) of the process.
    pub status: ExitStatus,
    /// The data that the process wrote to stdout.
    pub stdout: Vec<u8>,
    /// The data that the process wrote to stderr.
    pub stderr: Vec<u8>,
}

/// The sidecar binary names the app knows how to spawn.
const SIDECARS: &[(&str, &str)] = &[("sing-box", "libsingbox.so"), ("byedpi", "libciadpi.so")];

/// Resolves one of the bundled sidecar binaries (`sing-box` / `byedpi`) to a
/// spawnable path on the current platform.
pub fn sidecar(name: &str) -> Result<Command, String> {
    let file_name = binary_file_name(name)?;
    Ok(Command::new(sidecar_path(name, &file_name)?))
}

/// The platform file name of a sidecar: as-is on desktop (the bundler keeps
/// the `externalBin` name), `lib<name>.so` on Android.
fn binary_file_name(name: &str) -> Result<String, String> {
    #[cfg(target_os = "android")]
    {
        let (_, so_name) = SIDECARS
            .iter()
            .find(|(sidecar, _)| *sidecar == name)
            .ok_or_else(|| format!("unknown sidecar `{name}`"))?;
        Ok((*so_name).to_string())
    }
    #[cfg(not(target_os = "android"))]
    {
        if SIDECARS.iter().any(|(sidecar, _)| *sidecar == name) {
            #[cfg(windows)]
            return Ok(format!("{name}.exe"));
            #[cfg(not(windows))]
            return Ok(name.to_string());
        }
        Err(format!("unknown sidecar `{name}`"))
    }
}

/// Where the sidecar binary with `file_name` lives.
fn sidecar_path(name: &str, file_name: &str) -> Result<PathBuf, String> {
    #[cfg(target_os = "android")]
    {
        let _ = name;
        Ok(native_library_dir()?.join(file_name))
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = name;
        let exe = std::env::current_exe()
            .map_err(|error| format!("failed to locate the current executable: {error}"))?;
        let exe_dir = exe
            .parent()
            .ok_or_else(|| "the current executable has no parent directory".to_string())?;
        // cargo test binaries live in target/<profile>/deps — sidecars sit
        // one level up, next to the regular binaries
        let base_dir = if exe_dir.ends_with("deps") {
            exe_dir.parent().unwrap_or(exe_dir)
        } else {
            exe_dir
        };
        Ok(base_dir.join(file_name))
    }
}

/// The APK's extracted native library directory — the only place Android
/// lets an app keep executable files. Resolved once over JNI (the JVM and
/// the Activity are globally registered by the windowing backend).
#[cfg(target_os = "android")]
fn native_library_dir() -> Result<&'static Path, String> {
    static DIR: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    DIR.get_or_init(resolve_native_library_dir)
        .as_deref()
        .map_err(Clone::clone)
}

#[cfg(target_os = "android")]
fn resolve_native_library_dir() -> Result<PathBuf, String> {
    use jni::objects::{JObject, JString};

    let context = ndk_context::android_context();
    let vm = unsafe { jni::JavaVM::from_raw(context.vm() as *mut _) }
        .map_err(|error| format!("failed to attach to the JVM: {error}"))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|error| format!("failed to attach the current thread to the JVM: {error}"))?;
    let activity = unsafe { JObject::from_raw(context.context() as jni::sys::jobject) };
    let info = env
        .call_method(
            &activity,
            "getApplicationInfo",
            "()Landroid/content/pm/ApplicationInfo;",
            &[],
        )
        .map_err(|error| format!("getApplicationInfo failed: {error}"))?
        .l()
        .map_err(|error| format!("getApplicationInfo failed: {error}"))?;
    let dir = env
        .get_field(&info, "nativeLibraryDir", "Ljava/lang/String;")
        .map_err(|error| format!("reading nativeLibraryDir failed: {error}"))?
        .l()
        .map_err(|error| format!("reading nativeLibraryDir failed: {error}"))?;
    let dir: String = env
        .get_string(&JString::from(dir))
        .map_err(|error| format!("reading nativeLibraryDir failed: {error}"))?
        .into();
    Ok(PathBuf::from(dir))
}

/// The type to spawn sidecar commands.
#[derive(Debug)]
pub struct Command {
    program: PathBuf,
    args: Vec<String>,
}

impl Command {
    pub fn new(program: PathBuf) -> Self {
        Self { program, args: Vec::new() }
    }

    /// Appends arguments to the command.
    #[must_use]
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.args.extend(args.into_iter().map(|arg| arg.as_ref().to_string()));
        self
    }

    /// Spawns the sidecar. Returns immediately: output (including the final
    /// termination event) arrives over the channel, process control lives on
    /// the [`CommandChild`].
    pub fn spawn(self) -> Result<(Receiver<CommandEvent>, CommandChild), String> {
        let mut command = StdCommand::new(&self.program);
        command
            .args(&self.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        command.creation_flags(CREATE_NO_WINDOW);

        let child = shared_child::SharedChild::spawn(&mut command).map_err(|error| {
            format!("failed to spawn {}: {error}", self.program.display())
        })?;
        let child = Arc::new(child);

        let stdout = child.take_stdout();
        let stderr = child.take_stderr();

        let (tx, rx) = channel(64);
        // readers hold the read side while pumping; the waiter takes the
        // write side first, so `Terminated` is only delivered after both
        // pipes hit EOF — the plugin ordering the consumers rely on
        let guard = Arc::new(RwLock::new(()));

        if let Some(stdout) = stdout {
            spawn_pipe_reader(tx.clone(), guard.clone(), stdout, CommandEvent::Stdout);
        }
        if let Some(stderr) = stderr {
            spawn_pipe_reader(tx.clone(), guard.clone(), stderr, CommandEvent::Stderr);
        }

        let waiter_child = child.clone();
        let waiter_tx = tx.clone();
        thread::spawn(move || {
            let event = match waiter_child.wait() {
                Ok(status) => CommandEvent::Terminated(TerminatedPayload {
                    code: status.code(),
                    #[cfg(unix)]
                    signal: {
                        use std::os::unix::process::ExitStatusExt;
                        status.signal()
                    },
                    #[cfg(windows)]
                    signal: None,
                }),
                Err(error) => CommandEvent::Error(error.to_string()),
            };
            let _lock = guard.write().unwrap();
            let _ = waiter_tx.blocking_send(event);
        });

        Ok((rx, CommandChild { child }))
    }

    /// Runs the sidecar to completion, collecting its output.
    pub async fn output(self) -> Result<Output, String> {
        let (mut rx, _child) = self.spawn()?;

        let mut code = None;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        while let Some(event) = rx.recv().await {
            match event {
                CommandEvent::Stdout(chunk) => stdout.extend_from_slice(&chunk),
                CommandEvent::Stderr(chunk) => stderr.extend_from_slice(&chunk),
                CommandEvent::Terminated(payload) => code = payload.code,
                CommandEvent::Error(error) => return Err(error),
            }
        }
        Ok(Output { status: ExitStatus { code }, stdout, stderr })
    }
}

fn spawn_pipe_reader<R: std::io::Read + Send + 'static>(
    tx: tauri::async_runtime::Sender<CommandEvent>,
    guard: Arc<RwLock<()>>,
    pipe: R,
    wrap: fn(Vec<u8>) -> CommandEvent,
) {
    let _ = thread::spawn(move || {
        let mut reader = BufReader::new(pipe);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let _lock = guard.read().unwrap();
                    if tx.blocking_send(wrap(buf.clone())).is_err() {
                        break;
                    }
                }
            }
        }
    });
}

/// A spawned sidecar child: kill and pid surface.
#[derive(Debug)]
pub struct CommandChild {
    child: Arc<shared_child::SharedChild>,
}

impl CommandChild {
    /// Sends SIGKILL / TerminateProcess to the child.
    pub fn kill(self) -> Result<(), String> {
        self.child.kill().map_err(|error| error.to_string())
    }

    /// Returns the process pid.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }
}
