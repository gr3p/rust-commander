use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const MOUNT_TYPES: &[&str] = &[
    "ext2", "ext3", "ext4", "btrfs", "xfs", "vfat", "exfat", "ntfs", "ntfs3", "iso9660", "udf",
    "cifs", "nfs", "nfs4",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub device: String,
    pub size: u64,
    pub kind: String,
    pub mount_as: String,
    pub label: Option<String>,
    pub locked: bool,
    pub point: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Held {
    pub point: String,
    pub source: String,
    pub kind: String,
}

pub enum Job {
    Block {
        device: String,
        fstype: String,
        point: String,
        uid: u32,
        gid: u32,
    },
    Luks {
        device: String,
        point: String,
        uid: u32,
        gid: u32,
        passphrase_file: PathBuf,
    },
    Cifs {
        source: String,
        point: String,
        uid: u32,
        gid: u32,
        credentials: PathBuf,
    },
    Nfs {
        source: String,
        point: String,
    },
    Umount {
        point: String,
    },
}

impl Job {
    fn args(&self) -> Vec<String> {
        match self {
            Job::Block {
                device,
                fstype,
                point,
                uid,
                gid,
            } => {
                vec![
                    "block".into(),
                    device.clone(),
                    fstype.clone(),
                    point.clone(),
                    uid.to_string(),
                    gid.to_string(),
                ]
            }
            Job::Luks {
                device,
                point,
                uid,
                gid,
                passphrase_file,
            } => vec![
                "luks".into(),
                device.clone(),
                point.clone(),
                uid.to_string(),
                gid.to_string(),
                passphrase_file.display().to_string(),
            ],
            Job::Cifs {
                source,
                point,
                uid,
                gid,
                credentials,
            } => vec![
                "cifs".into(),
                source.clone(),
                point.clone(),
                uid.to_string(),
                gid.to_string(),
                credentials.display().to_string(),
            ],
            Job::Nfs { source, point } => vec!["nfs".into(), source.clone(), point.clone()],
            Job::Umount { point } => vec!["umount".into(), point.clone()],
        }
    }

    pub fn wipe(&self) {
        match self {
            Job::Luks {
                passphrase_file, ..
            } => wipe_secret(passphrase_file),
            Job::Cifs { credentials, .. } => wipe_secret(credentials),
            _ => {}
        }
    }
}

pub fn ids() -> (u32, u32) {
    unsafe { (libc::getuid(), libc::getgid()) }
}

pub fn candidates() -> Result<Vec<Candidate>, String> {
    let out = Command::new("lsblk")
        .args([
            "-J",
            "-b",
            "-o",
            "PATH,SIZE,TYPE,FSTYPE,LABEL,MOUNTPOINT",
        ])
        .output()
        .map_err(|err| format!("lsblk: {err}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim();
        return Err(if err.is_empty() {
            "lsblk failed".into()
        } else {
            err.to_string()
        });
    }
    parse_lsblk(&String::from_utf8_lossy(&out.stdout))
}

pub fn parse_lsblk(text: &str) -> Result<Vec<Candidate>, String> {
    let root: Value =
        serde_json::from_str(text).map_err(|_| "Could not read the disk list".to_string())?;
    let mut found = Vec::new();
    if let Some(list) = root.get("blockdevices").and_then(Value::as_array) {
        walk(list, &mut found);
    }
    assign_points(&mut found);
    Ok(found)
}

pub fn held() -> Vec<Held> {
    let Ok(text) = fs::read_to_string("/proc/mounts") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let Some(source) = parts.next() else { continue };
        let Some(point) = parts.next() else { continue };
        let Some(kind) = parts.next() else { continue };
        if check_point(point).is_err() || !(MOUNT_TYPES.contains(&kind) || kind == "fuseblk") {
            continue;
        }
        out.push(Held {
            point: point.to_string(),
            source: source.to_string(),
            kind: kind.to_string(),
        });
    }
    out
}

pub fn point_name(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            out.push(c);
        } else if c == '.' {
            if !out.is_empty() && !out.ends_with('.') {
                out.push('.');
            }
        } else if c.is_whitespace() && !out.is_empty() && !out.ends_with('_') {
            out.push('_');
        }
    }
    let out = out.trim_matches(|c| c == '.' || c == '_').to_string();
    if out.is_empty() {
        "disk".into()
    } else {
        trim_name(&out)
    }
}

