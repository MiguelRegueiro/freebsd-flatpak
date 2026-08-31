use super::chroot_instance::{OwnedMount, SandboxExecutionContext};
use super::process_supervision::ProcessReaper;
use super::stale_sandbox_recovery::{run_command, unmount_mountpoint};
use crate::installation as state;
use crate::installation::installation_paths::Installation;
use crate::{secure_launch, secure_mount};
use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use std::fs;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

const MAGIC: u32 = 0x4653_4250; // "FSBP"
const VERSION: u16 = 1;
const HEADER_SIZE: usize = 20;
const MAX_PAYLOAD: usize = 4096;
const MAX_FDS: usize = 32;
const PING: u16 = 1;
const PONG: u16 = 2;
const FD_TEST: u16 = 3;
const FD_OK: u16 = 4;
const LIFECYCLE_TEST_START: u16 = 5;
const LIFECYCLE_TEST_ACCEPTED: u16 = 6;
const LIFECYCLE_TEST_EXITED: u16 = 7;
const SPAWN: u16 = 8;
const SPAWN_ACCEPTED: u16 = 9;
const SPAWN_EXITED: u16 = 10;
const SPAWN_STARTED: u16 = 12;
const SPAWN_TERMINATE: u16 = 11;
const MAX_TARGET_FD: i32 = 65_535;
const SPAWN_WATCH_BUS: u32 = 1 << 4;
const SPAWN_EXPOSE_PIDS: u32 = 1 << 5;
const SPAWN_NOTIFY_START: u32 = 1 << 6;
const SPAWN_SUPPORTED_FLAGS: u32 = 0x1f | SPAWN_EXPOSE_PIDS | SPAWN_NOTIFY_START;
const NESTED_MOUNT_STAGING: &str = ".freebsd-flatpak-mount-sources";
static NEXT_NESTED_ROOT: AtomicU64 = AtomicU64::new(1);

struct SpawnRequest {
    cwd: Option<Vec<u8>>,
    flags: u32,
    argv: Vec<Vec<u8>>,
    caller_pid: u32,
    environment: Vec<(Vec<u8>, Vec<u8>)>,
    mappings: Vec<(i32, OwnedFd)>,
}

#[derive(Default)]
struct BrokerConnections {
    stopping: AtomicBool,
    retained_fds: Mutex<Vec<i32>>,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

impl BrokerConnections {
    fn retain(&self, fd: i32) {
        self.retained_fds
            .lock()
            .expect("broker fds poisoned")
            .push(fd);
    }
    fn release(&self, fd: i32) {
        let mut fds = self.retained_fds.lock().expect("broker fds poisoned");
        if let Some(index) = fds.iter().position(|candidate| *candidate == fd) {
            fds.swap_remove(index);
        }
    }
    fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        for fd in self
            .retained_fds
            .lock()
            .expect("broker fds poisoned")
            .iter()
        {
            unsafe {
                let _ = libc::shutdown(*fd, libc::SHUT_RDWR);
            }
        }
    }
    fn join_workers(&self) {
        let workers = std::mem::take(&mut *self.workers.lock().expect("broker workers poisoned"));
        for worker in workers {
            let _ = worker.join();
        }
    }
}

fn frame(message: u16, request: u32, payload: &[u8], fds: u32) -> [u8; HEADER_SIZE] {
    let mut out = [0; HEADER_SIZE];
    out[..4].copy_from_slice(&MAGIC.to_be_bytes());
    out[4..6].copy_from_slice(&VERSION.to_be_bytes());
    out[6..8].copy_from_slice(&message.to_be_bytes());
    out[8..12].copy_from_slice(&request.to_be_bytes());
    out[12..16].copy_from_slice(&(payload.len() as u32).to_be_bytes());
    out[16..20].copy_from_slice(&fds.to_be_bytes());
    out
}
fn parse_frame(packet: &[u8]) -> Option<(u16, u32, usize, u32)> {
    if packet.len() < HEADER_SIZE {
        return None;
    }
    let magic = u32::from_be_bytes(packet[..4].try_into().ok()?);
    let version = u16::from_be_bytes(packet[4..6].try_into().ok()?);
    let kind = u16::from_be_bytes(packet[6..8].try_into().ok()?);
    let request = u32::from_be_bytes(packet[8..12].try_into().ok()?);
    let length = u32::from_be_bytes(packet[12..16].try_into().ok()?) as usize;
    let fds = u32::from_be_bytes(packet[16..20].try_into().ok()?);
    if magic != MAGIC
        || version != VERSION
        || length > MAX_PAYLOAD
        || packet.len() != HEADER_SIZE + length
    {
        return None;
    }
    Some((kind, request, length, fds))
}

/// Host-only per-chroot transport. Its path is deliberately under the runner's
/// runtime root, never under the chroot and never included in its mount plan.
pub(super) struct SpawnBroker {
    #[allow(dead_code)]
    context: Arc<SandboxExecutionContext>,
    #[allow(dead_code)]
    supervisor: Arc<ProcessReaper>,
    path: PathBuf,
    listener: OwnedFd,
    connections: Arc<BrokerConnections>,
    worker: Option<JoinHandle<()>>,
}
impl SpawnBroker {
    pub(super) fn bind(
        paths: &Installation,
        context: Arc<SandboxExecutionContext>,
        supervisor: Arc<ProcessReaper>,
    ) -> Result<Self> {
        let path = broker_path(paths, &context.root)?;
        fs::create_dir_all(path.parent().expect("broker parent"))?;
        let _ = fs::remove_file(&path);
        let fd = socket_seqpacket()?;
        bind_socket(fd.as_raw_fd(), &path)?;
        let listener = fd;
        let accept_fd = unsafe { libc::dup(listener.as_raw_fd()) };
        if accept_fd < 0 {
            bail!(
                "duplicate spawn broker listener: {}",
                std::io::Error::last_os_error()
            );
        }
        let (uid, gid) = (context.uid, context.gid);
        let accept_context = context.clone();
        let accept_supervisor = supervisor.clone();
        let connections = Arc::new(BrokerConnections::default());
        let accept_connections = connections.clone();
        let worker = thread::spawn(move || unsafe {
            broker_loop(
                OwnedFd::from_raw_fd(accept_fd),
                uid,
                gid,
                accept_context,
                accept_supervisor,
                accept_connections,
            )
        });
        Ok(Self {
            context,
            supervisor,
            path,
            listener,
            connections,
            worker: Some(worker),
        })
    }
    #[allow(dead_code)]
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
    #[cfg(test)]
    fn context(&self) -> &Arc<SandboxExecutionContext> {
        &self.context
    }
    #[cfg(test)]
    #[allow(dead_code)]
    fn supervisor(&self) -> &Arc<ProcessReaper> {
        &self.supervisor
    }
}
impl Drop for SpawnBroker {
    fn drop(&mut self) {
        self.connections.stop();
        let _ = unsafe { libc::shutdown(self.listener.as_raw_fd(), libc::SHUT_RDWR) };
        wake_broker(&self.path);
        let _ = fs::remove_file(&self.path);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.connections.join_workers();
    }
}

