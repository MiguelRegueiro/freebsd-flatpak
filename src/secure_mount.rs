use anyhow::{bail, Context, Result};
use std::env;
use std::ffi::{CStr, CString, OsStr, OsString};
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

pub(crate) fn recover_orphaned_document_unmount_command(
    chroots: &Path,
    chroots_identity: (u64, u64),
    mountpoint: &Path,
    force: bool,
) -> Result<Command> {
    if !mountpoint.is_absolute() {
        bail!("orphaned document mountpoint must be absolute");
    }
    let mut command = privileged_helper_command()?;
    command
        .arg("recover-orphaned-document-unmount")
        .arg(chroots)
        .arg(chroots_identity.0.to_string())
        .arg(chroots_identity.1.to_string())
        .arg(mountpoint)
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
    if operation == "recover-orphaned-document-unmount" {
        return recover_orphaned_document_unmount(&mut args, caller_uid);
    }
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
            let source_fd = open_absolute_entry(Path::new(&source))
                .with_context(|| format!("open nullfs source {}", Path::new(&source).display()))?;
            if let Some(expected_source) = expected_source {
                expected_source.verify(source_fd.as_raw_fd(), "nullfs source")?;
            }
            let source_type = file_type(&source_fd)?;
            let source_is_directory = source_type == libc::S_IFDIR;
            if !source_is_directory && source_type != libc::S_IFREG {
                bail!("nullfs source is not a regular file or directory");
            }
            if source_is_directory {
                let target = chase_and_mkdir(
                    &root_fd,
                    Path::new(&target_relative),
                    0o755,
                    root_identity.uid,
                    root_identity.gid,
                )
                .with_context(|| {
                    format!(
                        "prepare mount target {}",
                        Path::new(&target_relative).display()
                    )
                })?;
                nmount_nullfs(Path::new(&source), &target, &CString::new(".")?, read_only)?;
            } else {
                nmount_regular_file_nullfs(
                    Path::new(&source),
                    Path::new(&root),
                    &root_fd,
                    Path::new(&target_relative),
                    root_identity.uid,
                    root_identity.gid,
                    read_only,
                )
                .with_context(|| {
                    format!(
                        "prepare mount target {}",
                        Path::new(&target_relative).display()
                    )
                })?;
            }
        }
        operation if operation == "tmpfs" => {
            let options = required_arg(&mut args, "tmpfs options")?;
            reject_extra_args(args)?;
            let target = chase_and_mkdir(
                &root_fd,
                Path::new(&target_relative),
                0o755,
                root_identity.uid,
                root_identity.gid,
            )
            .with_context(|| {
                format!(
                    "prepare mount target {}",
                    Path::new(&target_relative).display()
                )
            })?;
            nmount_tmpfs(&root_fd, Path::new(&target_relative), &target, &options)?;
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
            let target = chase_and_mkdir(
                &root_fd,
                Path::new(&target_relative),
                0o755,
                root_identity.uid,
                root_identity.gid,
            )
            .with_context(|| {
                format!(
                    "prepare mount target {}",
                    Path::new(&target_relative).display()
                )
            })?;
            nmount_special(&target, fs_type.to_str().unwrap(), source.to_str().unwrap())?;
        }
        operation if operation == "unmount" => {
            let force = match required_arg(&mut args, "unmount mode")?.to_str() {
                Some("normal") => false,
                Some("force") => true,
                _ => bail!("invalid secure unmount mode"),
            };
            reject_extra_args(args)?;
            // Cleanup never creates a target. Validate every parent as a
            // directory and permit a final regular file or directory.
            let target = validate_relative_mount_target(&root_fd, Path::new(&target_relative))
                .with_context(|| {
                    format!(
                        "prepare mount target {}",
                        Path::new(&target_relative).display()
                    )
                })?;
            match file_type(&target)? {
                libc::S_IFREG => unmount_regular_file_target(
                    target,
                    &Path::new(&root).join(&target_relative),
                    force,
                )?,
                libc::S_IFDIR => {
                    drop(target);
                    unmount_directory_target(&root_fd, Path::new(&target_relative), force)?;
                }
                _ => unreachable!("mount target was validated as a file or directory"),
            }
        }
        _ => bail!("unsupported secure mount operation"),
    }
    Ok(())
}

