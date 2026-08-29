use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

const HELPER_PATH: &str = "/usr/local/libexec/freebsd-flatpak/secure-launch";
const NESTED_CLIENT_PATH: &str = "/usr/local/libexec/freebsd-flatpak/secure-launch-client";
const MAX_MAPPED_FDS: usize = 32;
const MAX_TARGET_FD: i32 = 65_535;
const MAX_ARGUMENTS: usize = 1024;
const MAX_ENVIRONMENT: usize = 512;
const NESTED_DAEMON_SOCKET: &str = "/var/run/freebsd-flatpak/secure-launch.sock";
const MAX_NESTED_PACKET: usize = 65_536;
const JAIL_OWN_DESC: libc::c_int = 0x80;

pub(crate) struct LaunchRequest<'a> {
    pub(crate) root: &'a Path,
    pub(crate) runtime_root: &'a Path,
    pub(crate) uid: u32,
    pub(crate) gid: u32,
    pub(crate) supplementary_gids: &'a [u32],
    pub(crate) mapped_fds: &'a [i32],
    pub(crate) cwd: Option<&'a OsStr>,
    pub(crate) nested_sandbox: bool,
    pub(crate) no_network: bool,
    pub(crate) environment: &'a [(String, String)],
    pub(crate) argv: &'a [OsString],
}

pub(crate) fn command(request: LaunchRequest<'_>) -> Result<Command> {
    let LaunchRequest {
        root,
        runtime_root,
        uid,
        gid,
        supplementary_gids,
        mapped_fds,
        cwd,
        nested_sandbox,
        no_network,
        environment,
        argv,
    } = request;
    if argv.is_empty()
        || argv.len() > MAX_ARGUMENTS
        || environment.len() > MAX_ENVIRONMENT
        || mapped_fds.len() > MAX_MAPPED_FDS
    {
        bail!("invalid secure launch request size");
    }
    let root =
        fs::canonicalize(root).with_context(|| format!("canonicalize {}", root.display()))?;
    let runtime_root = fs::canonicalize(runtime_root)
        .with_context(|| format!("canonicalize {}", runtime_root.display()))?;
    let identity = file_identity(&root)?;
    let groups = supplementary_gids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let cwd = cwd.unwrap_or_else(|| OsStr::new("-"));
    let nested = nested_sandbox || no_network;
    let mut command = Command::new(if nested {
        NESTED_CLIENT_PATH
    } else {
        HELPER_PATH
    });
    command
        .arg(root)
        .arg(runtime_root)
        .arg(identity.0.to_string())
        .arg(identity.1.to_string())
        .arg(uid.to_string())
        .arg(gid.to_string())
        .arg(groups)
        .arg(cwd)
        .arg(if no_network {
            "no-network"
        } else if nested_sandbox {
            "sandbox"
        } else {
            "direct"
        })
        .arg(
            mapped_fds
                .iter()
                .map(i32::to_string)
                .collect::<Vec<_>>()
                .join(","),
        )
        .arg(environment.len().to_string());
    for (key, value) in environment {
        if key.is_empty() || key.contains('=') || key.contains('\0') || value.contains('\0') {
            bail!("invalid launch environment entry");
        }
        command.arg(format!("{key}={value}"));
    }
    command.arg("--").args(argv);
    Ok(command)
}

pub(crate) fn run_helper() -> Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        bail!("secure launch helper must run set-user-ID root");
    }
    let real_uid = unsafe { libc::getuid() };
    let real_gid = unsafe { libc::getgid() };
    if real_uid == 0 {
        bail!("secure launch helper does not accept root callers");
    }
    let request = Request::parse(env::args_os())?;
    request.validate(real_uid, real_gid)?;
    request.execute(real_uid, real_gid)
}

struct Request {
    root: PathBuf,
    runtime_root: PathBuf,
    root_device: u64,
    root_inode: u64,
    uid: u32,
    gid: u32,
    groups: Vec<u32>,
    cwd: Option<OsString>,
    jail_mode: JailMode,
    mapped_fds: Vec<i32>,
    environment: Vec<OsString>,
    argv: Vec<OsString>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum JailMode {
    Direct,
    Sandbox,
    NoNetwork,
}

impl Request {
    fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Self> {
        let _program = args.next();
        let root = PathBuf::from(required(&mut args, "sandbox root")?);
        let runtime_root = PathBuf::from(required(&mut args, "runtime root")?);
        let root_device = parse_u64(
            &required(&mut args, "sandbox root device")?,
            "sandbox root device",
        )?;
        let root_inode = parse_u64(
            &required(&mut args, "sandbox root inode")?,
            "sandbox root inode",
        )?;
        let uid = parse_u32(&required(&mut args, "uid")?, "uid")?;
        let gid = parse_u32(&required(&mut args, "gid")?, "gid")?;
        let groups = parse_groups(&required(&mut args, "supplementary groups")?)?;
        let cwd_value = required(&mut args, "working directory")?;
        let cwd = (cwd_value != OsStr::new("-")).then_some(cwd_value);
        let jail_mode = parse_jail_mode(&required(&mut args, "launch isolation mode")?)?;
        let mapped_fds = parse_mapped_fds(&required(&mut args, "mapped file descriptors")?)?;
        let environment_count = parse_usize(
            &required(&mut args, "environment count")?,
            "environment count",
        )?;
        if environment_count > MAX_ENVIRONMENT {
            bail!("too many launch environment entries");
        }
        let mut environment = Vec::with_capacity(environment_count);
        for _ in 0..environment_count {
            let entry = required(&mut args, "environment entry")?;
            validate_environment(&entry)?;
            environment.push(entry);
        }
        if args.next().as_deref() != Some(OsStr::new("--")) {
            bail!("secure launch request is missing argument separator");
        }
        let argv = args.collect::<Vec<_>>();
        if argv.is_empty() || argv.len() > MAX_ARGUMENTS {
            bail!("invalid secure launch program arguments");
        }
        Ok(Self {
            root,
            runtime_root,
            root_device,
            root_inode,
            uid,
            gid,
            groups,
            cwd,
            jail_mode,
            mapped_fds,
            environment,
            argv,
        })
    }