pub(super) fn broker_path(paths: &Installation, root: &Path) -> Result<PathBuf> {
    let chroots = paths.chroots();
    let relative = root
        .strip_prefix(&chroots)
        .context("broker root is outside chroots")?;
    let mut parts = relative.components();
    let _app = parts
        .next()
        .context("broker root has no app id")?
        .as_os_str();
    let instance = parts
        .next()
        .context("broker root has no instance id")?
        .as_os_str();
    if parts.next().is_some() || instance.is_empty() {
        bail!("broker root is not an instance root");
    }
    Ok(paths.spawn_brokers().join(instance).with_extension("sock"))
}
fn socket_seqpacket() -> Result<OwnedFd> {
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        bail!(
            "create spawn broker socket: {}",
            std::io::Error::last_os_error()
        )
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}
fn bind_socket(fd: i32, path: &Path) -> Result<()> {
    let bytes = path.as_os_str().as_bytes();
    if bytes.len() + 1 > 108 {
        bail!("spawn broker path too long")
    };
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as _;
    for (i, b) in bytes.iter().enumerate() {
        addr.sun_path[i] = *b as _;
    }
    let len = (std::mem::size_of::<libc::sa_family_t>() + bytes.len() + 1) as _;
    if unsafe { libc::bind(fd, (&addr as *const libc::sockaddr_un).cast(), len) } < 0 {
        bail!("bind spawn broker: {}", std::io::Error::last_os_error())
    };
    if unsafe { libc::listen(fd, 8) } < 0 {
        bail!("listen spawn broker: {}", std::io::Error::last_os_error())
    };
    Ok(())
}
fn wake_broker(path: &Path) {
    let Ok(fd) = socket_seqpacket() else {
        return;
    };
    if bind_socket_connect(fd.as_raw_fd(), path) == 0 {
        let stop = frame(0, 0, &[], 0);
        unsafe {
            let _ = libc::send(fd.as_raw_fd(), stop.as_ptr().cast(), stop.len(), 0);
        }
    }
}
fn bind_socket_connect(fd: i32, path: &Path) -> i32 {
    let bytes = path.as_os_str().as_bytes();
    if bytes.len() + 1 > 108 {
        return -1;
    }
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as _;
    for (i, b) in bytes.iter().enumerate() {
        addr.sun_path[i] = *b as _;
    }
    unsafe {
        libc::connect(
            fd,
            (&addr as *const libc::sockaddr_un).cast(),
            (std::mem::size_of::<libc::sa_family_t>() + bytes.len() + 1) as _,
        )
    }
}

unsafe fn receive_packet(fd: i32) -> Option<(Vec<u8>, Vec<OwnedFd>)> {
    let mut packet = vec![0u8; HEADER_SIZE + MAX_PAYLOAD];
    let mut control = vec![0u8; control_space(MAX_FDS)];
    let mut iov = libc::iovec {
        iov_base: packet.as_mut_ptr().cast(),
        iov_len: packet.len(),
    };
    let mut message: libc::msghdr = std::mem::zeroed();
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = control.len() as _;
    let count = libc::recvmsg(fd, &mut message, 0);
    if count <= 0 {
        return None;
    }
    if message.msg_flags & (libc::MSG_TRUNC | libc::MSG_CTRUNC) != 0 {
        close_control_fds(&message);
        return None;
    }
    packet.truncate(count as usize);
    let (_, _, _, expected) = match parse_frame(&packet) {
        Some(frame) => frame,
        None => {
            close_control_fds(&message);
            return None;
        }
    };
    let raw_descriptors = match control_fds(&message) {
        Some(descriptors) => descriptors,
        None => {
            close_control_fds(&message);
            return None;
        }
    };
    if raw_descriptors.len() != expected as usize {
        close_fds(&raw_descriptors);
        return None;
    }
    let mut descriptors = Vec::with_capacity(raw_descriptors.len());
    for raw in raw_descriptors {
        if raw < 0 || libc::fcntl(raw, libc::F_SETFD, libc::FD_CLOEXEC) < 0 {
            if raw >= 0 {
                libc::close(raw);
            }
            drop(descriptors);
            return None;
        }
        descriptors.push(OwnedFd::from_raw_fd(raw));
    }
    Some((packet, descriptors))
}

fn control_space(fd_count: usize) -> usize {
    unsafe { libc::CMSG_SPACE((fd_count * std::mem::size_of::<i32>()) as _) as usize }
}

unsafe fn control_fds(message: &libc::msghdr) -> Option<Vec<i32>> {
    let mut descriptors = Vec::new();
    let mut current = libc::CMSG_FIRSTHDR(message);
    while !current.is_null() {
        if (*current).cmsg_level != libc::SOL_SOCKET
            || (*current).cmsg_type != libc::SCM_RIGHTS
            || (*current).cmsg_len < libc::CMSG_LEN(0)
        {
            return None;
        }
        let bytes = ((*current).cmsg_len - libc::CMSG_LEN(0)) as usize;
        if !bytes.is_multiple_of(std::mem::size_of::<i32>())
            || descriptors.len() + bytes / std::mem::size_of::<i32>() > MAX_FDS
        {
            return None;
        }
        let data = libc::CMSG_DATA(current).cast::<i32>();
        for index in 0..bytes / std::mem::size_of::<i32>() {
            descriptors.push(*data.add(index));
        }
        current = libc::CMSG_NXTHDR(message, current);
    }
    Some(descriptors)
}

unsafe fn close_control_fds(message: &libc::msghdr) {
    let mut current = libc::CMSG_FIRSTHDR(message);
    while !current.is_null() {
        if (*current).cmsg_level == libc::SOL_SOCKET
            && (*current).cmsg_type == libc::SCM_RIGHTS
            && (*current).cmsg_len >= libc::CMSG_LEN(0)
        {
            let bytes = ((*current).cmsg_len - libc::CMSG_LEN(0)) as usize;
            if bytes.is_multiple_of(std::mem::size_of::<i32>()) {
                let data = libc::CMSG_DATA(current).cast::<i32>();
                for index in 0..bytes / std::mem::size_of::<i32>() {
                    let descriptor = *data.add(index);
                    if descriptor >= 0 {
                        libc::close(descriptor);
                    }
                }
            }
        }
        current = libc::CMSG_NXTHDR(message, current);
    }
}