pub fn check_point(raw: &str) -> Result<PathBuf, String> {
    let path = Path::new(raw.trim());
    let mut parts = path.components();
    let name = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (
            Some(Component::RootDir),
            Some(Component::Normal(dir)),
            Some(Component::Normal(name)),
            None,
        ) if dir == "mnt" => name,
        _ => return Err("Mount point must be /mnt/<name>".into()),
    };
    if !valid_name(name) {
        return Err("Mount point must be /mnt/<name>".into());
    }
    let name = name.to_str().unwrap();
    Ok(PathBuf::from(format!("/mnt/{name}")))
}

pub fn point_ready(raw: &str) -> Result<PathBuf, String> {
    let point = check_point(raw)?;
    if !point.exists() {
        return Ok(point);
    }
    if !point.is_dir() {
        return Err(format!("{} is not a directory", point.display()));
    }
    if fs::read_dir(&point)
        .map_err(|err| err.to_string())?
        .next()
        .is_some()
    {
        return Err(format!("{} is not empty", point.display()));
    }
    Ok(point)
}

pub fn check_unc(server: &str, share: &str) -> Result<String, String> {
    let server = server.trim();
    let share = share.trim();
    if server.is_empty() {
        return Err("Enter the server".into());
    }
    if share.is_empty() {
        return Err("Enter the share".into());
    }
    if !is_host(server) {
        return Err("The server name is not usable".into());
    }
    if !is_share(share) {
        return Err("The share name is not usable".into());
    }
    Ok(format!("//{server}/{share}"))
}

pub fn check_nfs(server: &str, export: &str) -> Result<String, String> {
    let server = server.trim();
    let export = export.trim();
    if server.is_empty() {
        return Err("Enter the server".into());
    }
    if export.is_empty() {
        return Err("Enter the export".into());
    }
    if !is_host(server) || !is_export(export) {
        return Err("The export must be a path like /data".into());
    }
    Ok(format!("{server}:{export}"))
}

pub fn block_job(
    device: &str,
    fstype: &str,
    point: &str,
    uid: u32,
    gid: u32,
) -> Result<Job, String> {
    let device = check_block_device(device)?;
    let fstype = known_type(fstype)?;
    let point = point_ready(point)?;
    Ok(Job::Block {
        device: path_str(&device)?,
        fstype,
        point: point.display().to_string(),
        uid,
        gid,
    })
}

pub fn luks_job(
    device: &str,
    point: &str,
    uid: u32,
    gid: u32,
    passphrase: &str,
) -> Result<Job, String> {
    if passphrase.is_empty() || !one_line(passphrase) {
        return Err("Enter the volume password".into());
    }
    let device = check_block_device(device)?;
    let point = point_ready(point)?;
    let passphrase_file = write_secret(passphrase)?;
    Ok(Job::Luks {
        device: path_str(&device)?,
        point: point.display().to_string(),
        uid,
        gid,
        passphrase_file,
    })
}

pub fn cifs_job(
    server: &str,
    share: &str,
    user: &str,
    password: &str,
    point: &str,
    uid: u32,
    gid: u32,
) -> Result<Job, String> {
    let source = check_unc(server, share)?;
    let user = user.trim();
    if user.is_empty() || !one_line(user) || user.contains('=') {
        return Err("Enter the user".into());
    }
    if password.is_empty() || !one_line(password) {
        return Err("Enter the password".into());
    }
    let point = point_ready(point)?;
    let credentials = write_secret(&format!("username={user}\npassword={password}\n"))?;
    Ok(Job::Cifs {
        source,
        point: point.display().to_string(),
        uid,
        gid,
        credentials,
    })
}

pub fn nfs_job(server: &str, export: &str, point: &str) -> Result<Job, String> {
    let source = check_nfs(server, export)?;
    let point = point_ready(point)?;
    Ok(Job::Nfs {
        source,
        point: point.display().to_string(),
    })
}

