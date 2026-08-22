use crate::paths::Installation;
use crate::runtime::{self, FlatpakApp, RuntimeGlExtension};
use anyhow::{bail, Context, Result};
use std::fs;
use std::os::unix::fs as unix_fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const DRM_MAJOR: u32 = 226;
const DRM_SYNCOBJ_ERRNO_SHIM_LIB: &str = "libdrm-syncobj-errno-shim.so";
const CHROMIUM_ZYGOTE_DRM_PRELOAD_LIB: &str = "libchromium-zygote-drm-preload.so";
const GRAPHICS_SHIM_SANDBOX_DIR: &str = "/run/host/freebsd-flatpak-poc";
const WAYLAND_DRM_DEVT_SHIM_LIB: &str = "libwayland-drm-devt-shim.so";

#[derive(Debug, Clone)]
pub struct HostGraphics {
    gl: Option<RuntimeGlExtension>,
    drm: Option<DrmSysfsBridge>,
    drm_syncobj_errno_shim: Option<DrmSyncobjErrnoShim>,
    chromium_zygote_drm_preload: Option<ChromiumZygoteDrmPreload>,
    wayland_drm_devt_shim: Option<WaylandDrmDevtShim>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct GraphicsMount {
    host_path: PathBuf,
    sandbox_path: PathBuf,
}

#[derive(Debug, Clone)]
struct DrmSysfsBridge {
    source_root: PathBuf,
    mounts: Vec<GraphicsMount>,
    device: DrmDevice,
}

#[derive(Debug, Clone)]
struct WaylandDrmDevtShim {
    host_dir: PathBuf,
    dev_t_map: String,
}

#[derive(Debug, Clone)]
struct DrmSyncobjErrnoShim {
    host_dir: PathBuf,
}

#[derive(Debug, Clone)]
struct ChromiumZygoteDrmPreload {
    host_dir: PathBuf,
}

#[derive(Debug, Clone)]
struct DrmDevice {
    card_name: String,
    card_minor: u32,
    card_host_dev_t: u64,
    render_name: String,
    render_minor: u32,
    render_host_dev_t: u64,
    pci_slot: String,
    vendor: String,
    device: String,
    class: String,
    revision: String,
    subsystem_vendor: String,
    subsystem_device: String,
    driver: String,
}

#[derive(Debug, Clone)]
struct PciInfo {
    class: String,
    revision: String,
    vendor: String,
    device: String,
    subsystem_vendor: String,
    subsystem_device: String,
}

impl HostGraphics {
    pub fn prepare(paths: &Installation, app: &FlatpakApp, instance_id: &str) -> Result<Self> {
        let mut warnings = Vec::new();
        let gl = runtime::ensure_default_gl_extension(paths, &app.runtime_ref, &app.runtime_dir)?;
        let drm = if gl.is_some() {
            match DrmDevice::detect() {
                Ok(device) => Some(DrmSysfsBridge::prepare(
                    paths,
                    &app.app_id,
                    instance_id,
                    device,
                )?),
                Err(error) => {
                    warnings.push(format!("DRM sysfs bridge disabled: {error:#}"));
                    None
                }
            }
        } else {
            None
        };

        let wayland_drm_devt_shim = if let Some(drm) = &drm {
            match WaylandDrmDevtShim::prepare(paths, &drm.device) {
                Ok(shim) => Some(shim),
                Err(error) => {
                    warnings.push(format!("Wayland DRM dev_t shim disabled: {error:#}"));
                    None
                }
            }
        } else {
            None
        };

        let drm_syncobj_errno_shim = if drm.is_some() {
            match DrmSyncobjErrnoShim::prepare(paths) {
                Ok(shim) => Some(shim),
                Err(error) => {
                    warnings.push(format!("DRM syncobj errno shim disabled: {error:#}"));
                    None
                }
            }
        } else {
            None
        };

        let chromium_zygote_drm_preload = if drm_syncobj_errno_shim.is_some() {
            match ChromiumZygoteDrmPreload::prepare(paths) {
                Ok(shim) => Some(shim),
                Err(error) => {
                    warnings.push(format!("Chromium zygote DRM preload disabled: {error:#}"));
                    None
                }
            }
        } else {
            None
        };

        Ok(Self {
            gl,
            drm,
            drm_syncobj_errno_shim,
            chromium_zygote_drm_preload,
            wayland_drm_devt_shim,
            warnings,
        })
    }

    pub fn extension_refs(&self) -> impl Iterator<Item = &str> {
        self.gl.iter().map(|gl| gl.ref_name())
    }

