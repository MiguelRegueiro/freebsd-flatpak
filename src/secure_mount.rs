use anyhow::{bail, Context, Result};
use std::env;
use std::ffi::{CString, OsStr, OsString};
use std::fs;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path};
use std::process::Command;

const HELPER_PATH: &str = "/usr/local/libexec/freebsd-flatpak/secure-mount";
const PRIVATE_SOURCE_ROOT: &str = ".freebsd-flatpak-mount-sources";

#[derive(Clone, Copy)]
struct FileIdentity {
    device: u64,
    inode: u64,
    uid: libc::uid_t,
    gid: libc::gid_t,
}

impl FileIdentity {
    fn from_fd(fd: RawFd) -> Result<Self> {
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: stat points to writable storage for one libc::stat.
        if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error()).context("fstat");
        }
        // SAFETY: fstat initialized the structure on success.
        let stat = unsafe { stat.assume_init() };
        Ok(Self {
            device: stat.st_dev,
            inode: stat.st_ino,
            uid: stat.st_uid,
            gid: stat.st_gid,
        })
    }

    fn parse(device: &OsStr, inode: &OsStr) -> Result<Self> {
        Ok(Self {
            device: parse_u64(device, "device")?,
            inode: parse_u64(inode, "inode")?,
            uid: 0,
            gid: 0,
        })
    }

    fn verify(self, fd: RawFd, label: &str) -> Result<()> {
        let actual = Self::from_fd(fd)?;
        if self.device != actual.device || self.inode != actual.inode {
            bail!("{label} changed while preparing the mount");
        }
        Ok(())
    }
}

pub(crate) fn nullfs_command(
    root: &Path,
    root_identity: (u64, u64),
    source: &Path,
    source_identity: Option<(u64, u64)>,
    target_relative: &Path,
    read_only: bool,
) -> Result<Command> {
    validate_relative(target_relative)?;
    let (source_device, source_inode) = source_identity
        .map(|identity| (identity.0.to_string(), identity.1.to_string()))
        .unwrap_or_else(|| ("-".to_string(), "-".to_string()));
    let mut command = privileged_helper_command()?;
    command
        .arg("nullfs")
        .arg(root)
        .arg(root_identity.0.to_string())
        .arg(root_identity.1.to_string())
        .arg(target_relative)
        .arg(source)
        .arg(source_device)
        .arg(source_inode)
        .arg(if read_only { "ro" } else { "rw" });
    Ok(command)
}

pub(crate) fn tmpfs_command(
    root: &Path,
    root_identity: (u64, u64),
    target_relative: &Path,
    options: &str,
) -> Result<Command> {
    validate_relative(target_relative)?;
    let mut command = privileged_helper_command()?;
    command
        .arg("tmpfs")
        .arg(root)
        .arg(root_identity.0.to_string())
        .arg(root_identity.1.to_string())
        .arg(target_relative)
        .arg(options);
    Ok(command)
}

pub(crate) fn special_command(
    root: &Path,
    root_identity: (u64, u64),
    target_relative: &Path,
    fs_type: &str,
    source: &str,
) -> Result<Command> {
    validate_relative(target_relative)?;
    let mut command = privileged_helper_command()?;
    command
        .arg("special")
        .arg(root)
        .arg(root_identity.0.to_string())
        .arg(root_identity.1.to_string())
        .arg(target_relative)
        .arg(fs_type)
        .arg(source);
    Ok(command)
}

pub(crate) fn unmount_command(
    root: &Path,
    root_identity: (u64, u64),
    target_relative: &Path,
    force: bool,
) -> Result<Command> {
    validate_relative(target_relative)?;
    let mut command = privileged_helper_command()?;
    command
        .arg("unmount")
        .arg(root)
        .arg(root_identity.0.to_string())
        .arg(root_identity.1.to_string())
        .arg(target_relative)
        .arg(if force { "force" } else { "normal" });
    Ok(command)
}

fn privileged_helper_command() -> Result<Command> {
    // The installed helper is set-user-ID root and validates one narrowly
    // defined mount operation. Invoking it directly keeps app launches
    // independent from interactive doas policy.
    Ok(Command::new(HELPER_PATH))
}