unsafe fn close_fds(descriptors: &[i32]) {
    for descriptor in descriptors {
        if *descriptor >= 0 {
            libc::close(*descriptor);
        }
    }
}
unsafe fn peer_matches(fd: i32, expected_uid: u32, expected_gid: u32) -> bool {
    unsafe extern "C" {
        fn getpeereid(
            fd: libc::c_int,
            euid: *mut libc::uid_t,
            egid: *mut libc::gid_t,
        ) -> libc::c_int;
    }
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    getpeereid(fd, &mut uid, &mut gid) == 0
        && uid as u32 == expected_uid
        && gid as u32 == expected_gid
}
unsafe fn broker_loop(
    listener: OwnedFd,
    expected_uid: u32,
    expected_gid: u32,
    context: Arc<SandboxExecutionContext>,
    supervisor: Arc<ProcessReaper>,
    connections: Arc<BrokerConnections>,
) {
    loop {
        let fd = libc::accept4(
            listener.as_raw_fd(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            libc::SOCK_CLOEXEC,
        );
        if fd < 0 {
            break;
        }
        if connections.stopping.load(Ordering::SeqCst) {
            libc::close(fd);
            break;
        }
        if !peer_matches(fd, expected_uid, expected_gid) {
            libc::close(fd);
            continue;
        }
        let connections = connections.clone();
        let context = context.clone();
        let supervisor = supervisor.clone();
        connections.retain(fd);
        let worker_connections = connections.clone();
        let worker = thread::spawn(move || unsafe {
            handle_connection(
                OwnedFd::from_raw_fd(fd),
                context,
                supervisor,
                worker_connections,
            );
        });
        connections
            .workers
            .lock()
            .expect("broker workers poisoned")
            .push(worker);
    }
}

unsafe fn send_frame(fd: i32, header: &[u8], payload: &[u8]) -> bool {
    let mut packet = Vec::with_capacity(header.len() + payload.len());
    packet.extend_from_slice(header);
    packet.extend_from_slice(payload);
    loop {
        let sent = libc::send(fd, packet.as_ptr().cast(), packet.len(), 0);
        if sent == packet.len() as isize {
            return true;
        }
        if sent < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        return false;
    }
}

unsafe fn watch_bus_termination_requested(fd: i32, request: u32) -> bool {
    let mut state = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    if unsafe { libc::poll(&mut state, 1, 0) } <= 0 {
        return false;
    }
    if state.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
        return true;
    }
    if state.revents & libc::POLLIN == 0 {
        return false;
    }
    let Some((packet, fds)) = receive_packet(fd) else {
        return true;
    };
    matches!(
        parse_frame(&packet),
        Some((SPAWN_TERMINATE, message_request, 0, 0)) if message_request == request
    ) && fds.is_empty()
}

unsafe fn spawn_termination_signal(
    fd: i32,
    request: u32,
    watch_bus: bool,
    broker_stopping: bool,
) -> Option<libc::c_int> {
    if broker_stopping {
        Some(libc::SIGTERM)
    } else if watch_bus && watch_bus_termination_requested(fd, request) {
        Some(libc::SIGINT)
    } else {
        None
    }
}

fn decode_u32(payload: &[u8], offset: &mut usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    let value = u32::from_be_bytes(payload.get(*offset..end)?.try_into().ok()?);
    *offset = end;
    Some(value)
}

fn decode_bytes(payload: &[u8], offset: &mut usize, allow_empty: bool) -> Option<Vec<u8>> {
    let length = usize::try_from(decode_u32(payload, offset)?).ok()?;
    let end = offset.checked_add(length)?;
    let value = payload.get(*offset..end)?;
    if (!allow_empty && value.is_empty()) || value.contains(&0) {
        return None;
    }
    *offset = end;
    Some(value.to_vec())
}

fn parse_spawn(
    payload: &[u8],
    fds: Vec<OwnedFd>,
) -> std::result::Result<SpawnRequest, &'static str> {
    let mut offset = 0;
    let cwd = decode_bytes(payload, &mut offset, true).ok_or("invalid cwd")?;
    let argc = usize::try_from(decode_u32(payload, &mut offset).ok_or("missing argument count")?)
        .map_err(|_| "invalid argument count")?;
    let envc =
        usize::try_from(decode_u32(payload, &mut offset).ok_or("missing environment count")?)
            .map_err(|_| "invalid environment count")?;
    let mapping_count =
        usize::try_from(decode_u32(payload, &mut offset).ok_or("missing fd mapping count")?)
            .map_err(|_| "invalid fd mapping count")?;
    let flags = decode_u32(payload, &mut offset).ok_or("missing flags")?;
    if argc == 0 || argc > 256 {
        return Err("invalid argument count");
    }
    if envc > 256 {
        return Err("invalid environment count");
    }
    if mapping_count != fds.len() {
        return Err("fd mapping count mismatch");
    }
    let mut argv = Vec::with_capacity(argc);
    for _ in 0..argc {
        argv.push(decode_bytes(payload, &mut offset, false).ok_or("invalid argument")?);
    }
    let mut environment = Vec::with_capacity(envc);
    for _ in 0..envc {
        let key = decode_bytes(payload, &mut offset, false).ok_or("invalid environment key")?;
        if key.contains(&b'=') {
            return Err("invalid environment key");
        }
        let value = decode_bytes(payload, &mut offset, true).ok_or("invalid environment value")?;
        environment.push((key, value));
    }
    let mut targets = HashSet::with_capacity(mapping_count);
    let mut mappings = Vec::with_capacity(mapping_count);
    for source in fds {
        let target = i32::try_from(decode_u32(payload, &mut offset).ok_or("missing fd target")?)
            .map_err(|_| "invalid fd target")?;
        if !(0..=MAX_TARGET_FD).contains(&target) {
            return Err("invalid fd target");
        }
        if !targets.insert(target) {
            return Err("duplicate fd target");
        }
        mappings.push((target, source));
    }
    let caller_pid = decode_u32(payload, &mut offset).ok_or("missing caller process id")?;
    if offset != payload.len() {
        return Err("trailing payload bytes");
    }
    Ok(SpawnRequest {
        flags,
        cwd: (!cwd.is_empty()).then_some(cwd),
        caller_pid,
        argv,
        environment,
        mappings,
    })
}

fn merge_environment(
    inherited: &[(String, String)],
    additions: &[(Vec<u8>, Vec<u8>)],
) -> Option<Vec<(String, String)>> {
    let mut environment = inherited.to_vec();
    for (key, value) in additions {
        let key = String::from_utf8(key.clone()).ok()?;
        let value = String::from_utf8(value.clone()).ok()?;
        if let Some(existing) = environment.iter_mut().find(|(name, _)| *name == key) {
            existing.1 = value;
        } else {
            environment.push((key, value));
        }
    }
    Some(environment)
}