    pub fn runtime_mounts(&self) -> Vec<GraphicsMount> {
        let mut mounts: Vec<GraphicsMount> = self
            .gl
            .iter()
            .map(|gl| GraphicsMount {
                host_path: gl.checkout_dir.join("files"),
                sandbox_path: PathBuf::from("/usr").join(&gl.runtime_mount_relative),
            })
            .collect();
        let shim_dir = self
            .drm_syncobj_errno_shim
            .as_ref()
            .map(|shim| &shim.host_dir)
            .or_else(|| {
                self.wayland_drm_devt_shim
                    .as_ref()
                    .map(|shim| &shim.host_dir)
            });
        if let Some(host_dir) = shim_dir {
            mounts.push(GraphicsMount {
                host_path: host_dir.clone(),
                sandbox_path: PathBuf::from(GRAPHICS_SHIM_SANDBOX_DIR),
            });
        }
        mounts
    }

    pub fn sysfs_mounts(&self) -> Vec<GraphicsMount> {
        self.drm
            .as_ref()
            .map(|drm| drm.mounts.clone())
            .unwrap_or_default()
    }

    pub fn env(&self) -> Vec<(String, String)> {
        if self.gl.is_none() {
            return Vec::new();
        }

        let gl_root = "/usr/lib/x86_64-linux-gnu/GL/default";
        let mut env = vec![
            (
                "LD_LIBRARY_PATH".to_string(),
                format!(
                    "{gl_root}/lib:/app/lib:/app/lib64:/usr/lib/x86_64-linux-gnu:/usr/lib:/usr/lib64"
                ),
            ),
            (
                "LIBGL_DRIVERS_PATH".to_string(),
                format!("{gl_root}/lib/dri"),
            ),
            (
                "GBM_BACKENDS_PATH".to_string(),
                format!("{gl_root}/lib/gbm"),
            ),
            (
                "__EGL_VENDOR_LIBRARY_DIRS".to_string(),
                format!("{gl_root}/share/glvnd/egl_vendor.d:/usr/share/glvnd/egl_vendor.d"),
            ),
            (
                "__EGL_EXTERNAL_PLATFORM_CONFIG_DIRS".to_string(),
                format!(
                    "/etc/egl/egl_external_platform.d:{gl_root}/egl/egl_external_platform.d:/usr/share/egl/egl_external_platform.d"
                ),
            ),
        ];

        if let Some(icd) = self.drm.as_ref().and_then(|drm| drm.device.vulkan_icd()) {
            let icd_path = format!("{gl_root}/lib/vulkan/icd.d/{icd}");
            env.push(("VK_DRIVER_FILES".to_string(), icd_path.clone()));
            env.push(("VK_ICD_FILENAMES".to_string(), icd_path));
        }
        if let Some(shim) = &self.wayland_drm_devt_shim {
            env.push((
                "FREEBSD_FLATPAK_DRM_DEV_T_MAP".to_string(),
                shim.dev_t_map.clone(),
            ));
        }

        env
    }

    pub fn ld_preload_paths(&self) -> Vec<String> {
        let mut paths = Vec::new();
        if self.drm_syncobj_errno_shim.is_some() {
            paths.push(format!(
                "{GRAPHICS_SHIM_SANDBOX_DIR}/{DRM_SYNCOBJ_ERRNO_SHIM_LIB}"
            ));
        }
        if self.wayland_drm_devt_shim.is_some() {
            paths.push(format!(
                "{GRAPHICS_SHIM_SANDBOX_DIR}/{WAYLAND_DRM_DEVT_SHIM_LIB}"
            ));
        }
        paths
    }

    pub fn zypak_ld_preload_paths(&self) -> Vec<String> {
        let mut paths = Vec::new();
        if self.wayland_drm_devt_shim.is_some() {
            paths.push(format!(
                "{GRAPHICS_SHIM_SANDBOX_DIR}/{WAYLAND_DRM_DEVT_SHIM_LIB}"
            ));
        }
        if self.drm_syncobj_errno_shim.is_some() {
            paths.push(format!(
                "{GRAPHICS_SHIM_SANDBOX_DIR}/{DRM_SYNCOBJ_ERRNO_SHIM_LIB}"
            ));
        }
        if self.chromium_zygote_drm_preload.is_some() {
            paths.push(format!(
                "{GRAPHICS_SHIM_SANDBOX_DIR}/{CHROMIUM_ZYGOTE_DRM_PRELOAD_LIB}"
            ));
        }
        paths
    }