pub fn umount_job(point: &str) -> Result<Job, String> {
    let point = check_point(point)?;
    Ok(Job::Umount {
        point: point.display().to_string(),
    })
}

pub fn pkexec(job: &Job) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    // After a package upgrade the running binary reads as "<path> (deleted)"; the new one sits at <path>.
    let exe = exe.to_str().and_then(|text| text.strip_suffix(" (deleted)")).map_or(exe.clone(), PathBuf::from);
    let mut cmd = Command::new("pkexec");
    cmd.arg(exe);
    cmd.arg("--mount-helper");
    cmd.args(job.args());
    let out = cmd.output().map_err(|err| format!("pkexec: {err}"))?;
    if out.status.success() {
        return Ok(());
    }
    let detail = {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if err.is_empty() {
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        } else {
            err
        }
    };
    match out.status.code() {
        Some(126) => Err("Mount cancelled".into()),
        Some(127) if detail.is_empty() => Err("Not allowed to mount".into()),
        _ if !detail.is_empty() => Err(detail),
        Some(code) => Err(format!("Mount failed ({code})")),
        None => Err("Mount failed".into()),
    }
}

// pkexec runs this before GTK. The argument list is fixed, never a shell.
pub fn helper() -> i32 {
    match helper_args(std::env::args().skip(2)) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

fn helper_args(args: impl IntoIterator<Item = String>) -> Result<(), String> {
    let mut args = args.into_iter();
    let cmd = args.next().unwrap_or_default();
    match cmd.as_str() {
        "block" => {
            let device = need(&mut args, "device")?;
            let fstype = need(&mut args, "type")?;
            let point = need(&mut args, "mount point")?;
            let (uid, gid) = owner(&mut args)?;
            finish(&mut args)?;
            with_point(&point, || mount_block(&device, &fstype, &point, uid, gid))
        }
        "luks" => {
            let device = need(&mut args, "device")?;
            let point = need(&mut args, "mount point")?;
            let (uid, gid) = owner(&mut args)?;
            let pass = need(&mut args, "passphrase")?;
            finish(&mut args)?;
            with_point(&point, || mount_luks(&device, &point, uid, gid, &pass))
        }
        "cifs" => {
            let source = need(&mut args, "share")?;
            let point = need(&mut args, "mount point")?;
            let (uid, gid) = owner(&mut args)?;
            let cred = need(&mut args, "credentials")?;
            finish(&mut args)?;
            with_point(&point, || mount_cifs(&source, &point, uid, gid, &cred))
        }
        "nfs" => {
            let source = need(&mut args, "export")?;
            let point = need(&mut args, "mount point")?;
            finish(&mut args)?;
            with_point(&point, || mount_nfs(&source, &point))
        }
        "umount" => {
            let point = need(&mut args, "mount point")?;
            finish(&mut args)?;
            do_umount(&point)
        }
        _ => Err("Unknown mount request".into()),
    }
}

fn owner(args: &mut impl Iterator<Item = String>) -> Result<(u32, u32), String> {
    let uid = need(args, "uid")?.parse().map_err(|_| "Bad uid".to_string())?;
    let gid = need(args, "gid")?.parse().map_err(|_| "Bad gid".to_string())?;
    Ok((uid, gid))
}

// A mount point made for a mount that failed is removed again.
fn with_point(point: &str, run: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    let path = check_point(point)?;
    let existed = path.exists();
    let result = run();
    if result.is_err() && !existed {
        let _ = fs::remove_dir(&path);
    }
    result
}

fn mount_block(
    device: &str,
    fstype: &str,
    point: &str,
    uid: u32,
    gid: u32,
) -> Result<(), String> {
    let device = path_str(&check_block_device(device)?)?;
    let fstype = known_type(fstype)?;
    let point = prepare_point(point)?;
    ensure_free(&device, &point)?;
    mount_at(&device, &fstype, &point, owner_opts(&fstype, uid, gid))
}

fn mount_luks(device: &str, point: &str, uid: u32, gid: u32, pass: &str) -> Result<(), String> {
    let pass = secret_path(Path::new(pass), uid)?;
    let result = mount_luks_at(device, point, uid, gid, &pass);
    wipe_secret(&pass);
    result
}

fn mount_luks_at(device: &str, point: &str, uid: u32, gid: u32, pass: &Path) -> Result<(), String> {
    let device = check_block_device(device)?;
    let point = prepare_point(point)?;
    let name = format!(
        "rc-{}",
        point.file_name().and_then(|n| n.to_str()).unwrap_or("disk")
    );
    let mapper = PathBuf::from(format!("/dev/mapper/{name}"));
    let opened = if mapper.exists() {
        if !mapped_from(&mapper, &device) {
            return Err(format!("{name} is already open for another volume"));
        }
        false
    } else {
        run_tool(
            "cryptsetup",
            &[
                "luksOpen".into(),
                "--key-file".into(),
                path_str(pass)?,
                "--batch-mode".into(),
                path_str(&device)?,
                name.clone(),
            ],
        )?;
        true
    };
    let mounted = (|| {
        let fstype = detect_type(&mapper)?;
        let mapper_path = path_str(&mapper)?;
        ensure_free(&mapper_path, &point)?;
        mount_at(
            &mapper_path,
            &fstype,
            &point,
            owner_opts(&fstype, uid, gid),
        )
    })();
    if mounted.is_err() && opened {
        let _ = run_tool("cryptsetup", &["close".into(), "--batch-mode".into(), name]);
    }
    mounted
}

fn mount_cifs(source: &str, point: &str, uid: u32, gid: u32, cred: &str) -> Result<(), String> {
    let cred = secret_path(Path::new(cred), uid)?;
    let result = mount_cifs_at(source, point, uid, gid, &cred);
    wipe_secret(&cred);
    result
}

fn mount_cifs_at(source: &str, point: &str, uid: u32, gid: u32, cred: &Path) -> Result<(), String> {
    let source = check_unc_source(source)?;
    let point = prepare_point(point)?;
    ensure_free(&source, &point)?;
    let cred = path_str(cred)?;
    if cred.contains(',') {
        return Err("Bad credentials path".into());
    }
    let opts = format!("credentials={cred},uid={uid},gid={gid}");
    mount_at(&source, "cifs", &point, Some(opts))
}

fn mount_nfs(source: &str, point: &str) -> Result<(), String> {
    let source = check_nfs_source(source)?;
    let point = prepare_point(point)?;
    ensure_free(&source, &point)?;
    // retry=0 fails a missing server instead of waiting.
    mount_at(&source, "nfs", &point, Some("retry=0".into()))
}

fn do_umount(point: &str) -> Result<(), String> {
    let point = check_point(point)?;
    let source =
        mounted_source(&point).ok_or_else(|| format!("{} is not mounted", point.display()))?;
    run_tool("umount", &[point.display().to_string()])?;
    close_ours(&source);
    Ok(())
}

fn mount_at(source: &str, fstype: &str, point: &Path, opts: Option<String>) -> Result<(), String> {
    // A disk or share from elsewhere must not bring setuid files or device nodes.
    let opts = opts.map_or("nosuid,nodev".to_string(), |opts| format!("{opts},nosuid,nodev"));
    let args = [
        "-t".into(),
        fstype.into(),
        source.into(),
        point.display().to_string(),
        "-o".into(),
        opts,
    ];
    run_tool("mount", &args)
}

fn owner_opts(fstype: &str, uid: u32, gid: u32) -> Option<String> {
    matches!(fstype, "vfat" | "exfat" | "ntfs" | "ntfs3").then(|| format!("uid={uid},gid={gid}"))
}

fn prepare_point(raw: &str) -> Result<PathBuf, String> {
    let point = check_point(raw)?;
    if point.exists() {
        if !point.is_dir() {
            return Err(format!("{} is not a directory", point.display()));
        }
        if fs::read_dir(&point)
            .map_err(|err| err.to_string())?
            .next()
            .is_some()
        {
            return Err(format!("{} is not empty", point.display()));
        }
    } else {
        if !Path::new("/mnt").exists() {
            fs::create_dir("/mnt").map_err(|err| err.to_string())?;
        }
        fs::create_dir(&point)
            .map_err(|err| format!("Could not create {}: {err}", point.display()))?;
    }
    Ok(point)
}

fn ensure_free(device: &str, point: &Path) -> Result<(), String> {
    let text = fs::read_to_string("/proc/mounts").unwrap_or_default();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let Some(dev) = parts.next() else { continue };
        let Some(mnt) = parts.next() else { continue };
        if Path::new(mnt) == point {
            return Err(format!("{} is already mounted", point.display()));
        }
        if same_device(dev, device) {
            return Err(format!("{device} is already mounted"));
        }
    }
    Ok(())
}