    fn validate(&self, real_uid: libc::uid_t, real_gid: libc::gid_t) -> Result<()> {
        if self.uid != real_uid || self.gid != real_gid {
            bail!("secure launch credentials do not match caller");
        }
        let allowed_groups = caller_groups(real_gid)?;
        if self
            .groups
            .iter()
            .any(|group| !allowed_groups.contains(group))
        {
            bail!("secure launch requested a group not held by caller");
        }
        validate_sandbox_root(
            &self.root,
            &self.runtime_root,
            self.root_device,
            self.root_inode,
            real_uid,
        )?;
        if let Some(cwd) = &self.cwd {
            let cwd = Path::new(cwd);
            if !cwd.is_absolute()
                || cwd
                    .components()
                    .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
            {
                bail!(
                    "secure launch working directory must be an absolute normalized sandbox path"
                );
            }
        }
        Ok(())
    }

    fn execute(self, real_uid: libc::uid_t, real_gid: libc::gid_t) -> Result<()> {
        self.execute_with_mappings(real_uid, real_gid, &[], &[], None)
    }

    fn execute_with_mappings(
        self,
        real_uid: libc::uid_t,
        real_gid: libc::gid_t,
        mapped_targets: &[i32],
        mapped_sources: &[OwnedFd],
        jail_lifecycle: Option<RawFd>,
    ) -> Result<()> {
        let mut root_fd = Some(open_directory(&self.root)?);
        if file_identity_fd(root_fd.as_ref().expect("sandbox root fd").as_raw_fd())?
            != (self.root_device, self.root_inode)
        {
            bail!("sandbox root changed while preparing secure launch");
        }
        if self.jail_mode != JailMode::Direct {
            // FreeBSD jail attachment performs chroot(2) internally and
            // rejects a process that still has an open directory descriptor.
            // Create the jail first while cwd is anchored to the validated
            // descriptor, then close it before attaching.
            let lifecycle = jail_lifecycle.context("nested jail requires daemon supervision")?;
            if unsafe { libc::setuid(0) } != 0
                || unsafe { libc::setgroups(0, std::ptr::null()) } != 0
                || unsafe { libc::setgid(0) } != 0
            {
                return Err(io::Error::last_os_error())
                    .context("adopt root credentials for nested sandbox jail");
            }
            if unsafe { libc::fchdir(root_fd.as_ref().expect("sandbox root fd").as_raw_fd()) } != 0
            {
                return Err(io::Error::last_os_error()).context("anchor nested jail root");
            }
            let (jail, owner) = create_nested_jail(self.jail_mode == JailMode::NoNetwork)?;
            drop(root_fd.take());
            if unsafe { jail_attach(jail) } != 0 {
                let error = io::Error::last_os_error();
                unsafe { jail_remove(jail) };
                return Err(error).context("attach nested sandbox jail");
            }
            synchronize_nested_jail(lifecycle, jail, owner)?;
        } else {
            // Descriptor-anchored chroot prevents a validated path from being
            // replaced before entry. Do not install caller-selected descriptor
            // numbers until after every privileged operation that uses root_fd:
            // otherwise an SCM_RIGHTS mapping could overwrite that descriptor and
            // redirect chroot(2) outside the validated sandbox.
            let dot = std::ffi::CString::new(".")?;
            let slash = std::ffi::CString::new("/")?;
            let root_fd = root_fd.take().expect("sandbox root fd");
            if unsafe { libc::fchdir(root_fd.as_raw_fd()) } != 0
                || unsafe { libc::chroot(dot.as_ptr()) } != 0
            {
                return Err(io::Error::last_os_error()).context("enter sandbox chroot");
            }
            if unsafe { libc::chdir(slash.as_ptr()) } != 0 {
                return Err(io::Error::last_os_error()).context("enter sandbox root");
            }
        }
        if let Some(cwd) = &self.cwd {
            let cwd = std::ffi::CString::new(cwd.as_bytes())?;
            if unsafe { libc::chdir(cwd.as_ptr()) } != 0 {
                return Err(io::Error::last_os_error()).context("change sandbox working directory");
            }
        }
        // Caller-selected descriptor targets are safe only after chroot and
        // cwd selection no longer depend on privileged descriptors.
        map_nested_fds(mapped_targets, mapped_sources)?;
        let groups = self
            .groups
            .iter()
            .map(|group| *group as libc::gid_t)
            .collect::<Vec<_>>();
        if unsafe {
            libc::setgroups(
                groups.len().try_into().expect("group limit checked"),
                groups.as_ptr(),
            )
        } != 0
            || unsafe { libc::setgid(real_gid) } != 0
            || unsafe { libc::setuid(real_uid) } != 0
        {
            return Err(io::Error::last_os_error()).context("drop secure launch credentials");
        }
        if unsafe { libc::geteuid() } != real_uid || unsafe { libc::getegid() } != real_gid {
            bail!("secure launch could not drop privilege");
        }
        let error = Command::new(&self.argv[0])
            .args(&self.argv[1..])
            .env_clear()
            .envs(self.environment.iter().filter_map(split_environment))
            .exec();
        Err(error).context("execute sandbox program")
    }
}

#[repr(C)]
struct JailParam {
    name: *mut libc::c_char,
    value: *mut libc::c_void,
    value_len: usize,
    element_len: usize,
    control_type: libc::c_int,
    struct_type: libc::c_int,
    flags: libc::c_uint,
}

#[link(name = "jail")]
unsafe extern "C" {
    fn jailparam_init(parameter: *mut JailParam, name: *const libc::c_char) -> libc::c_int;
    fn jailparam_import(parameter: *mut JailParam, value: *const libc::c_char) -> libc::c_int;
    fn jailparam_import_raw(
        parameter: *mut JailParam,
        value: *mut libc::c_void,
        value_len: usize,
    ) -> libc::c_int;
    fn jailparam_set(
        parameters: *mut JailParam,
        count: libc::c_uint,
        flags: libc::c_int,
    ) -> libc::c_int;
    fn jailparam_free(parameters: *mut JailParam, count: libc::c_uint);
    fn jail_attach(jid: libc::c_int) -> libc::c_int;
    fn jail_remove(jid: libc::c_int) -> libc::c_int;
}

fn set_jail_parameters(entries: &[(&str, &std::ffi::CStr)], flags: libc::c_int) -> Result<i32> {
    let names = entries
        .iter()
        .map(|(name, _)| std::ffi::CString::new(*name))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut parameters = entries
        .iter()
        .map(|_| JailParam {
            name: std::ptr::null_mut(),
            value: std::ptr::null_mut(),
            value_len: 0,
            element_len: 0,
            control_type: 0,
            struct_type: 0,
            flags: 0,
        })
        .collect::<Vec<_>>();
    for ((parameter, name), value) in parameters
        .iter_mut()
        .zip(&names)
        .zip(entries.iter().map(|(_, value)| *value))
    {
        if unsafe { jailparam_init(parameter, name.as_ptr()) } < 0
            || unsafe { jailparam_import(parameter, value.as_ptr()) } < 0
        {
            unsafe { jailparam_free(parameters.as_mut_ptr(), parameters.len() as _) };
            return Err(io::Error::last_os_error()).context("prepare nested sandbox jail");
        }
    }
    let result = unsafe {
        jailparam_set(
            parameters.as_mut_ptr(),
            parameters.len().try_into().expect("jail option limit"),
            flags,
        )
    };
    unsafe { jailparam_free(parameters.as_mut_ptr(), parameters.len() as _) };
    if result < 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(result)
}

fn create_nested_jail(no_network: bool) -> Result<(i32, OwnedFd)> {
    let path = std::ffi::CString::new(".")?;
    let network = if no_network { "disable" } else { "inherit" };
    let hostname = std::ffi::CString::new("freebsd-flatpak")?;
    let ip4 = std::ffi::CString::new(network)?;
    let ip6 = std::ffi::CString::new(network)?;
    let persist = std::ffi::CString::new("true")?;
    let entries = [
        ("path", path.as_c_str()),
        ("host.hostname", hostname.as_c_str()),
        ("ip4", ip4.as_c_str()),
        ("ip6", ip6.as_c_str()),
        ("persist", persist.as_c_str()),
    ];
    let names = entries
        .iter()
        .map(|(name, _)| std::ffi::CString::new(*name))
        .chain(std::iter::once(Ok(
            std::ffi::CString::new("desc").expect("static jail parameter")
        )))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut parameters = names
        .iter()
        .map(|_| JailParam {
            name: std::ptr::null_mut(),
            value: std::ptr::null_mut(),
            value_len: 0,
            element_len: 0,
            control_type: 0,
            struct_type: 0,
            flags: 0,
        })
        .collect::<Vec<_>>();
    for ((parameter, name), value) in parameters[..entries.len()]
        .iter_mut()
        .zip(&names)
        .zip(entries.iter().map(|(_, value)| *value))
    {
        if unsafe { jailparam_init(parameter, name.as_ptr()) } < 0
            || unsafe { jailparam_import(parameter, value.as_ptr()) } < 0
        {
            unsafe { jailparam_free(parameters.as_mut_ptr(), parameters.len() as _) };
            return Err(io::Error::last_os_error()).context("prepare nested sandbox jail");
        }
    }
    let descriptor = parameters.last_mut().expect("jail descriptor parameter");
    let descriptor_name = names.last().expect("jail descriptor name");
    let mut owner = -1;
    if unsafe { jailparam_init(descriptor, descriptor_name.as_ptr()) } < 0
        || unsafe {
            jailparam_import_raw(
                descriptor,
                (&mut owner as *mut i32).cast(),
                std::mem::size_of_val(&owner),
            )
        } < 0
    {
        unsafe { jailparam_free(parameters.as_mut_ptr(), parameters.len() as _) };
        return Err(io::Error::last_os_error()).context("prepare nested sandbox jail");
    }
    let jail = unsafe {
        jailparam_set(
            parameters.as_mut_ptr(),
            parameters.len().try_into().expect("jail option limit"),
            libc::JAIL_CREATE | JAIL_OWN_DESC,
        )
    };
    unsafe { jailparam_free(parameters.as_mut_ptr(), parameters.len() as _) };
    if jail < 0 {
        return Err(io::Error::last_os_error()).context("create nested sandbox jail");
    }
    if owner < 0 {
        bail!("nested sandbox jail did not return an owning descriptor");
    }
    Ok((jail, unsafe { OwnedFd::from_raw_fd(owner) }))
}

fn clear_nested_jail_persistence(jail: i32) -> Result<()> {
    let jail = std::ffi::CString::new(jail.to_string())?;
    let persist = std::ffi::CString::new("false")?;
    set_jail_parameters(
        &[("jid", jail.as_c_str()), ("persist", persist.as_c_str())],
        libc::JAIL_UPDATE,
    )
    .context("clear nested sandbox jail persistence")?;
    Ok(())
}

fn write_all_fd(fd: RawFd, bytes: &[u8]) -> Result<()> {
    let mut bytes = bytes;
    while !bytes.is_empty() {
        let written =
            unsafe { libc::send(fd, bytes.as_ptr().cast(), bytes.len(), libc::MSG_NOSIGNAL) };
        if written < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(io::Error::last_os_error()).context("write nested jail lifecycle");
        }
        if written == 0 {
            bail!("nested jail lifecycle closed unexpectedly");
        }
        bytes = &bytes[written as usize..];
    }
    Ok(())
}