    pub fn describe(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(gl) = &self.gl {
            lines.push(format!(
                "GL extension: {} -> /usr/{}",
                gl.checkout_dir.display(),
                gl.runtime_mount_relative.display()
            ));
            lines.push(format!("GL ref: {}", gl.ref_name));
        }
        if let Some(drm) = &self.drm {
            lines.push(format!(
                "DRM render node: /dev/dri/{} ({:}:{:})",
                drm.device.render_name, DRM_MAJOR, drm.device.render_minor
            ));
            lines.push(format!(
                "DRM render dev_t: host 0x{:x} -> linux 0x{:x}",
                drm.device.render_host_dev_t,
                linux_drm_dev_t(drm.device.render_minor)
            ));
            lines.push(format!(
                "DRM PCI: {} vendor={} device={} driver={}",
                drm.device.pci_slot, drm.device.vendor, drm.device.device, drm.device.driver
            ));
            if let Some(icd) = drm.device.vulkan_icd() {
                lines.push(format!("Vulkan ICD: {icd}"));
            }
            for mount in &drm.mounts {
                lines.push(format!(
                    "sysfs: {} -> {}",
                    mount.host_path.display(),
                    mount.sandbox_path.display()
                ));
            }
        }
        if let Some(shim) = &self.wayland_drm_devt_shim {
            lines.push(format!(
                "Wayland DRM dev_t shim: {} -> {}",
                shim.host_dir.display(),
                GRAPHICS_SHIM_SANDBOX_DIR
            ));
            lines.push(format!("Wayland DRM dev_t map: {}", shim.dev_t_map));
        }
        if let Some(shim) = &self.drm_syncobj_errno_shim {
            lines.push(format!(
                "DRM syncobj errno shim: {} -> {}/{}",
                shim.host_dir.display(),
                GRAPHICS_SHIM_SANDBOX_DIR,
                DRM_SYNCOBJ_ERRNO_SHIM_LIB
            ));
        }
        if let Some(shim) = &self.chromium_zygote_drm_preload {
            lines.push(format!(
                "Chromium zygote DRM preload: {} -> {}/{}",
                shim.host_dir.display(),
                GRAPHICS_SHIM_SANDBOX_DIR,
                CHROMIUM_ZYGOTE_DRM_PRELOAD_LIB
            ));
        }
        lines
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub fn cleanup(&self) -> Result<()> {
        if let Some(drm) = &self.drm {
            if drm.source_root.exists() {
                fs::remove_dir_all(&drm.source_root)
                    .with_context(|| format!("remove {}", drm.source_root.display()))?;
            }
        }
        Ok(())
    }
}

impl GraphicsMount {
    pub fn host_path(&self) -> &Path {
        &self.host_path
    }