fn recover_orphaned_document_unmount(
    args: &mut impl Iterator<Item = OsString>,
    caller_uid: libc::uid_t,
) -> Result<()> {
    let chroots = required_arg(args, "chroots root")?;
    let expected_chroots = FileIdentity::parse(
        &required_arg(args, "chroots root device")?,
        &required_arg(args, "chroots root inode")?,
    )?;
    let mountpoint = required_arg(args, "orphaned document mountpoint")?;
    let force = match required_arg(args, "unmount mode")?.to_str() {
        Some("normal") => false,
        Some("force") => true,
        _ => bail!("invalid secure unmount mode"),
    };
    reject_extra_args(args)?;

    let chroots_path = Path::new(&chroots);
    let chroots_fd = open_absolute_dir(chroots_path)
        .with_context(|| format!("open chroots root {}", chroots_path.display()))?;
    expected_chroots.verify(chroots_fd.as_raw_fd(), "chroots root")?;
    validate_orphaned_document_mountpoint(
        chroots_path,
        &chroots_fd,
        Path::new(&mountpoint),
        caller_uid,
    )?;
    let filesystem = mounted_filesystem_named(Path::new(&mountpoint))?;
    if filesystem.fs_type != "nullfs" {
        bail!("orphaned document mount is not nullfs");
    }
    unmount_filesystem(filesystem.fsid, force)
}
fn validate_orphaned_document_mountpoint(
    chroots: &Path,
    chroots_fd: &OwnedFd,
    mountpoint: &Path,
    caller_uid: libc::uid_t,
) -> Result<()> {
    let canonical_chroots = fs::canonicalize(chroots)
        .with_context(|| format!("canonicalize chroots root {}", chroots.display()))?;
    if canonical_chroots.file_name() != Some(OsStr::new("chroots")) {
        bail!("chroots root is not a freebsd-flatpak chroots directory");
    }
    let runtime = canonical_chroots
        .parent()
        .context("chroots root has no runtime parent")?;
    let runtime_metadata = fs::metadata(runtime).context("inspect sandbox runtime root")?;
    let chroots_identity = FileIdentity::from_fd(chroots_fd.as_raw_fd())?;
    if runtime_metadata.uid() != caller_uid
        || runtime_metadata.mode() & 0o077 != 0
        || chroots_identity.uid != caller_uid
    {
        bail!("chroots root is not private to caller");
    }
    if !mountpoint.is_absolute()
        || mountpoint
            .components()
            .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        bail!("orphaned document mountpoint is not an absolute normalized path");
    }
    let relative = mountpoint
        .strip_prefix(&canonical_chroots)
        .context("orphaned document mountpoint is outside chroots root")?;
    let parts: Vec<_> = relative.components().collect();
    let [Component::Normal(app), Component::Normal(instance), Component::Normal(run), Component::Normal(user), Component::Normal(uid), Component::Normal(doc), Component::Normal(_grant), Component::Normal(_file)] =
        parts.as_slice()
    else {
        bail!("orphaned mountpoint is not a regular-file document grant");
    };
    if *run != OsStr::new("run")
        || *user != OsStr::new("user")
        || *doc != OsStr::new("doc")
        || uid.to_str().and_then(|uid| uid.parse::<libc::uid_t>().ok()) != Some(caller_uid)
    {
        bail!("orphaned mountpoint is not a caller document grant");
    }
    let instance_path = Path::new(app).join(instance);
    match open_relative_dir(chroots_fd, &instance_path) {
        Ok(_) => bail!("refusing recovery for an existing sandbox instance"),
        Err(error)
            if error.chain().any(|cause| {
                cause
                    .downcast_ref::<io::Error>()
                    .is_some_and(|error| error.raw_os_error() == Some(libc::ENOENT))
            }) =>
        {
            Ok(())
        }
        Err(error) => Err(error).context("inspect orphaned sandbox instance"),
    }
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