pub(crate) fn run_helper() -> Result<()> {
    let mut args = env::args_os();
    let _program = args.next();
    if unsafe { libc::geteuid() } != 0 {
        bail!("secure mount helper must run as root");
    }
    let caller_uid = unsafe { libc::getuid() };
    if caller_uid == 0 {
        bail!("secure mount helper does not accept root callers");
    }

    let operation = required_arg(&mut args, "mount operation")?;
    let root = required_arg(&mut args, "sandbox root")?;
    let expected_root = FileIdentity::parse(
        &required_arg(&mut args, "sandbox root device")?,
        &required_arg(&mut args, "sandbox root inode")?,
    )?;
    let target_relative = required_arg(&mut args, "sandbox-relative target")?;
    let root_fd = open_absolute_dir(Path::new(&root))
        .with_context(|| format!("open sandbox root {}", Path::new(&root).display()))?;
    expected_root.verify(root_fd.as_raw_fd(), "sandbox root")?;
    let root_identity = FileIdentity::from_fd(root_fd.as_raw_fd())?;
    validate_sandbox_root(Path::new(&root), &root_fd, caller_uid)?;
    // Cleanup must never create a missing path: doing so can leave stale
    // trees behind and turns an idempotent unmount into a mutation. Mount
    // creation is the only operation allowed to materialize target dirs.
    let target_fd = if operation == "unmount" {
        open_relative_dir(&root_fd, Path::new(&target_relative))
    } else {
        chase_and_mkdir(
            &root_fd,
            Path::new(&target_relative),
            0o755,
            root_identity.uid,
            root_identity.gid,
        )
    }
    .with_context(|| {
        format!(
            "prepare mount target {}",
            Path::new(&target_relative).display()
        )
    })?;

    match operation.as_os_str() {
        operation if operation == "nullfs" => {
            let source = required_arg(&mut args, "nullfs source")?;
            let source_device = required_arg(&mut args, "nullfs source device")?;
            let source_inode = required_arg(&mut args, "nullfs source inode")?;
            let expected_source = if source_device == "-" && source_inode == "-" {
                let source_path = Path::new(&source);
                let private_root = Path::new(&root).join(PRIVATE_SOURCE_ROOT);
                if !source_path.starts_with(&private_root) {
                    bail!("unverified nullfs source is outside private mount staging");
                }
                None
            } else {
                Some(FileIdentity::parse(&source_device, &source_inode)?)
            };
            let access = required_arg(&mut args, "nullfs access mode")?;
            let read_only = match access.to_str() {
                Some("ro") => true,
                Some("rw") => false,
                _ => bail!("invalid nullfs access mode"),
            };
            reject_extra_args(args)?;
            let source_fd = open_absolute_dir(Path::new(&source))
                .with_context(|| format!("open nullfs source {}", Path::new(&source).display()))?;
            if let Some(expected_source) = expected_source {
                expected_source.verify(source_fd.as_raw_fd(), "nullfs source")?;
            }
            nmount_nullfs(Path::new(&source), &target_fd, read_only)?;
        }
        operation if operation == "tmpfs" => {
            let options = required_arg(&mut args, "tmpfs options")?;
            reject_extra_args(args)?;
            nmount_tmpfs(&root_fd, Path::new(&target_relative), &target_fd, &options)?;
        }
        operation if operation == "special" => {
            let fs_type = required_arg(&mut args, "special filesystem type")?;
            let source = required_arg(&mut args, "special filesystem source")?;
            reject_extra_args(args)?;
            let valid = matches!(
                (fs_type.to_str(), source.to_str()),
                (Some("devfs"), Some("devfs"))
                    | (Some("fdescfs"), Some("fdescfs"))
                    | (Some("linprocfs"), Some("linprocfs"))
                    | (Some("linsysfs"), Some("linsysfs"))
            );
            if !valid {
                bail!("unsupported secure special mount");
            }
            nmount_special(
                &target_fd,
                fs_type.to_str().unwrap(),
                source.to_str().unwrap(),
            )?;
        }
        operation if operation == "unmount" => {
            let force = match required_arg(&mut args, "unmount mode")?.to_str() {
                Some("normal") => false,
                Some("force") => true,
                _ => bail!("invalid secure unmount mode"),
            };
            reject_extra_args(args)?;
            // FreeBSD considers a mount busy while this helper itself holds a
            // descriptor to its mountpoint. The descriptor has served its
            // validation purpose; release it before the anchored unmount.
            drop(target_fd);
            unmount_target(&root_fd, Path::new(&target_relative), force)?;
        }
        _ => bail!("unsupported secure mount operation"),
    }
    Ok(())
}