    pub fn sandbox_target_relative(&self) -> Result<PathBuf> {
        self.sandbox_path
            .strip_prefix("/")
            .map(Path::to_path_buf)
            .with_context(|| {
                format!(
                    "graphics sandbox path is not absolute: {}",
                    self.sandbox_path.display()
                )
            })
    }
}

impl DrmSysfsBridge {
    fn prepare(
        paths: &Installation,
        app_id: &str,
        instance_id: &str,
        device: DrmDevice,
    ) -> Result<Self> {
        let source_root =
            paths
                .gpu()
                .join(format!("{}-{}", safe_name(app_id), safe_name(instance_id)));
        if source_root.exists() {
            fs::remove_dir_all(&source_root)
                .with_context(|| format!("replace {}", source_root.display()))?;
        }

        let bus = source_root.join("sys-bus");
        let dev_char = source_root.join("sys-dev-char");
        let class_drm = source_root.join("sys-class-drm");
        write_linux_drm_sysfs(&bus, &dev_char, &class_drm, &device)?;

        Ok(Self {
            mounts: vec![
                GraphicsMount {
                    host_path: bus,
                    sandbox_path: PathBuf::from("/sys/bus"),
                },
                GraphicsMount {
                    host_path: dev_char,
                    sandbox_path: PathBuf::from("/sys/dev/char"),
                },
                GraphicsMount {
                    host_path: class_drm,
                    sandbox_path: PathBuf::from("/sys/class/drm"),
                },
            ],
            source_root,
            device,
        })
    }
}

impl WaylandDrmDevtShim {
    fn prepare(paths: &Installation, device: &DrmDevice) -> Result<Self> {
        let helper = ensure_wayland_drm_devt_shim(paths)?;
        let host_dir = helper
            .parent()
            .context("Wayland DRM dev_t shim output path has no parent")?
            .to_path_buf();
        Ok(Self {
            host_dir,
            dev_t_map: device.dev_t_map(),
        })
    }
}

impl DrmSyncobjErrnoShim {
    fn prepare(paths: &Installation) -> Result<Self> {
        let helper = ensure_drm_syncobj_errno_shim(paths)?;
        let host_dir = helper
            .parent()
            .context("DRM syncobj errno shim output path has no parent")?
            .to_path_buf();
        Ok(Self { host_dir })
    }
}

impl ChromiumZygoteDrmPreload {
    fn prepare(paths: &Installation) -> Result<Self> {
        let helper = ensure_chromium_zygote_drm_preload(paths)?;
        let host_dir = helper
            .parent()
            .context("Chromium zygote DRM preload output path has no parent")?
            .to_path_buf();
        Ok(Self { host_dir })
    }
}

impl DrmDevice {
    fn detect() -> Result<Self> {
        let render = first_dri_node("renderD").context("no /dev/dri/renderD* node found")?;
        let card = matching_card_node(&render).context("no matching /dev/dri/card* node found")?;
        let card_host_dev_t = dri_node_dev_t(&card)?;
        let render_host_dev_t = dri_node_dev_t(&render)?;
        let pci_slot = sysctl_value(&format!("hw.dri.{}.busid", card.index))
            .ok()
            .and_then(|value| value.strip_prefix("pci:").map(ToOwned::to_owned))
            .context("could not determine DRM PCI bus id")?;
        let pci_id = sysctl_value(&format!("dev.drm.{}.PCI_ID", render.minor))
            .or_else(|_| sysctl_value(&format!("dev.drm.{}.PCI_ID", card.minor)))
            .context("could not determine DRM PCI vendor/device id")?;
        let (vendor, device_id) = pci_id
            .split_once(':')
            .map(|(vendor, device)| (hex4(vendor), hex4(device)))
            .context("unexpected dev.drm.*.PCI_ID format")?;
        let pci = pciconf_info(&pci_slot).unwrap_or_else(|_| PciInfo {
            class: "0x030000".to_string(),
            revision: "0x00".to_string(),
            vendor: vendor.clone(),
            device: device_id.clone(),
            subsystem_vendor: "0x0000".to_string(),
            subsystem_device: "0x0000".to_string(),
        });
        let driver = sysctl_value(&format!("hw.dri.{}.name", card.index))
            .ok()
            .and_then(|name| name.split_whitespace().next().map(ToOwned::to_owned))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "drm".to_string());

        Ok(Self {
            card_name: card.name,
            card_minor: card.minor,
            card_host_dev_t,
            render_name: render.name,
            render_minor: render.minor,
            render_host_dev_t,
            pci_slot,
            vendor: pci.vendor,
            device: pci.device,
            class: pci.class,
            revision: pci.revision,
            subsystem_vendor: pci.subsystem_vendor,
            subsystem_device: pci.subsystem_device,
            driver,
        })
    }

    fn vulkan_icd(&self) -> Option<&'static str> {
        match trim_hex(&self.vendor) {
            "8086" => Some("intel_icd.x86_64.json"),
            "1002" | "1022" => Some("radeon_icd.x86_64.json"),
            "1af4" => Some("virtio_icd.x86_64.json"),
            _ => None,
        }
    }

    fn dev_t_map(&self) -> String {
        [
            (self.card_host_dev_t, linux_drm_dev_t(self.card_minor)),
            (self.render_host_dev_t, linux_drm_dev_t(self.render_minor)),
        ]
        .into_iter()
        .map(|(host, linux)| format!("0x{host:x}=0x{linux:x}"))
        .collect::<Vec<_>>()
        .join(",")
    }
}

#[derive(Debug, Clone)]
struct DriNode {
    name: String,
    index: u32,
    minor: u32,
}

fn first_dri_node(prefix: &str) -> Result<DriNode> {
    let mut nodes = Vec::new();
    for entry in fs::read_dir("/dev/dri").context("read /dev/dri")? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(number) = name
            .strip_prefix(prefix)
            .and_then(|value| value.parse::<u32>().ok())
        else {
            continue;
        };
        nodes.push(DriNode {
            minor: number,
            index: if prefix == "renderD" {
                number.saturating_sub(128)
            } else {
                number
            },
            name,
        });
    }
    nodes.sort_by_key(|node| node.minor);
    nodes
        .into_iter()
        .next()
        .with_context(|| format!("no {prefix} node"))
}