fn duplicate_dir(fd: &OwnedFd) -> Result<OwnedFd> {
    // SAFETY: fcntl duplicates a valid descriptor on success.
    let duplicate = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if duplicate < 0 {
        return Err(io::Error::last_os_error()).context("duplicate directory descriptor");
    }
    // SAFETY: duplicate was returned by fcntl and is uniquely owned.
    Ok(unsafe { OwnedFd::from_raw_fd(duplicate) })
}

fn open_absolute_entry(path: &Path) -> Result<OwnedFd> {
    if !path.is_absolute() {
        bail!("descriptor-anchored path must be absolute");
    }
    let parent = path.parent().context("absolute path has no parent")?;
    let name = path
        .file_name()
        .context("absolute path has no final component")?;
    let parent = open_absolute_dir(parent)?;
    open_child_entry(&parent, name)
}

fn file_type(fd: &OwnedFd) -> Result<libc::mode_t> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: stat points to writable storage for one libc::stat.
    if unsafe { libc::fstat(fd.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error()).context("fstat mount path");
    }
    // SAFETY: fstat initialized the structure on success.
    Ok(unsafe { stat.assume_init() }.st_mode & libc::S_IFMT)
}

fn prepare_nullfs_parent(
    root: &OwnedFd,
    target_relative: &Path,
    uid: libc::uid_t,
    gid: libc::gid_t,
) -> Result<(OwnedFd, CString)> {
    validate_relative(target_relative)?;
    let name = c_string(
        target_relative
            .file_name()
            .context("mount target has no final component")?,
    )?;
    let parent = match target_relative.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => {
            chase_and_mkdir(root, parent, 0o755, uid, gid)?
        }
        _ => duplicate_dir(root)?,
    };
    Ok((parent, name))
}

fn open_or_create_regular_file(
    parent: &OwnedFd,
    name: &CString,
    uid: libc::uid_t,
    gid: libc::gid_t,
) -> Result<OwnedFd> {
    let flags = libc::O_RDWR
        | libc::O_CREAT
        | libc::O_EXCL
        | libc::O_CLOEXEC
        | libc::O_NOFOLLOW
        | libc::O_RESOLVE_BENEATH;
    // SAFETY: parent is a valid directory fd and name is NUL-terminated.
    let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags, 0o600) };
    if fd >= 0 {
        // SAFETY: fd was returned by openat and is uniquely owned.
        let entry = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: the entry was created by this process under parent.
        if unsafe {
            libc::fchownat(
                parent.as_raw_fd(),
                name.as_ptr(),
                uid,
                gid,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err(io::Error::last_os_error()).context("chown mount target");
        }
        return Ok(entry);
    }
    if io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST) {
        return Err(io::Error::last_os_error()).context("create regular-file mount target");
    }
    let entry = open_child_entry_cstr(parent, name)?;
    if file_type(&entry)? != libc::S_IFREG {
        bail!("mount target final component is not a regular file");
    }
    Ok(entry)
}
fn nmount_regular_file_nullfs(
    source: &Path,
    root_path: &Path,
    root: &OwnedFd,
    target_relative: &Path,
    uid: libc::uid_t,
    gid: libc::gid_t,
    read_only: bool,
) -> Result<()> {
    let (parent, name) = prepare_nullfs_parent(root, target_relative, uid, gid)?;
    let protection = protect_regular_file_parent(&parent)?;
    let expected_mountpoint = root_path.join(target_relative);
    match finish_regular_file_mount(
        (|| {
            let target = open_or_create_regular_file(&parent, &name, uid, gid)?;
            if file_type(&target)? != libc::S_IFREG {
                bail!("mount target final component is not a regular file");
            }
            // Keep only the protected parent descriptor for the mount itself.
            // A descriptor to a file mountpoint can make FreeBSD report EBUSY.
            drop(target);
            nmount_nullfs(source, &parent, &name, read_only)
        })(),
        || protection.restore(&parent),
        || rollback_regular_file_mount(&parent, &name, &expected_mountpoint),
    )? {
        RegularFileMountCompletion::Clean => Ok(()),
        RegularFileMountCompletion::NeedsTrackedCleanup(details) => {
            // Returning success intentionally makes every caller retain this
            // mount in its ownership list. The helper cannot safely report an
            // error here because nmount may still exist after rollback failed.
            eprintln!(
                "secure-mount: regular-file mount needs tracked cleanup after parent restoration failed: {details}"
            );
            Ok(())
        }
    }
}