fn read_exact_fd(fd: RawFd, mut bytes: &mut [u8]) -> Result<()> {
    while !bytes.is_empty() {
        let read = unsafe { libc::read(fd, bytes.as_mut_ptr().cast(), bytes.len()) };
        if read < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(io::Error::last_os_error()).context("read nested jail lifecycle");
        }
        if read == 0 {
            bail!("nested jail lifecycle closed unexpectedly");
        }
        let (_, remaining) = bytes.split_at_mut(read as usize);
        bytes = remaining;
    }
    Ok(())
}

fn send_nested_jail_lifecycle(fd: RawFd, jail: i32, owner: RawFd) -> Result<()> {
    let jail = jail.to_ne_bytes();
    let mut iov = libc::iovec {
        iov_base: jail.as_ptr().cast_mut().cast(),
        iov_len: jail.len(),
    };
    let mut control =
        vec![0u8; unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as _) as usize }];
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = control.len() as _;
    let cmsg = unsafe { libc::CMSG_FIRSTHDR(&message) };
    if cmsg.is_null() {
        bail!("prepare nested jail lifecycle descriptor");
    }
    unsafe {
        (*cmsg).cmsg_level = libc::SOL_SOCKET;
        (*cmsg).cmsg_type = libc::SCM_RIGHTS;
        (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as _) as _;
        *libc::CMSG_DATA(cmsg).cast::<RawFd>() = owner;
    }
    let sent = unsafe { libc::sendmsg(fd, &message, libc::MSG_NOSIGNAL) };
    if sent != jail.len() as isize {
        if sent == 0 {
            bail!("nested jail lifecycle closed unexpectedly");
        }
        return Err(io::Error::last_os_error()).context("send nested jail lifecycle");
    }
    Ok(())
}