fn matching_card_node(render: &DriNode) -> Result<DriNode> {
    let render_pci = sysctl_value(&format!("dev.drm.{}.PCI_ID", render.minor)).ok();
    let mut fallback = None;
    for entry in fs::read_dir("/dev/dri").context("read /dev/dri")? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(number) = name
            .strip_prefix("card")
            .and_then(|value| value.parse::<u32>().ok())
        else {
            continue;
        };
        let node = DriNode {
            name,
            index: number,
            minor: number,
        };
        if fallback.is_none() {
            fallback = Some(node.clone());
        }
        let card_pci = sysctl_value(&format!("dev.drm.{}.PCI_ID", node.minor)).ok();
        if render_pci.is_some() && card_pci == render_pci {
            return Ok(node);
        }
    }
    fallback.context("no card node")
}

fn dri_node_dev_t(node: &DriNode) -> Result<u64> {
    let path = Path::new("/dev/dri").join(&node.name);
    Ok(fs::metadata(&path)
        .with_context(|| format!("stat {}", path.display()))?
        .rdev())
}

fn write_linux_drm_sysfs(
    bus: &Path,
    dev_char: &Path,
    class_drm: &Path,
    device: &DrmDevice,
) -> Result<()> {
    let pci_device = bus.join("pci").join("devices").join(&device.pci_slot);
    let pci_driver = bus.join("pci").join("drivers").join(&device.driver);
    fs::create_dir_all(pci_device.join("drm").join(&device.card_name))?;
    fs::create_dir_all(pci_device.join("drm").join(&device.render_name))?;
    fs::create_dir_all(&pci_driver)?;
    write_file(pci_device.join("vendor"), &format!("{}\n", device.vendor))?;
    write_file(pci_device.join("device"), &format!("{}\n", device.device))?;
    write_file(
        pci_device.join("subsystem_vendor"),
        &format!("{}\n", device.subsystem_vendor),
    )?;
    write_file(
        pci_device.join("subsystem_device"),
        &format!("{}\n", device.subsystem_device),
    )?;
    write_file(
        pci_device.join("revision"),
        &format!("{}\n", device.revision),
    )?;
    write_file(pci_device.join("class"), &format!("{}\n", device.class))?;
    symlink_replace("/sys/bus/pci", &pci_device.join("subsystem"))?;
    symlink_replace(
        format!("/sys/bus/pci/drivers/{}", device.driver),
        &pci_device.join("driver"),
    )?;
    symlink_replace(
        format!("/sys/bus/pci/devices/{}", device.pci_slot),
        &pci_driver.join(&device.pci_slot),
    )?;
    write_file(
        pci_device.join("uevent"),
        &format!(
            "DRIVER={}\nPCI_CLASS={}\nPCI_ID={}:{}\nPCI_SUBSYS_ID={}:{}\nPCI_SLOT_NAME={}\nMODALIAS=pci:v0000{}d0000{}sv0000{}sd0000{}bc{}sc00i00\n",
            device.driver,
            trim_hex(&device.class),
            trim_hex(&device.vendor).to_ascii_uppercase(),
            trim_hex(&device.device).to_ascii_uppercase(),
            trim_hex(&device.subsystem_vendor).to_ascii_uppercase(),
            trim_hex(&device.subsystem_device).to_ascii_uppercase(),
            device.pci_slot,
            trim_hex(&device.vendor).to_ascii_uppercase(),
            trim_hex(&device.device).to_ascii_uppercase(),
            trim_hex(&device.subsystem_vendor).to_ascii_uppercase(),
            trim_hex(&device.subsystem_device).to_ascii_uppercase(),
            trim_hex(&device.class).get(0..2).unwrap_or("03"),
        ),
    )?;

    write_dev_char_node(
        dev_char,
        &device.card_name,
        device.card_minor,
        &device.pci_slot,
    )?;
    write_dev_char_node(
        dev_char,
        &device.render_name,
        device.render_minor,
        &device.pci_slot,
    )?;
    write_class_drm_node(
        class_drm,
        &device.card_name,
        device.card_minor,
        &device.pci_slot,
    )?;
    write_class_drm_node(
        class_drm,
        &device.render_name,
        device.render_minor,
        &device.pci_slot,
    )?;
    Ok(())
}

fn write_dev_char_node(dev_char: &Path, name: &str, minor: u32, pci_slot: &str) -> Result<()> {
    let dir = dev_char.join(format!("{DRM_MAJOR}:{minor}"));
    fs::create_dir_all(&dir)?;
    symlink_replace(
        format!("/sys/bus/pci/devices/{pci_slot}"),
        &dir.join("device"),
    )?;
    write_file(
        dir.join("uevent"),
        &format!("MAJOR={DRM_MAJOR}\nMINOR={minor}\nDEVNAME=dri/{name}\n"),
    )
}