#[derive(Debug)]
enum RegularFileMountCompletion {
    Clean,
    NeedsTrackedCleanup(String),
}

fn finish_regular_file_mount(
    mount_result: Result<()>,
    restore: impl Fn() -> Result<()>,
    rollback: impl FnOnce() -> Result<()>,
) -> Result<RegularFileMountCompletion> {
    match mount_result {
        Err(mount_error) => match restore() {
            Ok(()) => Err(mount_error),
            Err(restore_error) => Err(mount_error.context(format!(
                "regular-file mount failed and parent restoration also failed: {restore_error:#}"
            ))),
        },
        Ok(()) => match restore() {
            Ok(()) => Ok(RegularFileMountCompletion::Clean),
            Err(restore_error) => {
                let rollback_result = rollback();
                let final_restore = restore();
                match (rollback_result, final_restore) {
                    (Ok(()), Ok(())) => Err(restore_error.context(
                        "parent restoration failed after regular-file mount; mount was rolled back",
                    )),
                    (Err(rollback_error), final_restore) => {
                        Ok(RegularFileMountCompletion::NeedsTrackedCleanup(format!(
                            "parent restoration failed after regular-file mount: {restore_error:#}; \\
                             rollback failed: {rollback_error:#}; final parent restoration result: {}",
                            describe_result(final_restore),
                        )))
                    }
                    (Ok(()), Err(final_restore)) => bail!(
                        "parent restoration failed after regular-file mount; mount was rolled back; \\
                         final parent restoration result: {final_restore:#}",
                    ),
                }
            }
        },
    }
}

fn describe_result(result: Result<()>) -> String {
    match result {
        Ok(()) => "ok".to_string(),
        Err(error) => format!("{error:#}"),
    }
}

fn rollback_regular_file_mount(
    parent: &OwnedFd,
    name: &CString,
    expected_mountpoint: &Path,
) -> Result<()> {
    let target =
        open_child_entry_cstr(parent, name).context("open regular-file mount for rollback")?;
    unmount_regular_file_target(target, expected_mountpoint, true)
        .context("rollback regular-file mount")
}

#[derive(Clone, Copy)]
struct DirectoryAttributes {
    mode: libc::mode_t,
    uid: libc::uid_t,
    gid: libc::gid_t,
}

#[derive(Clone, Copy)]
enum RegularFileParentProtection {
    ReadOnly,
    Locked(DirectoryAttributes),
}

impl RegularFileParentProtection {
    fn restore(self, parent: &OwnedFd) -> Result<()> {
        let Self::Locked(attributes) = self else {
            return Ok(());
        };
        restore_directory_attributes(parent, attributes)
    }
}

fn protect_regular_file_parent(parent: &OwnedFd) -> Result<RegularFileParentProtection> {
    let attributes = directory_attributes(parent)?;
    // Removing write bits alone is not a lock: the directory owner can chmod
    // them back. Temporarily transfer ownership to root as well, so the
    // untrusted caller cannot replace the final path component before nmount.
    if unsafe { libc::fchown(parent.as_raw_fd(), 0, 0) } != 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EROFS) && mount_is_read_only(parent)? {
            return Ok(RegularFileParentProtection::ReadOnly);
        }
        return Err(error).context("chown regular-file mount parent");
    }
    if let Err(lock_error) = set_directory_mode(parent, attributes.mode & !0o222) {
        return match restore_directory_attributes(parent, attributes) {
            Ok(()) => Err(lock_error).context("lock regular-file mount parent"),
            Err(restore_error) => Err(lock_error.context(format!(
                "lock regular-file mount parent; restoration also failed: {restore_error:#}"
            ))),
        };
    }
    Ok(RegularFileParentProtection::Locked(attributes))
}