fn mounted_source(point: &Path) -> Option<String> {
    let text = fs::read_to_string("/proc/mounts").ok()?;
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let dev = parts.next()?;
        let mnt = parts.next()?;
        if Path::new(mnt) == point {
            return Some(dev.to_string());
        }
    }
    None
}

fn close_ours(source: &str) {
    let Some(name) = source.strip_prefix("/dev/mapper/") else {
        return;
    };
    let Some(rest) = name.strip_prefix("rc-") else {
        return;
    };
    if check_point(&format!("/mnt/{rest}")).is_err() || mounted_source_is(source) {
        return;
    }
    let _ = run_tool(
        "cryptsetup",
        &["close".into(), "--batch-mode".into(), name.into()],
    );
}

fn mounted_source_is(source: &str) -> bool {
    fs::read_to_string("/proc/mounts")
        .unwrap_or_default()
        .lines()
        .any(|line| line.split_whitespace().next() == Some(source))
}

fn same_device(listed: &str, device: &str) -> bool {
    if listed == device {
        return true;
    }
    let listed = fs::canonicalize(listed).ok();
    let device = fs::canonicalize(device).ok();
    listed.is_some() && listed == device
}

// A leftover mapping is reused only when it was opened from this very device.
fn mapped_from(mapper: &Path, device: &Path) -> bool {
    let (Ok(dm), Some(base)) = (fs::canonicalize(mapper), device.file_name()) else {
        return false;
    };
    let Some(dm) = dm.file_name() else { return false };
    Path::new("/sys/block").join(dm).join("slaves").join(base).exists()
}