fn ensure_wayland_drm_devt_shim(paths: &Installation) -> Result<PathBuf> {
    installed_helper(paths, "libwayland-drm-devt-shim.so")
}

fn ensure_drm_syncobj_errno_shim(paths: &Installation) -> Result<PathBuf> {
    installed_helper(paths, "libdrm-syncobj-errno-shim.so")
}

fn ensure_chromium_zygote_drm_preload(paths: &Installation) -> Result<PathBuf> {
    installed_helper(paths, "libchromium-zygote-drm-preload.so")
}

fn installed_helper(paths: &Installation, name: &str) -> Result<PathBuf> {
    let output = paths.libexec_root().join(name);
    if !output.is_file() {
        bail!("installed graphics helper is missing: {}", output.display());
    }
    Ok(output)
}

fn linux_drm_dev_t(minor: u32) -> u64 {
    linux_makedev(DRM_MAJOR as u64, minor as u64)
}

fn linux_makedev(major: u64, minor: u64) -> u64 {
    ((major & 0xfff) << 8) | (minor & 0xff) | ((minor & !0xff) << 12)
}

fn write_class_drm_node(class_drm: &Path, name: &str, minor: u32, pci_slot: &str) -> Result<()> {
    let dir = class_drm.join(name);
    fs::create_dir_all(&dir)?;
    symlink_replace(
        format!("/sys/bus/pci/devices/{pci_slot}"),
        &dir.join("device"),
    )?;
    write_file(dir.join("dev"), &format!("{DRM_MAJOR}:{minor}\n"))?;
    write_file(
        dir.join("uevent"),
        &format!("MAJOR={DRM_MAJOR}\nMINOR={minor}\nDEVNAME=dri/{name}\n"),
    )
}

fn pciconf_info(pci_slot: &str) -> Result<PciInfo> {
    let locator = pciconf_locator(pci_slot)?;
    let output = Command::new("pciconf")
        .arg("-lv")
        .arg(locator)
        .output()
        .context("run pciconf -lv")?;
    if !output.status.success() {
        bail!("pciconf -lv failed with status {}", output.status);
    }
    let text = String::from_utf8(output.stdout)?;
    let first = text.lines().next().context("empty pciconf output")?;
    let get = |key: &str| -> Result<String> {
        first
            .split_whitespace()
            .find_map(|part| part.strip_prefix(&format!("{key}=")))
            .map(ToOwned::to_owned)
            .with_context(|| format!("pciconf output missing {key}"))
    };
    Ok(PciInfo {
        class: hex_prefixed(&get("class")?, 6),
        revision: hex_prefixed(&get("rev")?, 2),
        vendor: hex_prefixed(&get("vendor")?, 4),
        device: hex_prefixed(&get("device")?, 4),
        subsystem_vendor: hex_prefixed(&get("subvendor")?, 4),
        subsystem_device: hex_prefixed(&get("subdevice")?, 4),
    })
}

fn pciconf_locator(pci_slot: &str) -> Result<String> {
    let mut slot_parts = pci_slot.split(':');
    let _domain = slot_parts.next().context("missing PCI domain")?;
    let bus = slot_parts.next().context("missing PCI bus")?;
    let device_function = slot_parts.next().context("missing PCI device/function")?;
    let (device, function) = device_function
        .split_once('.')
        .context("missing PCI function")?;
    Ok(format!(
        "pci0:{}:{}:{}",
        parse_hex_or_decimal(bus)?,
        parse_hex_or_decimal(device)?,
        parse_hex_or_decimal(function)?
    ))
}