struct SpawnedProcess {
    child: std::process::Child,
    started: Option<OwnedFd>,
    _nested_root: Option<NestedSandboxRoot>,
}

struct NestedSandboxRoot {
    root: PathBuf,
    run_record: PathBuf,
    mounts: Vec<OwnedMount>,
}

impl NestedSandboxRoot {
    fn create(context: &SandboxExecutionContext) -> Result<Self> {
        let app_root = context
            .root
            .parent()
            .context("sandbox root has no application directory")?;
        let parent = context
            .root
            .file_name()
            .context("sandbox root has no instance name")?
            .to_string_lossy();
        let sequence = NEXT_NESTED_ROOT.fetch_add(1, Ordering::Relaxed);
        let instance_id = format!("{parent}-nested-{}-{sequence}", std::process::id());
        let root = app_root.join(&instance_id);
        let run_record = state::write_nested_run_record(
            &context.paths,
            &context.app_id,
            &instance_id,
            &root,
            &context.root,
            std::process::id(),
        )
        .context("publish nested sandbox ownership")?;
        if let Err(error) = fs::create_dir(&root) {
            let _ = state::remove_run_record(&run_record);
            return Err(error)
                .with_context(|| format!("create nested sandbox root {}", root.display()));
        }

        let mut nested = Self {
            root,
            run_record,
            mounts: Vec::new(),
        };
        if let Err(error) = nested.populate(context) {
            nested.cleanup();
            return Err(error);
        }
        Ok(nested)
    }

    fn populate(&mut self, context: &SandboxExecutionContext) -> Result<()> {
        copy_unmounted_tree(&context.root, &self.root, &context.mounts)?;
        let info = fs::read_to_string(context.root.join(".flatpak-info"))
            .context("read parent Flatpak instance metadata")?;
        fs::write(
            self.root.join(".flatpak-info"),
            restricted_flatpak_info(&info),
        )
        .context("write nested Flatpak instance metadata")?;

        let staging = self.root.join(NESTED_MOUNT_STAGING);
        fs::create_dir_all(&staging)
            .with_context(|| format!("create nested mount staging {}", staging.display()))?;
        let root_identity = identity(&self.root)?;
        run_command(
            secure_mount::tmpfs_command(
                &self.root,
                root_identity,
                Path::new(NESTED_MOUNT_STAGING),
                "mode=0700",
            )?,
            "mount nested Spawn staging tmpfs",
        )?;
        self.mounts.push(OwnedMount {
            path: staging,
            read_only: false,
        });

        let mut mounts = context.nested_mounts.clone();
        mounts.sort_by_key(|mount| mount.path.components().count());
        for (index, mount) in mounts.into_iter().enumerate() {
            let target = mount.path.strip_prefix(&context.root).with_context(|| {
                format!(
                    "parent mount is outside sandbox root: {}",
                    mount.path.display()
                )
            })?;
            if target.as_os_str().is_empty() {
                bail!("nested Spawn cannot clone a mount over the sandbox root");
            }
            if is_internal_mount_staging(target) {
                continue;
            }
            let source_identity = identity(&mount.path)?;
            let stage_relative = Path::new(NESTED_MOUNT_STAGING).join(index.to_string());
            run_command(
                secure_mount::nullfs_command(
                    &self.root,
                    root_identity,
                    &mount.path,
                    Some(source_identity),
                    &stage_relative,
                    mount.read_only,
                )?,
                "stage nested Spawn mount",
            )?;
            self.mounts.push(OwnedMount {
                path: self.root.join(&stage_relative),
                read_only: mount.read_only,
            });
            run_command(
                secure_mount::nullfs_command(
                    &self.root,
                    root_identity,
                    &self.root.join(&stage_relative),
                    None,
                    target,
                    mount.read_only,
                )?,
                "clone nested Spawn mount",
            )?;
            self.mounts.push(OwnedMount {
                path: self.root.join(target),
                read_only: mount.read_only,
            });
        }
        Ok(())
    }

    fn cleanup(&mut self) {
        let mut failed = false;
        for mount in self.mounts.drain(..).rev() {
            if let Err(error) =
                unmount_mountpoint(&mount.path, mount.read_only, "umount nested Spawn mount")
            {
                failed = true;
                eprintln!(
                    "warning: nested Spawn mount cleanup failed for {}: {error:#}",
                    mount.path.display()
                );
            }
        }
        if failed {
            return;
        }
        match fs::remove_dir_all(&self.root) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                eprintln!(
                    "warning: nested Spawn root cleanup failed for {}: {error}",
                    self.root.display()
                );
                return;
            }
        }
        if let Err(error) = state::remove_run_record(&self.run_record) {
            eprintln!(
                "warning: nested Spawn ownership cleanup failed for {}: {error:#}",
                self.run_record.display()
            );
        }
    }
}

impl Drop for NestedSandboxRoot {
    fn drop(&mut self) {
        self.cleanup();
    }
}

fn identity(path: &Path) -> Result<(u64, u64)> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("read filesystem identity for {}", path.display()))?;
    Ok((metadata.dev(), metadata.ino()))
}

fn is_internal_mount_staging(target: &Path) -> bool {
    target.starts_with(Path::new(NESTED_MOUNT_STAGING))
}

fn copy_unmounted_tree(source: &Path, target: &Path, mounts: &[OwnedMount]) -> Result<()> {
    for entry in fs::read_dir(source).with_context(|| format!("read {}", source.display()))? {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if entry.file_name() == ".flatpak-info" {
            continue;
        }
        let is_mountpoint = mounts.iter().any(|mount| mount.path == source_path);
        let metadata = fs::symlink_metadata(&source_path)
            .with_context(|| format!("inspect {}", source_path.display()))?;
        if metadata.file_type().is_symlink() {
            symlink(fs::read_link(&source_path)?, &target_path)
                .with_context(|| format!("copy symlink {}", source_path.display()))?;
        } else if metadata.is_dir() {
            fs::create_dir(&target_path)
                .with_context(|| format!("create {}", target_path.display()))?;
            fs::set_permissions(&target_path, fs::Permissions::from_mode(metadata.mode()))?;
            if !is_mountpoint {
                copy_unmounted_tree(&source_path, &target_path, mounts)?;
            }
        } else if metadata.is_file() {
            if is_mountpoint {
                fs::File::create(&target_path)
                    .with_context(|| format!("create {}", target_path.display()))?;
            } else {
                fs::copy(&source_path, &target_path)
                    .with_context(|| format!("copy {}", source_path.display()))?;
            }
            fs::set_permissions(&target_path, fs::Permissions::from_mode(metadata.mode()))?;
        }
    }
    Ok(())
}