fn validate_sandbox_root(root: &Path, root_fd: &OwnedFd, caller_uid: libc::uid_t) -> Result<()> {
    let canonical_root = fs::canonicalize(root)
        .with_context(|| format!("canonicalize sandbox root {}", root.display()))?;
    let app = canonical_root
        .parent()
        .context("sandbox root has no app parent")?;
    let chroots = app.parent().context("sandbox root has no chroots parent")?;
    let runtime = chroots
        .parent()
        .context("sandbox root has no runtime parent")?;
    if chroots.file_name() != Some(OsStr::new("chroots"))
        || canonical_root.file_name().is_none()
        || app.file_name().is_none()
    {
        bail!("sandbox root is not a freebsd-flatpak instance");
    }
    let runtime_metadata = fs::metadata(runtime).context("inspect sandbox runtime root")?;
    let root_identity = FileIdentity::from_fd(root_fd.as_raw_fd())?;
    if runtime_metadata.uid() != caller_uid
        || runtime_metadata.mode() & 0o077 != 0
        || root_identity.uid != caller_uid
    {
        bail!("sandbox root is not private to caller");
    }
    let info = c_string(OsStr::new(".flatpak-info"))?;
    let info_fd = unsafe {
        libc::openat(
            root_fd.as_raw_fd(),
            info.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if info_fd < 0 {
        return Err(io::Error::last_os_error()).context("open sandbox .flatpak-info");
    }
    let info = unsafe { OwnedFd::from_raw_fd(info_fd) };
    let identity = FileIdentity::from_fd(info.as_raw_fd())?;
    let mode = unsafe {
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        if libc::fstat(info.as_raw_fd(), stat.as_mut_ptr()) != 0 {
            return Err(io::Error::last_os_error()).context("inspect sandbox .flatpak-info");
        }
        stat.assume_init().st_mode
    };
    if mode & libc::S_IFMT != libc::S_IFREG || identity.uid != caller_uid {
        bail!("sandbox .flatpak-info is not a caller-owned regular file");
    }
    Ok(())
}

fn required_arg(args: &mut impl Iterator<Item = OsString>, label: &str) -> Result<OsString> {
    args.next().with_context(|| format!("missing {label}"))
}

fn reject_extra_args(mut args: impl Iterator<Item = OsString>) -> Result<()> {
    if args.next().is_some() {
        bail!("unexpected secure mount helper argument");
    }
    Ok(())
}

fn parse_u64(value: &OsStr, label: &str) -> Result<u64> {
    value
        .to_str()
        .with_context(|| format!("{label} is not UTF-8"))?
        .parse()
        .with_context(|| format!("invalid {label}"))
}

fn validate_relative(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("mount target must be a non-empty normalized relative path");
    }
    Ok(())
}

fn open_absolute_dir(path: &Path) -> Result<OwnedFd> {
    if !path.is_absolute() {
        bail!("descriptor-anchored path must be absolute");
    }
    let slash = c_string(OsStr::new("/"))?;
    // SAFETY: slash is NUL-terminated and open returns a new descriptor.
    let fd = unsafe {
        libc::open(
            slash.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error()).context("open filesystem root");
    }
    // SAFETY: fd was returned by open and is uniquely owned.
    let mut current = unsafe { OwnedFd::from_raw_fd(fd) };
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => current = open_child_dir(&current, name)?,
            _ => bail!("absolute path is not normalized"),
        }
    }
    Ok(current)
}

fn chase_and_mkdir(
    start: &OwnedFd,
    path: &Path,
    mode: libc::mode_t,
    uid: libc::uid_t,
    gid: libc::gid_t,
) -> Result<OwnedFd> {
    validate_relative(path)?;
    // SAFETY: fcntl duplicates a valid descriptor on success.
    let fd = unsafe { libc::fcntl(start.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error()).context("duplicate directory descriptor");
    }
    // SAFETY: fd was returned by fcntl and is uniquely owned.
    let mut current = unsafe { OwnedFd::from_raw_fd(fd) };
    for component in path.components() {
        let Component::Normal(name) = component else {
            bail!("mount target is not normalized");
        };
        let name = c_string(name)?;
        // SAFETY: current is a valid directory fd and name is NUL-terminated.
        let created = unsafe { libc::mkdirat(current.as_raw_fd(), name.as_ptr(), mode) } == 0;
        if !created {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EEXIST) {
                return Err(error).context("mkdirat mount target");
            }
        } else {
            // SAFETY: the entry was created by this process under current.
            if unsafe {
                libc::fchownat(
                    current.as_raw_fd(),
                    name.as_ptr(),
                    uid,
                    gid,
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            } != 0
            {
                return Err(io::Error::last_os_error()).context("chown mount target");
            }
        }
        current = open_child_dir_cstr(&current, &name)?;
    }
    Ok(current)
}