fn sysctl_value(name: &str) -> Result<String> {
    let output = Command::new("sysctl")
        .arg("-n")
        .arg(name)
        .output()
        .with_context(|| format!("run sysctl -n {name}"))?;
    if !output.status.success() {
        bail!("sysctl -n {name} failed with status {}", output.status);
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn write_file(path: PathBuf, data: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, data).with_context(|| format!("write {}", path.display()))
}

fn symlink_replace(target: impl AsRef<Path>, link: &Path) -> Result<()> {
    if fs::symlink_metadata(link).is_ok() {
        fs::remove_file(link).with_context(|| format!("replace {}", link.display()))?;
    }
    unix_fs::symlink(target, link).with_context(|| format!("symlink {}", link.display()))
}

fn parse_hex_or_decimal(value: &str) -> Result<u32> {
    u32::from_str_radix(value, 16)
        .or_else(|_| value.parse::<u32>())
        .with_context(|| format!("parse PCI number {value}"))
}

fn hex4(value: &str) -> String {
    hex_prefixed(value, 4)
}

fn hex_prefixed(value: &str, width: usize) -> String {
    let value = value.trim().trim_start_matches("0x");
    format!("0x{:0>width$}", value.to_ascii_lowercase())
}

fn trim_hex(value: &str) -> &str {
    value.trim().trim_start_matches("0x")
}

fn safe_name(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

pub fn recover_stale_graphics_dirs(paths: &Installation) -> Result<()> {
    let root = paths.gpu();
    if !root.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(&root).with_context(|| format!("read {}", root.display()))? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            if entry
                .file_name()
                .to_string_lossy()
                .rsplit_once('-')
                .and_then(|(_, pid)| pid.parse::<i32>().ok())
                .is_some_and(process_alive)
            {
                continue;
            }
            let _ = fs::remove_dir_all(entry.path());
        }
    }
    Ok(())
}

fn process_alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