fn receive_nested_jail_lifecycle(fd: RawFd) -> Result<(i32, OwnedFd)> {
    let mut jail = [0; std::mem::size_of::<i32>()];
    let mut iov = libc::iovec {
        iov_base: jail.as_mut_ptr().cast(),
        iov_len: jail.len(),
    };
    let mut control =
        vec![0u8; unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as _) as usize }];
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = control.len() as _;
    let received = unsafe { libc::recvmsg(fd, &mut message, 0) };
    if received != jail.len() as isize {
        unsafe { close_nested_control_fds(&message) };
        if received < 0 {
            return Err(io::Error::last_os_error()).context("receive nested jail lifecycle");
        }
        bail!("nested jail lifecycle closed unexpectedly");
    }
    let cmsg = unsafe { libc::CMSG_FIRSTHDR(&message) };
    if message.msg_flags & libc::MSG_CTRUNC != 0
        || cmsg.is_null()
        || unsafe {
            (*cmsg).cmsg_level != libc::SOL_SOCKET
                || (*cmsg).cmsg_type != libc::SCM_RIGHTS
                || (*cmsg).cmsg_len != libc::CMSG_LEN(std::mem::size_of::<RawFd>() as _) as _
                || !libc::CMSG_NXTHDR(&message, cmsg).is_null()
        }
    {
        unsafe { close_nested_control_fds(&message) };
        bail!("invalid nested jail lifecycle descriptor");
    }
    let owner = unsafe { *libc::CMSG_DATA(cmsg).cast::<RawFd>() };
    if owner < 0 {
        unsafe { close_nested_control_fds(&message) };
        bail!("invalid nested jail lifecycle descriptor");
    }
    Ok((i32::from_ne_bytes(jail), unsafe {
        OwnedFd::from_raw_fd(owner)
    }))
}

fn synchronize_nested_jail(fd: RawFd, jail: i32, owner: OwnedFd) -> Result<()> {
    send_nested_jail_lifecycle(fd, jail, owner.as_raw_fd())?;
    let mut approved = [0];
    read_exact_fd(fd, &mut approved)?;
    if approved != [1] {
        bail!("nested jail supervisor rejected launch");
    }
    Ok(())
}

fn nested_jail_lifecycle_socket() -> Result<(OwnedFd, OwnedFd)> {
    let mut sockets = [-1; 2];
    if unsafe {
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
            0,
            sockets.as_mut_ptr(),
        )
    } != 0
    {
        return Err(io::Error::last_os_error()).context("create nested jail lifecycle socket");
    }
    Ok(unsafe {
        (
            OwnedFd::from_raw_fd(sockets[0]),
            OwnedFd::from_raw_fd(sockets[1]),
        )
    })
}

fn required(args: &mut impl Iterator<Item = OsString>, label: &str) -> Result<OsString> {
    args.next().with_context(|| format!("missing {label}"))
}
fn parse_u64(value: &OsStr, label: &str) -> Result<u64> {
    value
        .to_str()
        .with_context(|| format!("{label} is not UTF-8"))?
        .parse()
        .with_context(|| format!("invalid {label}"))
}
fn parse_u32(value: &OsStr, label: &str) -> Result<u32> {
    value
        .to_str()
        .with_context(|| format!("{label} is not UTF-8"))?
        .parse()
        .with_context(|| format!("invalid {label}"))
}
fn parse_usize(value: &OsStr, label: &str) -> Result<usize> {
    value
        .to_str()
        .with_context(|| format!("{label} is not UTF-8"))?
        .parse()
        .with_context(|| format!("invalid {label}"))
}
fn parse_jail_mode(value: &OsStr) -> Result<JailMode> {
    match value.to_str() {
        Some("direct") => Ok(JailMode::Direct),
        Some("sandbox") => Ok(JailMode::Sandbox),
        Some("no-network") => Ok(JailMode::NoNetwork),
        _ => bail!("invalid launch isolation mode"),
    }
}