fn detect_type(dev: &Path) -> Result<String, String> {
    let dev_path = path_str(dev)?;
    let out = Command::new(tool("lsblk")?)
        .args(["-n", "-o", "FSTYPE", &dev_path])
        .output()
        .map_err(|err| err.to_string())?;
    if !out.status.success() {
        return Err("Could not read the unlocked volume".into());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let kind = text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("");
    known_type(&driver(kind))
}

// lsblk says ntfs, or fuseblk for NTFS-3g. The in-kernel ntfs3 is used when the kernel has it.
fn driver(fstype: &str) -> String {
    let ntfs3 = fs::read_to_string("/proc/filesystems").is_ok_and(|text| text.lines().any(|line| line.split_whitespace().last() == Some("ntfs3")));
    match fstype {
        "ntfs" | "fuseblk" if ntfs3 => "ntfs3".into(),
        "fuseblk" => "ntfs".into(),
        other => other.into(),
    }
}

fn known_type(fstype: &str) -> Result<String, String> {
    if MOUNT_TYPES.contains(&fstype) {
        Ok(fstype.to_string())
    } else {
        Err(format!("Cannot mount {fstype}"))
    }
}

fn check_block_device(raw: &str) -> Result<PathBuf, String> {
    let path = Path::new(raw.trim());
    if !path.is_absolute()
        || !path.starts_with("/dev")
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err("The device is not a disk".into());
    }
    let canon = fs::canonicalize(path).map_err(|_| "The device is gone".to_string())?;
    if !canon.starts_with("/dev") {
        return Err("The device is not a disk".into());
    }
    let meta = fs::metadata(&canon).map_err(|err| err.to_string())?;
    if !meta.file_type().is_block_device() {
        return Err("The device is not a disk".into());
    }
    Ok(canon)
}

fn check_unc_source(raw: &str) -> Result<String, String> {
    let Some(rest) = raw.strip_prefix("//") else {
        return Err("The share address is not usable".into());
    };
    let Some((server, share)) = rest.split_once('/') else {
        return Err("The share address is not usable".into());
    };
    if share.contains('/') {
        return Err("The share address is not usable".into());
    }
    check_unc(server, share)
}