#[cfg(test)]
mod tests {
    use super::{
        hex_prefixed, linux_drm_dev_t, parse_hex_or_decimal, pciconf_locator,
        write_linux_drm_sysfs, DrmDevice, DrmSyncobjErrnoShim, HostGraphics, WaylandDrmDevtShim,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    #[test]
    fn converts_linux_pci_slot_to_freebsd_locator() {
        assert_eq!(pciconf_locator("0000:00:02.0").unwrap(), "pci0:0:2:0");
    }

    #[test]
    fn normalizes_hex_values() {
        assert_eq!(hex_prefixed("8086", 4), "0x8086");
        assert_eq!(hex_prefixed("0x2", 2), "0x02");
    }

    #[test]
    fn parses_pci_numbers_as_hex() {
        assert_eq!(parse_hex_or_decimal("1c").unwrap(), 28);
        assert_eq!(parse_hex_or_decimal("02").unwrap(), 2);
    }

    #[test]
    fn chooses_intel_vulkan_icd_from_pci_vendor() {
        let device = DrmDevice {
            card_name: "card0".to_string(),
            card_minor: 0,
            card_host_dev_t: 0x61,
            render_name: "renderD128".to_string(),
            render_minor: 128,
            render_host_dev_t: 0x100,
            pci_slot: "0000:00:02.0".to_string(),
            vendor: "0x8086".to_string(),
            device: "0x9b41".to_string(),
            class: "0x030000".to_string(),
            revision: "0x00".to_string(),
            subsystem_vendor: "0x0000".to_string(),
            subsystem_device: "0x0000".to_string(),
            driver: "i915".to_string(),
        };

        assert_eq!(device.vulkan_icd(), Some("intel_icd.x86_64.json"));
    }

    #[test]
    fn encodes_linux_drm_dev_t_values() {
        assert_eq!(linux_drm_dev_t(0), 0xe200);
        assert_eq!(linux_drm_dev_t(128), 0xe280);
    }

    #[test]
    fn maps_host_drm_dev_t_values_to_linux_drm_dev_t_values() {
        let device = DrmDevice {
            card_name: "card0".to_string(),
            card_minor: 0,
            card_host_dev_t: 0x61,
            render_name: "renderD128".to_string(),
            render_minor: 128,
            render_host_dev_t: 0x100,
            pci_slot: "0000:00:02.0".to_string(),
            vendor: "0x8086".to_string(),
            device: "0x9b41".to_string(),
            class: "0x030000".to_string(),
            revision: "0x00".to_string(),
            subsystem_vendor: "0x0000".to_string(),
            subsystem_device: "0x0000".to_string(),
            driver: "i915".to_string(),
        };

        assert_eq!(device.dev_t_map(), "0x61=0xe200,0x100=0xe280");
    }

    #[test]
    fn keeps_drm_and_wayland_preloads_separate_on_one_mount() {
        let host_dir = PathBuf::from("/tmp/freebsd-flatpak-poc-graphics-shims");
        let graphics = HostGraphics {
            gl: None,
            drm: None,
            drm_syncobj_errno_shim: Some(DrmSyncobjErrnoShim {
                host_dir: host_dir.clone(),
            }),
            chromium_zygote_drm_preload: None,
            wayland_drm_devt_shim: Some(WaylandDrmDevtShim {
                host_dir: host_dir.clone(),
                dev_t_map: "0x61=0xe200".to_string(),
            }),
            warnings: Vec::new(),
        };

        assert_eq!(
            graphics.ld_preload_paths(),
            vec![
                "/run/host/freebsd-flatpak-poc/libdrm-syncobj-errno-shim.so",
                "/run/host/freebsd-flatpak-poc/libwayland-drm-devt-shim.so",
            ]
        );
        assert_eq!(
            graphics.zypak_ld_preload_paths(),
            vec![
                "/run/host/freebsd-flatpak-poc/libwayland-drm-devt-shim.so",
                "/run/host/freebsd-flatpak-poc/libdrm-syncobj-errno-shim.so",
            ]
        );
        let mounts = graphics.runtime_mounts();
        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].host_path(), host_dir);
        assert_eq!(
            mounts[0].sandbox_target_relative().unwrap(),
            PathBuf::from("run/host/freebsd-flatpak-poc")
        );
    }

    #[test]
    fn writes_pci_driver_links_for_drm_sysfs() {
        let root = std::env::temp_dir().join(format!(
            "freebsd-flatpak-poc-graphics-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let bus = root.join("sys-bus");
        let dev_char = root.join("sys-dev-char");
        let class_drm = root.join("sys-class-drm");
        let device = DrmDevice {
            card_name: "card0".to_string(),
            card_minor: 0,
            card_host_dev_t: 0x61,
            render_name: "renderD128".to_string(),
            render_minor: 128,
            render_host_dev_t: 0x100,
            pci_slot: "0000:00:02.0".to_string(),
            vendor: "0x8086".to_string(),
            device: "0x9b41".to_string(),
            class: "0x030000".to_string(),
            revision: "0x02".to_string(),
            subsystem_vendor: "0x1028".to_string(),
            subsystem_device: "0x096e".to_string(),
            driver: "i915".to_string(),
        };

        write_linux_drm_sysfs(&bus, &dev_char, &class_drm, &device).unwrap();

        let pci_device = bus.join("pci/devices/0000:00:02.0");
        assert_eq!(
            fs::read_link(pci_device.join("driver")).unwrap(),
            PathBuf::from("/sys/bus/pci/drivers/i915")
        );
        assert_eq!(
            fs::read_link(bus.join("pci/drivers/i915/0000:00:02.0")).unwrap(),
            PathBuf::from("/sys/bus/pci/devices/0000:00:02.0")
        );
        assert!(fs::read_to_string(pci_device.join("uevent"))
            .unwrap()
            .contains("DRIVER=i915"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn drm_syncobj_errno_shim_has_narrow_ioctl_behavior() {
        let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let output_dir = project_root.join("target/graphics-tests");
        fs::create_dir_all(&output_dir).unwrap();
        let output = output_dir.join("drm-syncobj-errno-shim-test");
        let compile = Command::new("/compat/linux/usr/bin/gcc")
            .args(["-O2", "-Wall", "-Wextra", "-Werror"])
            .arg("-DDRM_SYNCOBJ_ERRNO_SHIM_TEST")
            .arg(project_root.join("scripts/drm-syncobj-errno-shim.c"))
            .arg(project_root.join("tests/drm-syncobj-errno-shim-test.c"))
            .arg("-o")
            .arg(&output)
            .env(
                "PATH",
                "/compat/linux/usr/bin:/compat/linux/bin:/usr/bin:/bin",
            )
            .status()
            .expect("compile DRM syncobj errno shim test");
        assert!(compile.success());

        let run = Command::new(&output)
            .status()
            .expect("run DRM syncobj errno shim test");
        assert!(run.success());
    }

    #[test]
    fn chromium_zygote_preload_matches_only_zygote_exec() {
        let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let output_dir = project_root.join("target/graphics-tests");
        fs::create_dir_all(&output_dir).unwrap();
        let output = output_dir.join("chromium-zygote-drm-preload-test");
        let compile = Command::new("/compat/linux/usr/bin/gcc")
            .args(["-O2", "-Wall", "-Wextra", "-Werror"])
            .arg("-DCHROMIUM_ZYGOTE_DRM_PRELOAD_TEST")
            .arg(project_root.join("scripts/chromium-zygote-drm-preload.c"))
            .arg(project_root.join("tests/chromium-zygote-drm-preload-test.c"))
            .arg("-o")
            .arg(&output)
            .env(
                "PATH",
                "/compat/linux/usr/bin:/compat/linux/bin:/usr/bin:/bin",
            )
            .status()
            .expect("compile Chromium zygote preload test");
        assert!(compile.success());

        let run = Command::new(&output)
            .status()
            .expect("run Zypak child spawn preload test");
        assert!(run.success());
    }
}