fn parse_groups(value: &OsStr) -> Result<Vec<u32>> {
    let value = value
        .to_str()
        .context("supplementary groups are not UTF-8")?;
    if value.is_empty() {
        return Ok(Vec::new());
    }
    let mut seen = HashSet::new();
    value
        .split(',')
        .map(|group| {
            let group = group
                .parse::<u32>()
                .context("invalid supplementary group")?;
            if !seen.insert(group) {
                bail!("duplicate supplementary group");
            }
            Ok(group)
        })
        .collect()
}
fn parse_mapped_fds(value: &OsStr) -> Result<Vec<i32>> {
    let value = value
        .to_str()
        .context("mapped file descriptors are not UTF-8")?;
    if value.is_empty() {
        return Ok(Vec::new());
    }
    let mut seen = HashSet::new();
    let fds = value
        .split(',')
        .map(|fd| fd.parse::<i32>().context("invalid mapped file descriptor"))
        .collect::<Result<Vec<_>>>()?;
    if fds.len() > MAX_MAPPED_FDS
        || fds
            .iter()
            .any(|fd| !(0..=MAX_TARGET_FD).contains(fd) || !seen.insert(*fd))
    {
        bail!("invalid mapped file descriptors");
    }
    Ok(fds)
}
fn validate_environment(entry: &OsStr) -> Result<()> {
    let bytes = entry.as_bytes();
    if bytes.first() == Some(&b'=') || !bytes.contains(&b'=') {
        bail!("invalid launch environment entry");
    }
    Ok(())
}
fn split_environment(entry: &OsString) -> Option<(&OsStr, &OsStr)> {
    let bytes = entry.as_bytes();
    let split = bytes.iter().position(|byte| *byte == b'=')?;
    Some((
        OsStr::from_bytes(&bytes[..split]),
        OsStr::from_bytes(&bytes[split + 1..]),
    ))
}

fn caller_groups(real_gid: libc::gid_t) -> Result<HashSet<u32>> {
    let count = unsafe { libc::getgroups(0, std::ptr::null_mut()) };
    if count < 0 {
        return Err(io::Error::last_os_error()).context("read caller groups");
    }
    let mut groups = vec![0 as libc::gid_t; count as usize];
    if unsafe { libc::getgroups(count, groups.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error()).context("read caller groups");
    }
    let mut allowed = groups.into_iter().collect::<HashSet<_>>();
    allowed.insert(real_gid);
    Ok(allowed)
}

fn validate_sandbox_root(
    root: &Path,
    runtime_root: &Path,
    device: u64,
    inode: u64,
    uid: libc::uid_t,
) -> Result<()> {
    let root = fs::canonicalize(root)
        .with_context(|| format!("canonicalize sandbox root {}", root.display()))?;
    let runtime_root = fs::canonicalize(runtime_root)
        .with_context(|| format!("canonicalize runtime root {}", runtime_root.display()))?;
    let metadata = fs::metadata(&runtime_root).context("inspect sandbox runtime root")?;
    if metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        bail!("sandbox runtime root is not private to caller");
    }
    let chroots = runtime_root.join("chroots");
    let relative = root
        .strip_prefix(&chroots)
        .context("sandbox root is outside freebsd-flatpak chroots")?;
    let parts = relative.components().collect::<Vec<_>>();
    if parts.len() != 2
        || parts
            .iter()
            .any(|part| !matches!(part, Component::Normal(name) if !name.is_empty()))
    {
        bail!("sandbox root is not a freebsd-flatpak instance");
    }
    let root_fd = open_directory(&root)?;
    if file_identity_fd(root_fd.as_raw_fd())? != (device, inode) {
        bail!("sandbox root identity does not match request");
    }
    let info_name = std::ffi::CString::new(".flatpak-info")?;
    let info_fd = unsafe {
        libc::openat(
            root_fd.as_raw_fd(),
            info_name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if info_fd < 0 {
        return Err(io::Error::last_os_error()).context("open sandbox .flatpak-info");
    }
    let info = unsafe { OwnedFd::from_raw_fd(info_fd) };
    let root_stat = stat_fd(root_fd.as_raw_fd())?;
    let info_stat = stat_fd(info.as_raw_fd())?;
    if root_stat.st_uid != uid
        || info_stat.st_uid != uid
        || info_stat.st_mode & libc::S_IFMT != libc::S_IFREG
    {
        bail!("sandbox root or .flatpak-info is not caller-owned and regular");
    }
    Ok(())
}

fn open_directory(path: &Path) -> Result<OwnedFd> {
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    let fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error()).context("open sandbox root");
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}
fn stat_fd(fd: i32) -> Result<libc::stat> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error()).context("fstat");
    }
    Ok(unsafe { stat.assume_init() })
}
fn file_identity(path: &Path) -> Result<(u64, u64)> {
    let fd = open_directory(path)?;
    file_identity_fd(fd.as_raw_fd())
}
fn file_identity_fd(fd: i32) -> Result<(u64, u64)> {
    let stat = stat_fd(fd)?;
    Ok((stat.st_dev, stat.st_ino))
}

use std::os::unix::fs::MetadataExt;
pub(crate) fn run_nested_client() -> Result<()> {
    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };
    if uid == 0 {
        bail!("nested secure-launch client does not accept root callers");
    }
    let request = Request::parse(env::args_os())?;
    if request.jail_mode == JailMode::Direct {
        bail!("nested secure-launch client requires nested isolation");
    }
    request.validate(uid, gid)?;
    let socket = connect_nested_socket()?;
    let args = request.wire_arguments();
    let fds = request
        .mapped_fds
        .iter()
        .map(|fd| duplicate_fd(*fd))
        .collect::<Result<Vec<_>>>()?;
    send_nested_request(socket.as_raw_fd(), &args, &fds)?;
    drop(fds);
    let status = read_status(socket.as_raw_fd())?;
    exit_from_wait_status(status)
}

pub(crate) fn run_nested_daemon() -> Result<()> {
    if unsafe { libc::getuid() } != 0 || unsafe { libc::geteuid() } != 0 {
        bail!("nested secure-launch daemon must run as root");
    }
    let listener = bind_nested_socket()?;
    loop {
        let connection = unsafe {
            libc::accept4(
                listener.as_raw_fd(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                libc::SOCK_CLOEXEC,
            )
        };
        if connection < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(io::Error::last_os_error()).context("accept nested secure-launch request");
        }
        let connection = unsafe { OwnedFd::from_raw_fd(connection) };
        if let Err(error) = serve_nested_request(connection.as_raw_fd()) {
            eprintln!("freebsd-flatpak secure-launch daemon: {error:#}");
        }
    }
}