fn directory_attributes(fd: &OwnedFd) -> Result<DirectoryAttributes> {
    if file_type(fd)? != libc::S_IFDIR {
        bail!("mount target parent is not a directory");
    }
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: stat points to writable storage for one libc::stat.
    if unsafe { libc::fstat(fd.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error()).context("fstat mount target parent");
    }
    // SAFETY: fstat initialized the structure on success.
    let stat = unsafe { stat.assume_init() };
    Ok(DirectoryAttributes {
        mode: stat.st_mode & 0o7777,
        uid: stat.st_uid,
        gid: stat.st_gid,
    })
}

fn restore_directory_attributes(parent: &OwnedFd, attributes: DirectoryAttributes) -> Result<()> {
    // Restore ownership while the temporary mode still denies writes. Only
    // then restore the exact mode: fchown can clear set-id bits on FreeBSD.
    if unsafe { libc::fchown(parent.as_raw_fd(), attributes.uid, attributes.gid) } != 0 {
        return Err(io::Error::last_os_error()).context("restore regular-file mount parent owner");
    }
    set_directory_mode(parent, attributes.mode)
}

fn mount_is_read_only(fd: &OwnedFd) -> Result<bool> {
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: stat points to writable storage for one libc::statfs.
    if unsafe { libc::fstatfs(fd.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error()).context("fstatfs mount target parent");
    }
    // SAFETY: fstatfs initialized the structure on success.
    Ok(unsafe { stat.assume_init() }.f_flags & libc::MNT_RDONLY as u64 != 0)
}

fn set_directory_mode(fd: &OwnedFd, mode: libc::mode_t) -> Result<()> {
    // SAFETY: fd is a live directory descriptor and mode came from fstat.
    if unsafe { libc::fchmod(fd.as_raw_fd(), mode) } != 0 {
        return Err(io::Error::last_os_error()).context("chmod mount target parent");
    }
    Ok(())
}

fn open_relative_dir(start: &OwnedFd, path: &Path) -> Result<OwnedFd> {
    validate_relative(path)?;
    let mut current = duplicate_dir(start)?;
    for component in path.components() {
        let Component::Normal(name) = component else {
            bail!("mount target is not normalized");
        };
        current = open_child_dir(&current, name)?;
    }
    Ok(current)
}

fn validate_relative_mount_target(start: &OwnedFd, path: &Path) -> Result<OwnedFd> {
    validate_relative(path)?;
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => open_relative_dir(start, parent)?,
        _ => duplicate_dir(start)?,
    };
    let name = path
        .file_name()
        .context("mount target has no final component")?;
    let target = open_child_entry(&parent, name)?;
    match file_type(&target)? {
        libc::S_IFDIR | libc::S_IFREG => Ok(target),
        _ => bail!("mount target final component is not a regular file or directory"),
    }
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

fn open_child_entry(parent: &OwnedFd, name: &OsStr) -> Result<OwnedFd> {
    open_child_entry_cstr(parent, &c_string(name)?)
}

fn open_child_entry_cstr(parent: &OwnedFd, name: &CString) -> Result<OwnedFd> {
    let flags = libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_RESOLVE_BENEATH;
    // SAFETY: parent is a valid directory fd and name is NUL-terminated.
    let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if fd < 0 {
        return Err(io::Error::last_os_error()).context("openat mount target final component");
    }
    // SAFETY: fd was returned by openat and is uniquely owned.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn c_string(value: &OsStr) -> Result<CString> {
    CString::new(value.as_bytes()).context("path contains a NUL byte")
}