fn open_relative_dir(start: &OwnedFd, path: &Path) -> Result<OwnedFd> {
    validate_relative(path)?;
    // SAFETY: fcntl duplicates a valid descriptor on success.
    let fd = unsafe { libc::fcntl(start.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error()).context("duplicate directory descriptor");
    }
    // SAFETY: fd was returned by fcntl and is uniquely owned.
    let mut current = unsafe { OwnedFd::from_raw_fd(fd) };
    for component in path.components() {
        let Component::Normal(name) = component else {
            bail!("mount target is not normalized");
        };
        current = open_child_dir(&current, name)?;
    }
    Ok(current)
}

fn open_child_dir(parent: &OwnedFd, name: &OsStr) -> Result<OwnedFd> {
    open_child_dir_cstr(parent, &c_string(name)?)
}

fn open_child_dir_cstr(parent: &OwnedFd, name: &CString) -> Result<OwnedFd> {
    let flags = libc::O_RDONLY
        | libc::O_DIRECTORY
        | libc::O_CLOEXEC
        | libc::O_NOFOLLOW
        | libc::O_RESOLVE_BENEATH;
    // SAFETY: parent is a valid directory fd and name is NUL-terminated.
    let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if fd < 0 {
        return Err(io::Error::last_os_error()).context("openat directory component");
    }
    // SAFETY: fd was returned by openat and is uniquely owned.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn c_string(value: &OsStr) -> Result<CString> {
    CString::new(value.as_bytes()).context("path contains a NUL byte")
}

fn nmount_nullfs(source: &Path, target: &OwnedFd, read_only: bool) -> Result<()> {
    anchor_mount_target(target)?;
    let source_path = c_string(source.as_os_str())?;
    let mut options = MountOptions::new();
    options.value("fstype", "nullfs")?;
    options.value("fspath", ".")?;
    options.value_cstr(CString::new("target")?, source_path);
    options.flag("nocover")?;
    options.mount(if read_only { libc::MNT_RDONLY } else { 0 })
}

fn nmount_special(target: &OwnedFd, fs_type: &str, source: &str) -> Result<()> {
    anchor_mount_target(target)?;
    let mut options = MountOptions::new();
    options.value("fstype", fs_type)?;
    options.value("fspath", ".")?;
    options.value("from", source)?;
    options.mount(0)
}

fn unmount_target(root: &OwnedFd, target_relative: &Path, force: bool) -> Result<()> {
    validate_relative(target_relative)?;
    // `unmount(".")` from the target itself is always busy because this
    // helper owns that cwd. The target was descriptor-walked above; resolve
    // its normalized relative name from the validated sandbox-root fd instead.
    if unsafe { libc::fchdir(root.as_raw_fd()) } != 0 {
        return Err(io::Error::last_os_error()).context("fchdir sandbox root for unmount");
    }
    let path = c_string(target_relative.as_os_str())?;
    let flags = if force { libc::MNT_FORCE } else { 0 };
    if unsafe { libc::unmount(path.as_ptr(), flags) } != 0 {
        return Err(io::Error::last_os_error()).context("unmount");
    }
    Ok(())
}