impl Request {
    fn wire_arguments(&self) -> Vec<OsString> {
        let mut args = vec![
            OsString::from("secure-launch"),
            self.root.clone().into_os_string(),
            self.runtime_root.clone().into_os_string(),
            OsString::from(self.root_device.to_string()),
            OsString::from(self.root_inode.to_string()),
            OsString::from(self.uid.to_string()),
            OsString::from(self.gid.to_string()),
            OsString::from(
                self.groups
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
            ),
            self.cwd.clone().unwrap_or_else(|| OsString::from("-")),
            OsString::from(match self.jail_mode {
                JailMode::Direct => "direct",
                JailMode::Sandbox => "sandbox",
                JailMode::NoNetwork => "no-network",
            }),
            OsString::from(
                self.mapped_fds
                    .iter()
                    .map(i32::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
            ),
            OsString::from(self.environment.len().to_string()),
        ];
        args.extend(self.environment.iter().cloned());
        args.push(OsString::from("--"));
        args.extend(self.argv.iter().cloned());
        args
    }

    fn validate_for_peer(&self, uid: libc::uid_t, gid: libc::gid_t) -> Result<()> {
        if self.uid != uid || self.gid != gid {
            bail!("nested request credentials do not match socket peer");
        }
        let allowed_groups = account_groups(uid, gid)?;
        if self
            .groups
            .iter()
            .any(|group| !allowed_groups.contains(group))
        {
            bail!("nested request includes a group not held by socket peer");
        }
        validate_sandbox_root(
            &self.root,
            &self.runtime_root,
            self.root_device,
            self.root_inode,
            uid,
        )?;
        if let Some(cwd) = &self.cwd {
            let cwd = Path::new(cwd);
            if !cwd.is_absolute()
                || cwd
                    .components()
                    .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
            {
                bail!("nested working directory must be an absolute normalized sandbox path");
            }
        }
        Ok(())
    }
}

fn serve_nested_request(fd: RawFd) -> Result<()> {
    let (uid, gid) = peer_credentials(fd)?;
    let (args, fds) = receive_nested_request(fd)?;
    let request = Request::parse(args.into_iter())?;
    if request.jail_mode == JailMode::Direct {
        bail!("nested daemon refused direct launch request");
    }
    request.validate_for_peer(uid, gid)?;
    if request.mapped_fds.len() != fds.len() {
        bail!("nested request descriptor count does not match mappings");
    }
    let (parent_lifecycle, child_lifecycle) = nested_jail_lifecycle_socket()?;
    let child = unsafe { libc::fork() };
    if child < 0 {
        return Err(io::Error::last_os_error()).context("fork nested sandbox child");
    }
    if child == 0 {
        drop(parent_lifecycle);
        unsafe { libc::close(fd) };
        let targets = request.mapped_fds.clone();
        if let Err(error) = request.execute_with_mappings(
            uid,
            gid,
            &targets,
            &fds,
            Some(child_lifecycle.as_raw_fd()),
        ) {
            eprintln!("freebsd-flatpak secure-launch child: {error:#}");
        }
        unsafe { libc::_exit(127) };
    }
    drop(child_lifecycle);
    drop(fds);
    let mut child_wait_started = false;
    let result = (|| {
        let (jail, owner) = receive_nested_jail_lifecycle(parent_lifecycle.as_raw_fd())?;
        if let Err(error) = clear_nested_jail_persistence(jail) {
            unsafe { jail_remove(jail) };
            return Err(error);
        }
        if let Err(error) = write_all_fd(parent_lifecycle.as_raw_fd(), &[1]) {
            unsafe { jail_remove(jail) };
            return Err(error);
        }
        child_wait_started = true;
        let status = wait_for_nested_child(child)?;
        drop(owner);
        write_status(fd, status as u32)
    })();
    if result.is_err() && !child_wait_started {
        unsafe { libc::kill(child, libc::SIGKILL) };
        let _ = wait_for_nested_child(child);
    }
    result
}

fn wait_for_nested_child(child: libc::pid_t) -> Result<i32> {
    let mut status = 0;
    loop {
        if unsafe { libc::waitpid(child, &mut status, 0) } >= 0 {
            return Ok(status);
        }
        if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return Err(io::Error::last_os_error()).context("wait nested sandbox child");
        }
    }
}

fn account_groups(uid: libc::uid_t, primary_gid: libc::gid_t) -> Result<HashSet<u32>> {
    unsafe extern "C" {
        fn getgrouplist(
            name: *const libc::c_char,
            basegid: libc::gid_t,
            groups: *mut libc::gid_t,
            ngroups: *mut libc::c_int,
        ) -> libc::c_int;
    }
    let entry = unsafe { libc::getpwuid(uid) };
    if entry.is_null() {
        bail!("socket peer has no passwd entry");
    }
    let mut count: libc::c_int = 16;
    let mut groups = vec![0 as libc::gid_t; count as usize];
    loop {
        let result = unsafe {
            getgrouplist(
                (*entry).pw_name,
                primary_gid,
                groups.as_mut_ptr(),
                &mut count,
            )
        };
        if result >= 0 {
            break;
        }
        if count <= 0 || count as usize > 1024 {
            bail!("invalid socket peer group list");
        }
        groups.resize(count as usize, 0);
    }
    let mut allowed = groups.into_iter().collect::<HashSet<_>>();
    allowed.insert(primary_gid);
    Ok(allowed)
}