fn nmount_nullfs(
    source: &Path,
    target_parent: &OwnedFd,
    target_name: &CString,
    read_only: bool,
) -> Result<()> {
    anchor_mount_target(target_parent)?;
    let source_path = c_string(source.as_os_str())?;
    let mut options = MountOptions::new();
    options.value("fstype", "nullfs")?;
    options.value_cstr(CString::new("fspath")?, target_name.clone());
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

#[derive(Clone, Copy)]
struct FileSystemId([i32; 2]);

struct MountedFilesystem {
    fsid: FileSystemId,
    fs_type: String,
}

fn unmount_regular_file_target(
    target: OwnedFd,
    expected_mountpoint: &Path,
    force: bool,
) -> Result<()> {
    let filesystem = mounted_filesystem(&target, expected_mountpoint)?;
    // fstatfs gave us the filesystem ID through a descriptor-validated
    // mountpoint. Drop the descriptor before unmounting: FreeBSD treats an
    // open descriptor to a file mountpoint as a busy reference.
    drop(target);
    unmount_filesystem(filesystem.fsid, force)
}

fn unmount_directory_target(root: &OwnedFd, target_relative: &Path, force: bool) -> Result<()> {
    validate_relative(target_relative)?;
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

fn mounted_filesystem(target: &OwnedFd, expected_mountpoint: &Path) -> Result<MountedFilesystem> {
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: stat points to writable storage for one libc::statfs.
    if unsafe { libc::fstatfs(target.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error()).context("fstatfs mount target");
    }
    // SAFETY: fstatfs initialized the structure on success.
    let stat = unsafe { stat.assume_init() };
    let mountpoint = statfs_string(&stat.f_mntonname)?;
    if mountpoint.to_bytes() != expected_mountpoint.as_os_str().as_bytes() {
        bail!("mount target is not an exact mountpoint");
    }
    Ok(MountedFilesystem {
        fsid: statfs_fsid(&stat),
        fs_type: statfs_string(&stat.f_fstypename)?
            .to_string_lossy()
            .into_owned(),
    })
}

fn mounted_filesystem_named(expected_mountpoint: &Path) -> Result<MountedFilesystem> {
    let mut mounts: *mut libc::statfs = std::ptr::null_mut();
    // SAFETY: getmntinfo initializes mounts to a process-owned mount table.
    let count = unsafe { libc::getmntinfo(&mut mounts, libc::MNT_NOWAIT) };
    let mounts = getmntinfo_mounts(count, mounts)?;
    let expected = expected_mountpoint.as_os_str().as_bytes();
    let stat = mounts
        .iter()
        .find(|stat| statfs_string(&stat.f_mntonname).is_ok_and(|path| path.to_bytes() == expected))
        .context("orphaned document mount is no longer present")?;
    Ok(MountedFilesystem {
        fsid: statfs_fsid(stat),
        fs_type: statfs_string(&stat.f_fstypename)?
            .to_string_lossy()
            .into_owned(),
    })
}

fn getmntinfo_mounts(count: libc::c_int, mounts: *const libc::statfs) -> Result<Vec<libc::statfs>> {
    // FreeBSD getmntinfo(3) returns zero on failure. Do not manufacture a
    // slice from its output until both the positive count and pointer are
    // validated; an error leaves the pointer unspecified.
    if count <= 0 {
        return Err(io::Error::last_os_error()).context("getmntinfo");
    }
    if mounts.is_null() {
        bail!("getmntinfo returned a null mount table");
    }
    // SAFETY: getmntinfo returned a positive count and non-null pointer to
    // that many initialized statfs entries. Copy the table immediately: the
    // original storage is owned by libc and invalidated by a later call.
    Ok(unsafe { std::slice::from_raw_parts(mounts, count as usize) }.to_vec())
}

fn statfs_string(value: &[libc::c_char]) -> Result<&CStr> {
    // SAFETY: FreeBSD statfs names are NUL-terminated fixed-size arrays.
    Ok(unsafe { CStr::from_ptr(value.as_ptr()) })
}

fn statfs_fsid(stat: &libc::statfs) -> FileSystemId {
    // fsid_t intentionally hides its platform representation in libc. FreeBSD
    // defines it as two int values, exactly the form required by MNT_BYFSID.
    let values =
        unsafe { std::ptr::read((&stat.f_fsid as *const libc::fsid_t).cast::<[i32; 2]>()) };
    FileSystemId(values)
}

fn unmount_filesystem(fsid: FileSystemId, force: bool) -> Result<()> {
    let fsid = CString::new(format!("FSID:{}:{}", fsid.0[0], fsid.0[1]))?;
    let flags = libc::MNT_BYFSID | if force { libc::MNT_FORCE } else { 0 };
    // SAFETY: fsid is encoded as required by unmount(2) with MNT_BYFSID.
    if unsafe { libc::unmount(fsid.as_ptr(), flags) } != 0 {
        return Err(io::Error::last_os_error()).context("unmount by filesystem ID");
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