fn restricted_flatpak_info(info: &str) -> String {
    if info.lines().any(|line| line.trim() == "sandbox=true") {
        return info.to_string();
    }
    let mut output = String::with_capacity(info.len() + 13);
    let mut inserted = false;
    for line in info.lines() {
        output.push_str(line);
        output.push('\n');
        if !inserted && line.trim() == "[Instance]" {
            output.push_str("sandbox=true\n");
            inserted = true;
        }
    }
    if !inserted {
        output.push_str("[Instance]\nsandbox=true\n");
    }
    output
}

fn spawn_flags_supported(flags: u32) -> bool {
    flags & !SPAWN_SUPPORTED_FLAGS == 0
}

fn readiness_pipe(minimum_fd: i32) -> Result<(OwnedFd, OwnedFd)> {
    let mut fds = [-1; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(std::io::Error::last_os_error()).context("create Spawn readiness pipe");
    }
    let read = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    let write = unsafe { OwnedFd::from_raw_fd(fds[1]) };
    let duplicate = unsafe { libc::fcntl(write.as_raw_fd(), libc::F_DUPFD_CLOEXEC, minimum_fd) };
    if duplicate < 0 {
        return Err(std::io::Error::last_os_error()).context("duplicate Spawn readiness pipe");
    }
    Ok((read, unsafe { OwnedFd::from_raw_fd(duplicate) }))
}

fn spawn_in_existing_sandbox(
    context: &SandboxExecutionContext,
    request: SpawnRequest,
) -> Result<SpawnedProcess> {
    if !spawn_flags_supported(request.flags) {
        bail!("unsupported nested Spawn flags");
    }
    if request
        .cwd
        .as_deref()
        .is_some_and(|directory| !directory.starts_with(b"/"))
    {
        bail!("Spawn cwd must be absolute");
    }
    let nested_sandbox = request.flags & 0x04 != 0;
    let no_network = request.flags & 0x08 != 0;
    let notify_start = request.flags & SPAWN_NOTIFY_START != 0;
    let nested_root = nested_sandbox
        .then(|| NestedSandboxRoot::create(context))
        .transpose()?;
    let launch_root = nested_root
        .as_ref()
        .map_or(context.root.as_path(), |nested| nested.root.as_path());
    let argv = request
        .argv
        .into_iter()
        .map(std::ffi::OsString::from_vec)
        .collect::<Vec<_>>();
    let cwd = request.cwd.map(std::ffi::OsString::from_vec);
    let inherited = if request.flags & 0x01 == 0 {
        context.environment.as_slice()
    } else {
        &[]
    };
    let environment = merge_environment(inherited, &request.environment)
        .context("Spawn environment is not UTF-8")?;
    let minimum_source = request
        .mappings
        .iter()
        .map(|(target, _)| *target)
        .max()
        .and_then(|target| target.checked_add(1))
        .unwrap_or(3)
        .max(3);
    let readiness = notify_start
        .then(|| readiness_pipe(minimum_source))
        .transpose()?;
    let started_fd = readiness.as_ref().map(|(_, write)| write.as_raw_fd());
    let mut mappings = Vec::with_capacity(request.mappings.len());
    for (target, source) in request.mappings {
        let duplicate =
            unsafe { libc::fcntl(source.as_raw_fd(), libc::F_DUPFD_CLOEXEC, minimum_source) };
        if duplicate < 0 {
            return Err(std::io::Error::last_os_error()).context("duplicate Spawn file descriptor");
        }
        mappings.push((target, unsafe { OwnedFd::from_raw_fd(duplicate) }));
    }
    let mapped_targets = mappings
        .iter()
        .map(|(target, _)| *target)
        .collect::<Vec<_>>();
    let mut command = secure_launch::command(secure_launch::LaunchRequest {
        root: launch_root,
        runtime_root: &context.runtime_root,
        uid: context.uid,
        gid: context.gid,
        mapped_fds: &mapped_targets,
        supplementary_gids: &context.supplementary_gids,
        cwd: cwd.as_deref(),
        nested_sandbox,
        no_network,
        started_fd,
        environment: &environment,
        argv: &argv,
    })?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    unsafe { command.pre_exec(move || map_fds_in_child(&mappings)) };
    let child = command
        .spawn()
        .context("spawn command through existing chroot")?;
    Ok(SpawnedProcess {
        child,
        started: readiness.map(|(read, _)| read),
        _nested_root: nested_root,
    })
}