fn map_nested_fds(targets: &[i32], sources: &[OwnedFd]) -> Result<()> {
    let minimum = targets
        .iter()
        .copied()
        .max()
        .and_then(|fd| fd.checked_add(1))
        .unwrap_or(3)
        .max(3);
    let duplicates = sources
        .iter()
        .map(|source| {
            let fd = unsafe { libc::fcntl(source.as_raw_fd(), libc::F_DUPFD_CLOEXEC, minimum) };
            if fd < 0 {
                return Err(io::Error::last_os_error())
                    .context("duplicate nested mapped descriptor");
            }
            Ok(unsafe { OwnedFd::from_raw_fd(fd) })
        })
        .collect::<Result<Vec<_>>>()?;
    for (target, source) in targets.iter().zip(duplicates) {
        if unsafe { libc::dup2(source.as_raw_fd(), *target) } < 0
            || unsafe { libc::fcntl(*target, libc::F_SETFD, 0) } < 0
        {
            return Err(io::Error::last_os_error()).context("map nested descriptor");
        }
    }
    Ok(())
}

fn exit_from_wait_status(status: u32) -> Result<()> {
    let status = status as i32;
    if libc::WIFEXITED(status) {
        std::process::exit(libc::WEXITSTATUS(status));
    }
    if libc::WIFSIGNALED(status) {
        unsafe {
            libc::signal(libc::WTERMSIG(status), libc::SIG_DFL);
            libc::raise(libc::WTERMSIG(status));
        }
    }
    std::process::exit(127)
}

fn peer_credentials(fd: RawFd) -> Result<(libc::uid_t, libc::gid_t)> {
    let mut uid = 0;
    let mut gid = 0;
    if unsafe { libc::getpeereid(fd, &mut uid, &mut gid) } != 0 {
        return Err(io::Error::last_os_error()).context("read nested socket credentials");
    }
    Ok((uid, gid))
}
fn take_wire_u32(payload: &[u8], offset: &mut usize) -> Result<u32> {
    let end = offset.checked_add(4).context("nested request overflow")?;
    let bytes = payload
        .get(*offset..end)
        .context("truncated nested request")?;
    *offset = end;
    Ok(u32::from_be_bytes(bytes.try_into().expect("u32 bytes")))
}
fn receive_nested_request(fd: RawFd) -> Result<(Vec<OsString>, Vec<OwnedFd>)> {
    let mut payload = vec![0u8; MAX_NESTED_PACKET];
    let mut control = vec![
        0u8;
        unsafe {
            libc::CMSG_SPACE((MAX_MAPPED_FDS * std::mem::size_of::<RawFd>()) as _) as usize
        }
    ];
    let mut iov = libc::iovec {
        iov_base: payload.as_mut_ptr().cast(),
        iov_len: payload.len(),
    };
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = control.len() as _;
    let count = unsafe { libc::recvmsg(fd, &mut message, 0) };
    if count <= 0 {
        return Err(io::Error::last_os_error()).context("receive nested request");
    }
    if message.msg_flags & (libc::MSG_TRUNC | libc::MSG_CTRUNC) != 0 {
        bail!("truncated nested request");
    }
    payload.truncate(count as usize);
    let mut fds = Vec::new();
    let cmsg = unsafe { libc::CMSG_FIRSTHDR(&message) };
    if !cmsg.is_null() {
        if unsafe {
            (*cmsg).cmsg_level != libc::SOL_SOCKET
                || (*cmsg).cmsg_type != libc::SCM_RIGHTS
                || (*cmsg).cmsg_len < libc::CMSG_LEN(0) as _
                || !libc::CMSG_NXTHDR(&message, cmsg).is_null()
        } {
            unsafe { close_nested_control_fds(&message) };
            bail!("invalid descriptor transport");
        }
        let bytes = unsafe { (*cmsg).cmsg_len as usize - libc::CMSG_LEN(0) as usize };
        if bytes % std::mem::size_of::<RawFd>() != 0
            || bytes / std::mem::size_of::<RawFd>() > MAX_MAPPED_FDS
        {
            unsafe { close_nested_control_fds(&message) };
            bail!("invalid descriptor count");
        }
        for fd in unsafe {
            std::slice::from_raw_parts(
                libc::CMSG_DATA(cmsg).cast::<RawFd>(),
                bytes / std::mem::size_of::<RawFd>(),
            )
        } {
            if *fd < 0 {
                unsafe { close_nested_control_fds(&message) };
                bail!("invalid descriptor");
            }
            if unsafe { libc::fcntl(*fd, libc::F_SETFD, libc::FD_CLOEXEC) } != 0 {
                unsafe { close_nested_control_fds(&message) };
                return Err(io::Error::last_os_error()).context("secure received descriptor");
            }
            fds.push(unsafe { OwnedFd::from_raw_fd(*fd) });
        }
    }
    let mut offset = 0;
    if take_wire_u32(&payload, &mut offset)? != 0x4653_4C4A {
        bail!("invalid nested request");
    }
    let args_count = take_wire_u32(&payload, &mut offset)? as usize;
    if args_count == 0 || args_count > MAX_ARGUMENTS + MAX_ENVIRONMENT + 16 {
        bail!("invalid nested argument count");
    }
    let mut args = Vec::with_capacity(args_count);
    for _ in 0..args_count {
        let length = take_wire_u32(&payload, &mut offset)? as usize;
        let end = offset
            .checked_add(length)
            .context("nested argument overflow")?;
        let value = payload
            .get(offset..end)
            .context("truncated nested argument")?;
        if value.contains(&0) {
            bail!("invalid nested argument");
        }
        args.push(OsString::from_vec(value.to_vec()));
        offset = end;
    }
    if offset != payload.len() {
        bail!("trailing nested request data");
    }
    Ok((args, fds))
}

unsafe fn close_nested_control_fds(message: &libc::msghdr) {
    let mut current = libc::CMSG_FIRSTHDR(message);
    while !current.is_null() {
        if (*current).cmsg_level == libc::SOL_SOCKET
            && (*current).cmsg_type == libc::SCM_RIGHTS
            && (*current).cmsg_len >= libc::CMSG_LEN(0)
        {
            let bytes = ((*current).cmsg_len - libc::CMSG_LEN(0)) as usize;
            if bytes.is_multiple_of(std::mem::size_of::<RawFd>()) {
                let values = libc::CMSG_DATA(current).cast::<RawFd>();
                for index in 0..bytes / std::mem::size_of::<RawFd>() {
                    let value = *values.add(index);
                    if value >= 0 {
                        libc::close(value);
                    }
                }
            }
        }
        current = libc::CMSG_NXTHDR(message, current);
    }
}