fn check_nfs_source(raw: &str) -> Result<String, String> {
    let Some((server, export)) = raw.split_once(':') else {
        return Err("The export is not usable".into());
    };
    if server.contains(':') {
        return Err("The export is not usable".into());
    }
    check_nfs(server, export)
}

fn walk(list: &[Value], out: &mut Vec<Candidate>) {
    for dev in list {
        let children = dev.get("children").and_then(Value::as_array);
        // The filesystem is the child: a partition, or the mapper of an open LUKS volume.
        if children.is_none_or(|kids| kids.is_empty())
            && let Some(cand) = consider(dev)
        {
            out.push(cand);
        }
        if let Some(children) = children {
            walk(children, out);
        }
    }
}

fn consider(dev: &Value) -> Option<Candidate> {
    let path = text(dev, "path")?;
    if !path.starts_with("/dev/") || text(dev, "type").as_deref() == Some("loop") {
        return None;
    }
    let size = number(dev, "size");
    if size == 0 || text(dev, "mountpoint").is_some() {
        return None;
    }
    let fstype = text(dev, "fstype")?;
    if fstype == "swap" || fstype == "squashfs" {
        return None;
    }
    let label = text(dev, "label");
    if fstype == "crypto_LUKS" {
        return Some(Candidate {
            device: path,
            size,
            kind: "LUKS".into(),
            mount_as: String::new(),
            label,
            locked: true,
            point: String::new(),
        });
    }
    let mount_as = driver(&fstype);
    if !MOUNT_TYPES.contains(&mount_as.as_str()) {
        return None;
    }
    let kind = if fstype == "fuseblk" { "ntfs".into() } else { fstype };
    Some(Candidate {
        device: path,
        size,
        kind,
        mount_as,
        label,
        locked: false,
        point: String::new(),
    })
}

fn assign_points(items: &mut [Candidate]) {
    let mut used = HashSet::new();
    for item in items {
        let short = short_dev(&item.device).to_string();
        let base = point_name(item.label.as_deref().unwrap_or(&short));
        let name = unique_name(&base, &short, &mut used);
        item.point = format!("/mnt/{name}");
    }
}

fn unique_name(base: &str, extra: &str, used: &mut HashSet<String>) -> String {
    let base = trim_name(base);
    if used.insert(base.clone()) {
        return base;
    }
    let alt = trim_name(&format!("{base}-{extra}"));
    if used.insert(alt.clone()) {
        return alt;
    }
    let mut n = 2u32;
    loop {
        let next = trim_name(&format!("{extra}-{n}"));
        if used.insert(next.clone()) {
            return next;
        }
        n += 1;
    }
}

fn short_dev(path: &str) -> &str {
    path.rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("disk")
}

fn trim_name(name: &str) -> String {
    let name = if name.chars().count() > 64 {
        name.chars().take(64).collect()
    } else {
        name.to_string()
    };
    if name.is_empty() { "disk".into() } else { name }
}

fn text(dev: &Value, key: &str) -> Option<String> {
    dev.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn number(dev: &Value, key: &str) -> u64 {
    dev.get(key)
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
        .unwrap_or(0)
}

fn valid_name(name: &std::ffi::OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    (1..=64).contains(&name.len())
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

fn is_host(raw: &str) -> bool {
    !raw.is_empty()
        && raw.len() <= 253
        && !raw.starts_with('-')
        && !raw.ends_with('-')
        && !raw.contains("..")
        && raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
}

fn is_share(raw: &str) -> bool {
    !raw.is_empty()
        && raw.chars().count() <= 80
        && raw != "."
        && raw != ".."
        && !raw.contains("..")
        && raw
            .chars()
            .all(|c| !c.is_control() && c != '/' && c != '\\' && c != ':')
}

fn is_export(raw: &str) -> bool {
    let path = Path::new(raw);
    path.is_absolute()
        && path.components().all(|part| match part {
            Component::RootDir => true,
            Component::Normal(name) => is_share(name.to_str().unwrap_or("")),
            _ => false,
        })
}

fn one_line(text: &str) -> bool {
    !text.chars().any(|c| c.is_control()) && text.len() <= 1024
}

// The runtime dir is tmpfs and private, so a password never reaches the disk.
fn secret_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_dir())
        .unwrap_or_else(std::env::temp_dir)
}