unsafe fn map_fds_in_child(mappings: &[(i32, OwnedFd)]) -> std::io::Result<()> {
    for (target, source) in mappings {
        if libc::dup2(source.as_raw_fd(), *target) < 0 || libc::fcntl(*target, libc::F_SETFD, 0) < 0
        {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

fn spawn_started_payload(pid: u32, relative_pid: u32) -> [u8; 8] {
    let mut payload = [0; 8];
    payload[..4].copy_from_slice(&pid.to_be_bytes());
    payload[4..].copy_from_slice(&relative_pid.to_be_bytes());
    payload
}

fn spawn_started_relative_pid(expose_pids: bool, started_pid: u32) -> u32 {
    if expose_pids {
        started_pid
    } else {
        0
    }
}

unsafe fn send_spawn_started(fd: RawFd, request: u32, pid: u32, relative_pid: u32) -> bool {
    let payload = spawn_started_payload(pid, relative_pid);
    send_frame(fd, &frame(SPAWN_STARTED, request, &payload, 0), &payload)
}

fn wait_for_spawn_started(fd: RawFd) -> Result<u32> {
    let mut bytes = [0; 4];
    let mut read = 0;
    while read < bytes.len() {
        let count =
            unsafe { libc::read(fd, bytes[read..].as_mut_ptr().cast(), bytes.len() - read) };
        if count > 0 {
            read += count as usize;
            continue;
        }
        if count == 0 {
            bail!("secure launch closed before Spawn readiness");
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error).context("wait for Spawn readiness");
        }
    }
    let pid = u32::from_be_bytes(bytes);
    if pid == 0 {
        bail!("invalid Spawn readiness notification");
    }
    Ok(pid)
}

unsafe fn complete_spawn_start_notification(
    fd: RawFd,
    request: u32,
    portal_pid: u32,
    started: Option<&OwnedFd>,
    expose_pids: bool,
) -> bool {
    let Some(started) = started else {
        return true;
    };
    let started_pid = match wait_for_spawn_started(started.as_raw_fd()) {
        Ok(relative_pid) => relative_pid,
        Err(error) => {
            eprintln!("spawn broker request {request}: readiness failed: {error:#}");
            0
        }
    };
    let relative_pid = spawn_started_relative_pid(expose_pids, started_pid);
    send_spawn_started(fd, request, portal_pid, relative_pid)
}

fn raw_wait_status(status: ExitStatus) -> u32 {
    use std::os::unix::process::ExitStatusExt;
    status.into_raw() as u32
}

fn cleanup_exited_watch_bus_owner(
    supervisor: &ProcessReaper,
    owner_subtree: Option<super::process_supervision::SandboxSubtree>,
    caller_pid: u32,
) -> bool {
    let Some(owner_subtree) = owner_subtree else {
        eprintln!("spawn broker WATCH_BUS caller pid {caller_pid} has no tracked reaper subtree");
        return true;
    };
    match supervisor.terminate_orphaned_subtree_with_signal(owner_subtree, libc::SIGINT) {
        Ok(cleaned) => cleaned,
        Err(error) => {
            eprintln!("spawn broker WATCH_BUS orphaned subtree cleanup failed: {error:#}");
            true
        }
    }
}

fn resolve_watch_bus_owner<T>(
    watch_bus: bool,
    caller_pid: u32,
    resolve: impl FnOnce(u32) -> Result<Option<T>>,
) -> Result<Option<T>> {
    if !watch_bus {
        return Ok(None);
    }
    if caller_pid == 0 {
        bail!("WATCH_BUS caller has no authenticated process id");
    }
    resolve(caller_pid)?
        .with_context(|| format!("WATCH_BUS caller pid {caller_pid} has no current reaper subtree"))
        .map(Some)
}

unsafe fn handle_connection(
    connection: OwnedFd,
    context: Arc<SandboxExecutionContext>,
    supervisor: Arc<ProcessReaper>,
    connections: Arc<BrokerConnections>,
) {
    let fd = connection.as_raw_fd();
    if let Some((packet, fds)) = receive_packet(fd) {
        if let Some((kind, request, _, _)) = parse_frame(&packet) {
            match kind {
                PING if fds.is_empty() => {
                    let _ = send_frame(fd, &frame(PONG, request, &[], 0), &[]);
                }
                FD_TEST if fds.len() == 1 => {
                    let mut byte = 0;
                    if libc::read(fds[0].as_raw_fd(), (&mut byte as *mut u8).cast(), 1) == 1
                        && byte == b'F'
                    {
                        let _ = send_frame(fd, &frame(FD_OK, request, &[], 0), &[]);
                    }
                }
                LIFECYCLE_TEST_START
                    if fds.is_empty()
                        && send_frame(
                            fd,
                            &frame(LIFECYCLE_TEST_ACCEPTED, request, &request.to_be_bytes(), 0),
                            &request.to_be_bytes(),
                        ) =>
                {
                    for _ in 0..50 {
                        if connections.stopping.load(Ordering::SeqCst) {
                            break;
                        }
                        thread::sleep(std::time::Duration::from_millis(1));
                    }
                    if !connections.stopping.load(Ordering::SeqCst) {
                        let mut payload = [0u8; 8];
                        payload[..4].copy_from_slice(&request.to_be_bytes());
                        let _ = send_frame(
                            fd,
                            &frame(LIFECYCLE_TEST_EXITED, request, &payload, 0),
                            &payload,
                        );
                    }
                }
                SPAWN => {
                    let payload = &packet[HEADER_SIZE..];
                    let passed_fds = fds.len();
                    let spawn_request = match parse_spawn(payload, fds) {
                        Ok(request) => request,
                        Err(reason) => {
                            eprintln!("spawn broker rejected request {request}: stage=parse reason={reason} payload_bytes={} passed_fds={}", payload.len(), passed_fds);
                            connections.release(fd);
                            return;
                        }
                    };
                    let cwd_bytes = spawn_request.cwd.as_ref().map_or(0, Vec::len);
                    eprintln!("spawn broker request {request}: flags={} caller_pid={} cwd_bytes={cwd_bytes} argv_count={} environment_count={} fd_mappings={} payload_bytes={}", spawn_request.flags, spawn_request.caller_pid, spawn_request.argv.len(), spawn_request.environment.len(), spawn_request.mappings.len(), payload.len());
                    let watch_bus = spawn_request.flags & SPAWN_WATCH_BUS != 0;
                    let expose_pids = spawn_request.flags & SPAWN_EXPOSE_PIDS != 0;
                    let caller_pid = spawn_request.caller_pid;
                    let watch_owner_subtree = match resolve_watch_bus_owner(
                        watch_bus,
                        caller_pid,
                        |pid| supervisor.subtree_for_descendant(pid),
                    ) {
                        Ok(subtree) => subtree,
                        Err(error) => {
                            eprintln!("spawn broker rejected request {request}: stage=watch-bus-owner error={error:#}");
                            connections.release(fd);
                            return;
                        }
                    };
                    let spawned = match spawn_in_existing_sandbox(&context, spawn_request) {
                        Ok(spawned) => spawned,
                        Err(error) => {
                            eprintln!("spawn broker rejected request {request}: stage=launch error={error:#}");
                            connections.release(fd);
                            return;
                        }
                    };
                    let SpawnedProcess {
                        mut child,
                        started,
                        _nested_root,
                    } = spawned;
                    let portal_pid = child.id();
                    let tree = match supervisor.track(portal_pid) {
                        Ok(tree) => tree,
                        Err(error) => {
                            let _ = child.kill();
                            let _ = child.wait();
                            eprintln!("spawn broker rejected request {request}: stage=track error={error:#}");
                            connections.release(fd);
                            return;
                        }
                    };
                    let accepted = portal_pid.to_be_bytes();
                    if !send_frame(fd, &frame(SPAWN_ACCEPTED, request, &accepted, 0), &accepted) {
                        let _ = tree.wait_for_exit(&mut child, || true);
                        connections.release(fd);
                        return;
                    }
                    if !complete_spawn_start_notification(
                        fd,
                        request,
                        portal_pid,
                        started.as_ref(),
                        expose_pids,
                    ) {
                        let _ = tree.wait_for_exit(&mut child, || true);
                        connections.release(fd);
                        return;
                    }
                    let mut watch_bus_triggered = false;
                    let status = tree.wait_for_exit_with_signal(&mut child, || {
                        let signal = spawn_termination_signal(
                            fd,
                            request,
                            watch_bus,
                            connections.stopping.load(Ordering::SeqCst),
                        );
                        if signal == Some(libc::SIGINT) && !watch_bus_triggered {
                            watch_bus_triggered = true;
                        }
                        signal
                    });
                    if let Ok(status) = status {
                        let mut exited = [0; 8];
                        exited[..4].copy_from_slice(&portal_pid.to_be_bytes());
                        exited[4..].copy_from_slice(&raw_wait_status(status).to_be_bytes());
                        let _ = send_frame(fd, &frame(SPAWN_EXITED, request, &exited, 0), &exited);
                    }
                    while watch_bus && !connections.stopping.load(Ordering::SeqCst) {
                        if !watch_bus_triggered && watch_bus_termination_requested(fd, request) {
                            watch_bus_triggered = true;
                        }
                        if watch_bus_triggered
                            && cleanup_exited_watch_bus_owner(
                                &supervisor,
                                watch_owner_subtree,
                                caller_pid,
                            )
                        {
                            break;
                        }
                        thread::sleep(std::time::Duration::from_millis(100));
                    }
                }
                _ => {}
            }
        }
    }
    connections.release(fd);
}
#[cfg(test)]
mod tests {
    use super::*;
    fn pair() -> (OwnedFd, OwnedFd) {
        let mut f = [0; 2];
        assert_eq!(
            unsafe {
                libc::socketpair(
                    libc::AF_UNIX,
                    libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
                    0,
                    f.as_mut_ptr(),
                )
            },
            0
        );
        unsafe { (OwnedFd::from_raw_fd(f[0]), OwnedFd::from_raw_fd(f[1])) }
    }
    fn send(fd: i32, frame: &[u8], descriptors: &[i32]) {
        unsafe {
            let mut iov = libc::iovec {
                iov_base: frame.as_ptr().cast_mut().cast(),
                iov_len: frame.len(),
            };
            let mut control = [0u8; 256];
            let mut msg: libc::msghdr = std::mem::zeroed();
            msg.msg_iov = &mut iov;
            msg.msg_iovlen = 1;
            if !descriptors.is_empty() {
                msg.msg_control = control.as_mut_ptr().cast();
                msg.msg_controllen = libc::CMSG_SPACE(std::mem::size_of_val(descriptors) as _) as _;
                let c = libc::CMSG_FIRSTHDR(&msg);
                (*c).cmsg_level = libc::SOL_SOCKET;
                (*c).cmsg_type = libc::SCM_RIGHTS;
                (*c).cmsg_len = libc::CMSG_LEN(std::mem::size_of_val(descriptors) as _) as _;
                std::ptr::copy_nonoverlapping(
                    descriptors.as_ptr(),
                    libc::CMSG_DATA(c).cast(),
                    descriptors.len(),
                );
            }
            assert_eq!(libc::sendmsg(fd, &msg, 0), frame.len() as isize);
        }
    }
    fn pipe_f() -> OwnedFd {
        let mut f = [0; 2];
        assert_eq!(unsafe { libc::pipe2(f.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
        assert_eq!(unsafe { libc::write(f[1], b"F".as_ptr().cast(), 1) }, 1);
        unsafe {
            libc::close(f[1]);
            OwnedFd::from_raw_fd(f[0])
        }
    }
    #[test]
    fn non_stdio_fd_mapping_reaches_final_child() {
        let mut pipe = [0; 2];
        assert_eq!(
            unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) },
            0
        );
        assert_eq!(
            unsafe { libc::write(pipe[1], b"mapped\n".as_ptr().cast(), 7) },
            7
        );
        unsafe { libc::close(pipe[1]) };
        let source = unsafe { OwnedFd::from_raw_fd(pipe[0]) };
        let duplicate = unsafe { libc::fcntl(source.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 10) };
        assert!(duplicate >= 10);
        let mappings = vec![(9, unsafe { OwnedFd::from_raw_fd(duplicate) })];
        let mut command = std::process::Command::new("/bin/sh");
        command
            .arg("-c")
            .arg("IFS= read -r value <&9; test \"$value\" = mapped");
        unsafe { command.pre_exec(move || map_fds_in_child(&mappings)) };
        assert!(command.status().unwrap().success());
    }
    #[test]
    fn ping_has_no_fds() {
        let (a, b) = pair();
        let f = frame(PING, 42, &[], 0);
        send(a.as_raw_fd(), &f, &[]);
        let (packet, fds) = unsafe { receive_packet(b.as_raw_fd()) }.unwrap();
        assert!(fds.is_empty());
        assert_eq!(parse_frame(&packet).unwrap().1, 42);
    }
    #[test]
    fn one_fd_is_received_cloexec() {
        let (a, b) = pair();
        let source = pipe_f();
        let f = frame(FD_TEST, 7, &[], 1);
        send(a.as_raw_fd(), &f, &[source.as_raw_fd()]);
        let (_, fds) = unsafe { receive_packet(b.as_raw_fd()) }.unwrap();
        assert_eq!(fds.len(), 1);
        assert_ne!(
            unsafe { libc::fcntl(fds[0].as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        let mut byte = 0;
        assert_eq!(
            unsafe { libc::read(fds[0].as_raw_fd(), (&mut byte as *mut u8).cast(), 1) },
            1
        );
        assert_eq!(byte, b'F');
    }
    #[test]
    fn fd_count_mismatch_rejects() {
        let (a, b) = pair();
        let source = pipe_f();
        let f = frame(PING, 1, &[], 0);
        send(a.as_raw_fd(), &f, &[source.as_raw_fd()]);
        assert!(unsafe { receive_packet(b.as_raw_fd()) }.is_none());
    }
    #[test]
    fn missing_fd_rejects() {
        let (a, b) = pair();
        let f = frame(FD_TEST, 1, &[], 1);
        send(a.as_raw_fd(), &f, &[]);
        assert!(unsafe { receive_packet(b.as_raw_fd()) }.is_none());
    }
    #[test]
    fn multiple_fds_reject_when_advertised_one() {
        let (a, b) = pair();
        let one = pipe_f();
        let two = pipe_f();
        let f = frame(FD_TEST, 1, &[], 1);
        send(a.as_raw_fd(), &f, &[one.as_raw_fd(), two.as_raw_fd()]);
        assert!(unsafe { receive_packet(b.as_raw_fd()) }.is_none());
    }
    #[test]
    fn spawn_payload_preserves_cwd_argv_and_environment_overrides() {
        fn field(payload: &mut Vec<u8>, value: &[u8]) {
            payload.extend_from_slice(&(value.len() as u32).to_be_bytes());
            payload.extend_from_slice(value);
        }
        let mut payload = Vec::new();
        field(&mut payload, b"/app");
        payload.extend_from_slice(&2u32.to_be_bytes());
        payload.extend_from_slice(&1u32.to_be_bytes());
        payload.extend_from_slice(&0u32.to_be_bytes());
        payload.extend_from_slice(&0u32.to_be_bytes());
        field(&mut payload, b"/usr/bin/env");
        field(&mut payload, b"marker");
        field(&mut payload, b"PATH");
        field(&mut payload, b"/custom/bin");
        payload.extend_from_slice(&1234u32.to_be_bytes());

        let request = parse_spawn(&payload, Vec::new()).unwrap();
        assert_eq!(request.cwd.as_deref(), Some(b"/app".as_slice()));
        assert_eq!(
            request.argv,
            vec![b"/usr/bin/env".to_vec(), b"marker".to_vec()]
        );
        assert_eq!(
            merge_environment(&[("PATH".into(), "/usr/bin".into())], &request.environment),
            Some(vec![("PATH".into(), "/custom/bin".into())])
        );
        assert_eq!(request.caller_pid, 1234);
    }
    #[test]
    fn spawn_payload_rejects_duplicate_fd_targets() {
        let (a, b) = pair();
        let mut payload = Vec::new();
        payload.extend_from_slice(&0u32.to_be_bytes());
        payload.extend_from_slice(&1u32.to_be_bytes());
        payload.extend_from_slice(&0u32.to_be_bytes());
        payload.extend_from_slice(&0u32.to_be_bytes());
        payload.extend_from_slice(&2u32.to_be_bytes());
        payload.extend_from_slice(&1u32.to_be_bytes());
        payload.extend_from_slice(b"x");
        payload.extend_from_slice(&3u32.to_be_bytes());
        payload.extend_from_slice(&3u32.to_be_bytes());
        payload.extend_from_slice(&1234u32.to_be_bytes());
        assert!(parse_spawn(&payload, vec![a, b]).is_err());
    }
    #[test]
    fn lifecycle_messages_share_one_connection() {
        let (client, server) = pair();
        let connections = Arc::new(BrokerConnections::default());
        let worker_connections = connections.clone();
        let worker = thread::spawn(move || unsafe {
            handle_connection(
                server,
                Arc::new(SandboxExecutionContext {
                    paths: Installation::for_test(&tempfile_dir()),
                    app_id: "app.id".into(),
                    root: PathBuf::from("/"),
                    runtime_root: PathBuf::from("/"),
                    uid: 0,
                    gid: 0,
                    supplementary_gids: Vec::new(),
                    environment: Vec::new(),
                    mounts: Vec::new(),
                    nested_mounts: Vec::new(),
                }),
                Arc::new(ProcessReaper::test_inert()),
                worker_connections,
            )
        });
        let start = frame(LIFECYCLE_TEST_START, 91, &[], 0);
        send(client.as_raw_fd(), &start, &[]);
        let mut accepted = [0u8; 24];
        assert_eq!(
            unsafe {
                libc::recv(
                    client.as_raw_fd(),
                    accepted.as_mut_ptr().cast(),
                    accepted.len(),
                    0,
                )
            },
            accepted.len() as isize
        );
        let (kind, request, length, fd_count) = parse_frame(&accepted).unwrap();
        assert_eq!(
            (kind, request, length, fd_count),
            (LIFECYCLE_TEST_ACCEPTED, 91, 4, 0)
        );
        let mut exited = [0u8; 28];
        assert_eq!(
            unsafe {
                libc::recv(
                    client.as_raw_fd(),
                    exited.as_mut_ptr().cast(),
                    exited.len(),
                    0,
                )
            },
            exited.len() as isize
        );
        let (kind, request, length, fd_count) = parse_frame(&exited).unwrap();
        assert_eq!(
            (kind, request, length, fd_count),
            (LIFECYCLE_TEST_EXITED, 91, 8, 0)
        );
        worker.join().unwrap();
        assert!(connections.retained_fds.lock().unwrap().is_empty());
    }

    #[test]
    fn broker_retains_shared_execution_context() {
        let dir = tempfile_dir();
        let paths = Installation::for_test(&dir);
        let root = paths.chroots().join("app.id/instance");
        fs::create_dir_all(&root).unwrap();
        let context = Arc::new(SandboxExecutionContext {
            paths: paths.clone(),
            app_id: "app.id".into(),
            root,
            runtime_root: paths.runtime_root().to_path_buf(),
            uid: unsafe { libc::getuid() },
            nested_mounts: Vec::new(),
            gid: unsafe { libc::getgid() },
            supplementary_gids: vec![1],
            environment: vec![("A".into(), "B".into())],
            mounts: Vec::new(),
        });
        let broker = SpawnBroker::bind(
            &paths,
            context.clone(),
            Arc::new(ProcessReaper::test_inert()),
        )
        .unwrap();
        assert!(Arc::ptr_eq(&context, broker.context()));
        assert_eq!(broker.context().environment, context.environment);
        drop(broker);
        assert_eq!(Arc::strong_count(&context), 1);
    }
    #[test]
    fn distinct_instances_keep_distinct_contexts() {
        let dir = tempfile_dir();
        let paths = Installation::for_test(&dir);
        let first = Arc::new(SandboxExecutionContext {
            paths: paths.clone(),
            app_id: "app.id".into(),
            root: paths.chroots().join("app.id/one"),
            runtime_root: paths.runtime_root().to_path_buf(),
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            supplementary_gids: vec![1],
            environment: vec![("A".into(), "one".into())],
            nested_mounts: Vec::new(),
            mounts: Vec::new(),
        });
        let second = Arc::new(SandboxExecutionContext {
            paths: paths.clone(),
            app_id: "app.id".into(),
            root: paths.chroots().join("app.id/two"),
            runtime_root: paths.runtime_root().to_path_buf(),
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            supplementary_gids: vec![2],
            environment: vec![("A".into(), "two".into())],
            mounts: Vec::new(),
            nested_mounts: Vec::new(),
        });
        fs::create_dir_all(&first.root).unwrap();
        fs::create_dir_all(&second.root).unwrap();
        let one = SpawnBroker::bind(&paths, first.clone(), Arc::new(ProcessReaper::test_inert()))
            .unwrap();
        let two = SpawnBroker::bind(
            &paths,
            second.clone(),
            Arc::new(ProcessReaper::test_inert()),
        )
        .unwrap();
        assert!(!Arc::ptr_eq(one.context(), two.context()));
        assert_ne!(one.context().root, two.context().root);
        assert_ne!(
            one.context().supplementary_gids,
            two.context().supplementary_gids
        );
        assert_ne!(one.context().environment, two.context().environment);
    }
    #[test]
    fn path_is_per_instance_and_outside_root() {
        let d = tempfile_dir();
        let paths = Installation::for_test(&d);
        let a = paths.chroots().join("app.id/a");
        let b = paths.chroots().join("app.id/b");
        assert_ne!(
            broker_path(&paths, &a).unwrap(),
            broker_path(&paths, &b).unwrap()
        );
        assert!(!broker_path(&paths, &a).unwrap().starts_with(&a));
    }
    fn tempfile_dir() -> PathBuf {
        let p = std::env::temp_dir().join(format!("ffp-broker-{}", std::process::id()));
        let _ = fs::create_dir_all(&p);
        p
    }
}

#[cfg(test)]
#[path = "tests/spawn_portal.rs"]
mod spawn_portal_tests;

#[cfg(test)]
#[path = "tests/spawn_broker_watch_bus.rs"]
mod watch_bus_tests;