fn nmount_tmpfs(
    root: &OwnedFd,
    target_relative: &Path,
    target: &OwnedFd,
    options: &OsStr,
) -> Result<()> {
    anchor_mount_target(target)?;
    let mut mount_options = MountOptions::new();
    mount_options.value("fstype", "tmpfs")?;
    mount_options.value("fspath", ".")?;
    mount_options.value("from", "tmpfs")?;
    mount_options.flag("nocover")?;
    let options = options.to_str().context("tmpfs options are not UTF-8")?;
    let mut requested_mode = None;
    let mut requested_uid = None;
    let mut requested_gid = None;
    for option in options.split(',').filter(|option| !option.is_empty()) {
        let (name, value) = option
            .split_once('=')
            .context("tmpfs option requires a value")?;
        match name {
            "mode" => {
                requested_mode = Some(
                    u32::from_str_radix(value, 8)
                        .context("invalid tmpfs mode")?
                        .try_into()
                        .context("tmpfs mode is out of range")?,
                );
            }
            "uid" | "gid" => {
                let id = value.parse::<u32>().context("invalid tmpfs owner")?;
                if name == "uid" {
                    requested_uid = Some(id);
                } else {
                    requested_gid = Some(id);
                }
            }
            _ => bail!("unsupported tmpfs option"),
        }
        mount_options.value(name, value)?;
    }
    mount_options.mount(0)?;

    // A set-user-ID helper keeps the caller's real uid. tmpfs deliberately
    // ignores uid/gid/mode mount options for non-root real credentials, so
    // apply the explicitly allowlisted metadata through a fresh descriptor
    // rooted at the validated sandbox after the mount is visible there.
    let mounted_root = open_relative_dir(root, target_relative)?;
    if let Some(mode) = requested_mode {
        // SAFETY: mounted_root is a live descriptor for the just-mounted
        // tmpfs root and mode was parsed as an octal mode_t above.
        if unsafe { libc::fchmod(mounted_root.as_raw_fd(), mode) } != 0 {
            return Err(io::Error::last_os_error()).context("chmod tmpfs root");
        }
    }
    if requested_uid.is_some() || requested_gid.is_some() {
        // SAFETY: mounted_root is a live descriptor for the just-mounted
        // tmpfs root. Passing -1 preserves an unspecified owner component.
        if unsafe {
            libc::fchown(
                mounted_root.as_raw_fd(),
                requested_uid.unwrap_or(!0),
                requested_gid.unwrap_or(!0),
            )
        } != 0
        {
            return Err(io::Error::last_os_error()).context("chown tmpfs root");
        }
    }
    Ok(())
}

struct MountOptions {
    strings: Vec<CString>,
    pairs: Vec<(usize, Option<usize>)>,
}

fn anchor_mount_target(target: &OwnedFd) -> Result<()> {
    // SAFETY: target is a live directory descriptor. The helper performs one
    // mount and exits, so its working directory does not need to be restored.
    if unsafe { libc::fchdir(target.as_raw_fd()) } != 0 {
        return Err(io::Error::last_os_error()).context("fchdir mount target");
    }
    Ok(())
}

impl MountOptions {
    fn new() -> Self {
        Self {
            strings: Vec::new(),
            pairs: Vec::new(),
        }
    }

    fn value(&mut self, name: &str, value: &str) -> Result<()> {
        self.value_cstr(CString::new(name)?, CString::new(value)?);
        Ok(())
    }

    fn flag(&mut self, name: &str) -> Result<()> {
        let name = CString::new(name)?;
        let name_index = self.strings.len();
        self.strings.push(name);
        self.pairs.push((name_index, None));
        Ok(())
    }

    fn value_cstr(&mut self, name: CString, value: CString) {
        let name_index = self.strings.len();
        self.strings.push(name);
        let value_index = self.strings.len();
        self.strings.push(value);
        self.pairs.push((name_index, Some(value_index)));
    }

    fn mount(&mut self, flags: libc::c_int) -> Result<()> {
        let mut iov = Vec::with_capacity(self.pairs.len() * 2);
        for (name, value) in &self.pairs {
            iov.push(libc::iovec {
                iov_base: self.strings[*name].as_ptr().cast_mut().cast(),
                iov_len: self.strings[*name].as_bytes_with_nul().len(),
            });
            if let Some(value) = value {
                iov.push(libc::iovec {
                    iov_base: self.strings[*value].as_ptr().cast_mut().cast(),
                    iov_len: self.strings[*value].as_bytes_with_nul().len(),
                });
            } else {
                iov.push(libc::iovec {
                    iov_base: std::ptr::null_mut(),
                    iov_len: 0,
                });
            }
        }
        // SAFETY: every iovec points into self.strings, which remains alive
        // and immovable for the duration of nmount.
        if unsafe { libc::nmount(iov.as_mut_ptr(), iov.len() as libc::c_uint, flags) } != 0 {
            return Err(io::Error::last_os_error()).context("nmount");
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/secure_mount.rs"]
mod tests;