fn write_secret(text: &str) -> Result<PathBuf, String> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let path = secret_dir().join(format!("rc-secret-{}-{nanos}", std::process::id()));
    let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path).map_err(|err| err.to_string())?;
    file.write_all(text.as_bytes()).map_err(|err| err.to_string())?;
    Ok(path)
}

// Root reads only a plain file the caller made: named rc-secret-*, owned by the caller, closed to others.
fn secret_path(raw: &Path, uid: u32) -> Result<PathBuf, String> {
    let named = raw.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("rc-secret-"));
    let meta = fs::symlink_metadata(raw).map_err(|_| "The password file is gone".to_string())?;
    let plain = meta.file_type().is_file() && meta.uid() == uid && meta.mode() & 0o077 == 0 && meta.len() <= 4096;
    if !raw.is_absolute() || !named || !plain {
        return Err("Bad secret path".into());
    }
    Ok(raw.to_path_buf())
}

fn wipe_secret(path: &Path) {
    let _ = fs::remove_file(path);
}

fn need(args: &mut impl Iterator<Item = String>, what: &str) -> Result<String, String> {
    args.next()
        .filter(|arg| !arg.is_empty())
        .ok_or_else(|| format!("Missing {what}"))
}

fn finish(args: &mut impl Iterator<Item = String>) -> Result<(), String> {
    if args.next().is_some() {
        Err("Too many arguments".into())
    } else {
        Ok(())
    }
}

fn path_str(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| "Bad path".to_string())
}

fn tool(name: &str) -> Result<PathBuf, String> {
    for dir in ["/usr/sbin", "/usr/bin", "/sbin", "/bin"] {
        let path = Path::new(dir).join(name);
        if path.is_file() {
            return Ok(path);
        }
    }
    Err(format!("{name} is not installed"))
}