fn nested_sockaddr() -> Result<(libc::sockaddr_un, libc::socklen_t)> {
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    let bytes = NESTED_DAEMON_SOCKET.as_bytes();
    if bytes.len() >= address.sun_path.len() {
        bail!("nested daemon socket path too long");
    }
    address.sun_family = libc::AF_UNIX as _;
    address.sun_len = (std::mem::size_of::<libc::sa_family_t>() + bytes.len() + 1) as _;
    for (slot, byte) in address.sun_path.iter_mut().zip(bytes) {
        *slot = *byte as _;
    }
    Ok((address, address.sun_len as _))
}
fn nested_socket() -> Result<OwnedFd> {
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error()).context("create nested daemon socket");
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}
fn connect_nested_socket() -> Result<OwnedFd> {
    let fd = nested_socket()?;
    let (address, length) = nested_sockaddr()?;
    if unsafe {
        libc::connect(
            fd.as_raw_fd(),
            (&address as *const libc::sockaddr_un).cast(),
            length,
        )
    } != 0
    {
        return Err(io::Error::last_os_error()).context("connect nested daemon");
    }
    Ok(fd)
}
fn bind_nested_socket() -> Result<OwnedFd> {
    let directory = Path::new("/var/run/freebsd-flatpak");
    fs::create_dir_all(directory)?;
    let metadata = fs::metadata(directory)?;
    if metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
        bail!("unsafe nested daemon directory");
    }
    let _ = fs::remove_file(NESTED_DAEMON_SOCKET);
    let fd = nested_socket()?;
    let (address, length) = nested_sockaddr()?;
    if unsafe {
        libc::bind(
            fd.as_raw_fd(),
            (&address as *const libc::sockaddr_un).cast(),
            length,
        )
    } != 0
    {
        return Err(io::Error::last_os_error()).context("bind nested daemon");
    }
    let path = std::ffi::CString::new(NESTED_DAEMON_SOCKET)?;
    if unsafe { libc::chmod(path.as_ptr(), 0o666) } != 0
        || unsafe { libc::listen(fd.as_raw_fd(), 16) } != 0
    {
        return Err(io::Error::last_os_error()).context("activate nested daemon");
    }
    Ok(fd)
}
fn duplicate_fd(fd: RawFd) -> Result<OwnedFd> {
    let fd = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
    if fd < 0 {
        return Err(io::Error::last_os_error()).context("duplicate mapped descriptor");
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}
fn nested_payload(args: &[OsString]) -> Result<Vec<u8>> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&0x4653_4C4Au32.to_be_bytes());
    payload.extend_from_slice(&(args.len() as u32).to_be_bytes());
    for arg in args {
        let value = arg.as_bytes();
        if value.contains(&0) || value.len() > u32::MAX as usize {
            bail!("invalid nested argument");
        }
        payload.extend_from_slice(&(value.len() as u32).to_be_bytes());
        payload.extend_from_slice(value);
    }
    if payload.len() > MAX_NESTED_PACKET {
        bail!("nested request too large");
    }
    Ok(payload)
}
fn send_nested_request(fd: RawFd, args: &[OsString], fds: &[OwnedFd]) -> Result<()> {
    let payload = nested_payload(args)?;
    let mut iov = libc::iovec {
        iov_base: payload.as_ptr().cast_mut().cast(),
        iov_len: payload.len(),
    };
    let mut control = vec![
        0u8;
        if fds.is_empty() {
            0
        } else {
            unsafe { libc::CMSG_SPACE((fds.len() * std::mem::size_of::<RawFd>()) as _) as usize }
        }
    ];
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    if !fds.is_empty() {
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = control.len() as _;
        let cmsg = unsafe { libc::CMSG_FIRSTHDR(&message) };
        if cmsg.is_null() {
            bail!("prepare descriptor transport");
        }
        unsafe {
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = libc::CMSG_LEN((fds.len() * std::mem::size_of::<RawFd>()) as _) as _;
            std::ptr::copy_nonoverlapping(
                fds.as_ptr().cast::<RawFd>(),
                libc::CMSG_DATA(cmsg).cast(),
                fds.len(),
            );
        }
    }
    if unsafe { libc::sendmsg(fd, &message, 0) } != payload.len() as isize {
        return Err(io::Error::last_os_error()).context("send nested request");
    }
    Ok(())
}
fn read_status(fd: RawFd) -> Result<u32> {
    let mut bytes = [0; 4];
    read_exact(fd, &mut bytes)?;
    Ok(u32::from_be_bytes(bytes))
}
fn write_status(fd: RawFd, status: u32) -> Result<()> {
    write_all(fd, &status.to_be_bytes())
}
fn read_exact(fd: RawFd, bytes: &mut [u8]) -> Result<()> {
    let mut done = 0;
    while done < bytes.len() {
        let count =
            unsafe { libc::read(fd, bytes[done..].as_mut_ptr().cast(), bytes.len() - done) };
        if count == 0 {
            bail!("nested daemon disconnected");
        }
        if count < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(io::Error::last_os_error()).context("read nested response");
        }
        done += count as usize;
    }
    Ok(())
}
fn write_all(fd: RawFd, bytes: &[u8]) -> Result<()> {
    let mut done = 0;
    while done < bytes.len() {
        let count = unsafe {
            libc::send(
                fd,
                bytes[done..].as_ptr().cast(),
                bytes.len() - done,
                libc::MSG_NOSIGNAL,
            )
        };
        if count < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(io::Error::last_os_error()).context("write nested response");
        }
        done += count as usize;
    }
    Ok(())
}
#[cfg(test)]
#[path = "tests/secure_launch.rs"]
mod tests;