fn run_tool(name: &str, args: &[String]) -> Result<(), String> {
    let path = tool(name)?;
    let out = Command::new(path)
        .args(args)
        .output()
        .map_err(|err| format!("{name}: {err}"))?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if err.is_empty() {
        Err(format!("{name} failed"))
    } else {
        Err(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "blockdevices": [
            {"path":"/dev/sda","size":100,"type":"disk","fstype":null,"label":null,"mountpoint":null,"children":[
                {"path":"/dev/sda1","size":50,"type":"part","fstype":"ext4","label":"root","mountpoint":"/"},
                {"path":"/dev/sda2","size":10,"type":"part","fstype":"swap","label":null,"mountpoint":"[SWAP]"},
                {"path":"/dev/sda3","size":40,"type":"part","fstype":"crypto_LUKS","label":"secret","mountpoint":null}
            ]},
            {"path":"/dev/sdb1","size":20,"type":"part","fstype":"ntfs","label":"Seagate","mountpoint":null},
            {"path":"/dev/sdc1","size":20,"type":"part","fstype":"ntfs","label":"Seagate","mountpoint":null},
            {"path":"/dev/sdd1","size":8,"type":"part","fstype":"fuseblk","label":"Backup","mountpoint":null},
            {"path":"/dev/loop0","size":5,"type":"loop","fstype":"squashfs","label":null,"mountpoint":"/snap/x"},
            {"path":"/dev/sr0","size":0,"type":"rom","fstype":"iso9660","label":"Disc","mountpoint":null},
            {"path":"/dev/sde1","size":4,"type":"part","fstype":"crypto_LUKS","label":"open","mountpoint":null,"children":[
                {"path":"/dev/mapper/open","size":4,"type":"crypt","fstype":"ext4","label":"open","mountpoint":null}
            ]}
        ]
    }"#;

    #[test]
    fn disks_skip_mounted_swap_loop_and_empty() {
        let found = parse_lsblk(SAMPLE).unwrap();
        let devices: Vec<_> = found.iter().map(|c| c.device.as_str()).collect();
        assert_eq!(
            devices,
            vec![
                "/dev/sda3",
                "/dev/sdb1",
                "/dev/sdc1",
                "/dev/sdd1",
                "/dev/mapper/open"
            ]
        );
        assert!(
            found
                .iter()
                .any(|c| c.device == "/dev/sda3" && c.locked && c.kind == "LUKS")
        );
        assert!(
            found
                .iter()
                .any(|c| c.device == "/dev/sdd1" && c.kind == "ntfs" && c.mount_as == driver("ntfs"))
        );
        assert!(
            found
                .iter()
                .any(|c| c.device == "/dev/mapper/open" && !c.locked && c.mount_as == "ext4")
        );
        assert_eq!(
            found
                .iter()
                .find(|c| c.device == "/dev/sdb1")
                .unwrap()
                .point,
            "/mnt/Seagate"
        );
        assert_eq!(
            found
                .iter()
                .find(|c| c.device == "/dev/sdc1")
                .unwrap()
                .point,
            "/mnt/Seagate-sdc1"
        );
    }

    #[test]
    fn mount_points_stay_under_mnt() {
        assert_eq!(
            check_point("/mnt/Seagate").unwrap(),
            PathBuf::from("/mnt/Seagate")
        );
        assert_eq!(
            check_point("  /mnt/disk-1  ").unwrap(),
            PathBuf::from("/mnt/disk-1")
        );
        for bad in [
            "/etc/passwd",
            "/mnt/../etc",
            "/mnt/foo/bar",
            "mnt/disk",
            "/mnt/foo bar",
            "/mnt/..",
            "/mnt/.hidden",
        ] {
            assert!(check_point(bad).is_err(), "{bad}");
        }
        assert_eq!(point_name("My Disk"), "My_Disk");
        assert_eq!(point_name("../etc"), "etc");
    }

    #[test]
    fn network_addresses_are_checked() {
        assert_eq!(check_unc("nas", "media").unwrap(), "//nas/media");
        assert_eq!(check_unc("nas", "Hidden$").unwrap(), "//nas/Hidden$");
        assert_eq!(check_unc("nas", "My Share").unwrap(), "//nas/My Share");
        assert!(check_unc("nas/evil", "media").is_err());
        assert!(check_unc("", "media").is_err());
        assert!(check_unc("nas", "").is_err());
        assert_eq!(
            check_nfs("nas", "/export/data").unwrap(),
            "nas:/export/data"
        );
        assert!(check_nfs("nas", "export").is_err());
        assert!(check_nfs("nas", "/../etc").is_err());
        assert!(check_nfs("na s", "/data").is_err());
        assert!(
            helper_args(["umount".into(), "/etc".into()])
                .unwrap_err()
                .contains("/mnt")
        );
        assert!(helper_args(["nope".into()]).is_err());
    }

    #[test]
    fn ntfs_uses_one_driver_and_other_types_are_left_alone() {
        assert_eq!(driver("fuseblk"), driver("ntfs"));
        assert!(matches!(driver("ntfs").as_str(), "ntfs3" | "ntfs"));
        assert_eq!(driver("ext4"), "ext4");
    }

    #[test]
    fn secrets_must_be_ours_private_and_named() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("rc-mount-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let uid = ids().0;
        let make = |name: &str, mode: u32| {
            let path = dir.join(name);
            fs::write(&path, "x").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            path
        };
        let good = make("rc-secret-1", 0o600);
        let open = make("rc-secret-2", 0o644);
        let other = make("password", 0o600);
        let link = dir.join("rc-secret-3");
        std::os::unix::fs::symlink(&good, &link).unwrap();
        assert!(secret_path(&good, uid).is_ok());
        assert!(secret_path(&good, uid + 1).is_err());
        assert!(secret_path(&open, uid).is_err());
        assert!(secret_path(&other, uid).is_err());
        assert!(secret_path(&link, uid).is_err());
        assert!(secret_path(Path::new("rc-secret-1"), uid).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn block_paths_stay_under_dev() {
        assert!(check_block_device("/etc/passwd").is_err());
        assert!(check_block_device("/dev/../etc/passwd").is_err());
        assert!(check_block_device("sda1").is_err());
        assert!(check_block_device("/dev/null").is_err());
    }
}
