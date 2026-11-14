//! Finds and checks stand-in libraries.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

/// Export every stand-in has.
pub const ENTRY_SYMBOL: &str = "CreateInterface";
/// Name cargo gives the cdylib.
const CARGO_LIB: &str = "libsteamclient.so";

/// Architecture of an ELF library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfArch {
    X86_64,
    I386,
}

impl ElfArch {
    /// Layout folder name.
    pub fn sdk_dir(self) -> &'static str {
        match self {
            Self::X86_64 => "sdk64",
            Self::I386 => "sdk32",
        }
    }
}

/// Little-endian field reader.
struct Elf<'a> {
    bytes: &'a [u8],
    wide: bool,
}

impl Elf<'_> {
    fn u16(&self, at: usize) -> Option<u64> {
        let b = self.bytes.get(at..at + 2)?;
        Some(u64::from(u16::from_le_bytes([b[0], b[1]])))
    }

    fn u32(&self, at: usize) -> Option<u64> {
        let b = self.bytes.get(at..at + 4)?;
        Some(u64::from(u32::from_le_bytes(b.try_into().ok()?)))
    }

    fn u64(&self, at: usize) -> Option<u64> {
        let b = self.bytes.get(at..at + 8)?;
        Some(u64::from_le_bytes(b.try_into().ok()?))
    }

    /// Address-sized field.
    fn word(&self, at: usize) -> Option<u64> {
        if self.wide {
            self.u64(at)
        } else {
            self.u32(at)
        }
    }
}

/// Reads class and machine.
pub fn elf_arch(bytes: &[u8]) -> Result<ElfArch> {
    if bytes.get(..4) != Some(b"\x7fELF") {
        bail!("not an ELF file");
    }
    let class = bytes.get(4).copied().unwrap_or(0);
    if bytes.get(5) != Some(&1) {
        bail!("not a little-endian ELF file");
    }
    let machine = Elf { bytes, wide: false }
        .u16(0x12)
        .ok_or_else(|| anyhow!("truncated ELF header"))?;
    match (class, machine) {
        (2, 62) => Ok(ElfArch::X86_64),
        (1, 3) => Ok(ElfArch::I386),
        (1 | 2, m) => bail!("ELF for machine {m}, not x86_64 or i386"),
        (c, _) => bail!("unknown ELF class {c}"),
    }
}

/// `name` is a defined dynamic symbol.
pub fn exports_symbol(bytes: &[u8], name: &str) -> Result<bool> {
    let arch = elf_arch(bytes)?;
    let elf = Elf {
        bytes,
        wide: arch == ElfArch::X86_64,
    };
    let bad = || anyhow!("truncated or malformed ELF section table");
    let (shoff, shentsize, shnum) = if elf.wide {
        (elf.u64(0x28), elf.u16(0x3A), elf.u16(0x3C))
    } else {
        (elf.u32(0x20), elf.u16(0x2E), elf.u16(0x30))
    };
    let (shoff, shentsize, shnum) = (
        shoff.ok_or_else(bad)?,
        shentsize.ok_or_else(bad)?,
        shnum.ok_or_else(bad)?,
    );
    let section = |i: u64| -> Option<usize> { usize::try_from(shoff + i * shentsize).ok() };
    // Field offsets per class.
    let (off_at, size_at, link_at, entsize_at) = if elf.wide {
        (0x18, 0x20, 0x28, 0x38)
    } else {
        (0x10, 0x14, 0x18, 0x24)
    };
    let mut found_table = false;
    for i in 0..shnum {
        let at = section(i).ok_or_else(bad)?;
        // SHT_DYNSYM is 11.
        if elf.u32(at + 4).ok_or_else(bad)? != 11 {
            continue;
        }
        found_table = true;
        let sym_off = elf.word(at + off_at).ok_or_else(bad)?;
        let sym_size = elf.word(at + size_at).ok_or_else(bad)?;
        let link = elf.u32(at + link_at).ok_or_else(bad)?;
        let entsize = elf.word(at + entsize_at).ok_or_else(bad)?.max(1);
        let str_at = section(link).ok_or_else(bad)?;
        let str_off = elf.word(str_at + off_at).ok_or_else(bad)?;
        let str_size = elf.word(str_at + size_at).ok_or_else(bad)?;
        let strings = usize::try_from(str_off)
            .ok()
            .and_then(|o| bytes.get(o..o + usize::try_from(str_size).ok()?))
            .ok_or_else(bad)?;
        for n in 0..sym_size / entsize {
            let sym = usize::try_from(sym_off + n * entsize).map_err(|_| bad())?;
            let name_at = usize::try_from(elf.u32(sym).ok_or_else(bad)?).map_err(|_| bad())?;
            let shndx_at = if elf.wide { sym + 6 } else { sym + 14 };
            let defined = elf.u16(shndx_at).ok_or_else(bad)? != 0;
            let sym_name = strings
                .get(name_at..)
                .and_then(|s| s.split(|b| *b == 0).next())
                .unwrap_or_default();
            if defined && sym_name == name.as_bytes() {
                return Ok(true);
            }
        }
    }
    if !found_table {
        bail!("no dynamic symbol table");
    }
    Ok(false)
}

/// Checks one library; returns its arch.
pub fn inspect_library(path: &Path) -> Result<ElfArch> {
    let bytes = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    let arch =
        elf_arch(&bytes).with_context(|| format!("{} is not a usable library", path.display()))?;
    let exported = exports_symbol(&bytes, ENTRY_SYMBOL)
        .with_context(|| format!("{} has no readable symbol table", path.display()))?;
    if !exported {
        bail!(
            "{} does not export {ENTRY_SYMBOL}; it is not a stand-in steamclient.so",
            path.display()
        );
    }
    Ok(arch)
}

/// Libraries found for each arch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resolved {
    pub sdk64: Option<PathBuf>,
    pub sdk32: Option<PathBuf>,
}

/// Expands `~`, absolute against `cwd`.
pub fn absolute(input: &str, cwd: &Path, home: &Path) -> PathBuf {
    let input = input.trim();
    let path = match input.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            home.join(rest.trim_start_matches('/'))
        }
        _ => PathBuf::from(input),
    };
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

/// Library candidates inside a folder.
fn candidates(dir: &Path) -> Vec<PathBuf> {
    let i686 = "i686-unknown-linux-gnu/release";
    let x64 = "x86_64-unknown-linux-gnu/release";
    vec![
        // Installed layout.
        dir.join("sdk64/steamclient.so"),
        dir.join("sdk32/steamclient.so"),
        // Checkout, then target dir.
        dir.join("target/release").join(CARGO_LIB),
        dir.join("target").join(x64).join(CARGO_LIB),
        dir.join("target").join(i686).join(CARGO_LIB),
        dir.join("release").join(CARGO_LIB),
        dir.join(x64).join(CARGO_LIB),
        dir.join(i686).join(CARGO_LIB),
    ]
}

/// File or folder to checked libraries.
pub fn resolve(input: &str, cwd: &Path, home: &Path) -> Result<Resolved> {
    if input.trim().is_empty() {
        bail!("empty stand-in path");
    }
    let path = absolute(input, cwd, home);
    let path = path
        .canonicalize()
        .with_context(|| format!("{} does not exist", path.display()))?;
    let mut out = Resolved::default();
    let mut put = |file: PathBuf, arch: ElfArch| {
        let slot = match arch {
            ElfArch::X86_64 => &mut out.sdk64,
            ElfArch::I386 => &mut out.sdk32,
        };
        // First match per arch wins.
        if slot.is_none() {
            *slot = Some(file);
        }
    };
    if path.is_file() {
        let arch = inspect_library(&path)?;
        put(path, arch);
        return Ok(out);
    }
    let mut problems = Vec::new();
    for file in candidates(&path).into_iter().filter(|f| f.is_file()) {
        let file = file.canonicalize().unwrap_or(file);
        match inspect_library(&file) {
            Ok(arch) => put(file, arch),
            Err(e) => problems.push(format!("{e:#}")),
        }
    }
    if out.sdk64.is_none() && out.sdk32.is_none() {
        let tried: Vec<String> = candidates(&path)
            .iter()
            .map(|c| c.display().to_string())
            .collect();
        let mut msg = format!(
            "no stand-in steamclient.so found in {}; looked for {}",
            path.display(),
            tried.join(", ")
        );
        if !problems.is_empty() {
            msg.push_str(&format!(" ({})", problems.join("; ")));
        }
        bail!(msg);
    }
    Ok(out)
}

/// Installed name inside `sdk64`/`sdk32`.
pub const INSTALLED_NAME: &str = "steamclient.so";

/// Atomic copies into `dir`, mode 0644.
pub fn install(resolved: &Resolved, dir: &Path) -> Result<Resolved> {
    let mut out = Resolved::default();
    for (src, arch, slot) in [
        (&resolved.sdk64, ElfArch::X86_64, &mut out.sdk64),
        (&resolved.sdk32, ElfArch::I386, &mut out.sdk32),
    ] {
        let Some(src) = src else { continue };
        let sdk = dir.join(arch.sdk_dir());
        std::fs::create_dir_all(&sdk)
            .with_context(|| format!("cannot create {}", sdk.display()))?;
        let dest = sdk.join(INSTALLED_NAME);
        let same = dest.canonicalize().ok() == src.canonicalize().ok();
        if !same {
            let tmp = sdk.join(format!(".{INSTALLED_NAME}.{}.tmp", std::process::id()));
            let copied = std::fs::copy(src, &tmp)
                .with_context(|| format!("cannot copy {} to {}", src.display(), tmp.display()))
                .and_then(|_| set_mode(&tmp))
                .and_then(|()| {
                    std::fs::rename(&tmp, &dest)
                        .with_context(|| format!("cannot install {}", dest.display()))
                });
            if let Err(e) = copied {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
        }
        write_hash(&dest)?;
        *slot = Some(dest);
    }
    Ok(out)
}

/// Sidecar holding the expected hash.
pub fn hash_file(lib: &Path) -> PathBuf {
    let mut name = lib.as_os_str().to_owned();
    name.push(".sha256");
    PathBuf::from(name)
}

/// Lowercase hex SHA-256 of a file.
pub fn sha256_hex(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut file =
        std::fs::File::open(path).with_context(|| format!("cannot read {}", path.display()))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)
        .with_context(|| format!("cannot read {}", path.display()))?;
    Ok(hex::encode(hasher.finalize()))
}

/// Records the hash next to `lib`.
fn write_hash(lib: &Path) -> Result<()> {
    let hash = sha256_hex(lib)?;
    let sidecar = hash_file(lib);
    let tmp = sidecar.with_extension(format!("sha256.{}.tmp", std::process::id()));
    std::fs::write(&tmp, format!("{hash}\n"))
        .and_then(|()| std::fs::rename(&tmp, &sidecar))
        .with_context(|| format!("cannot write {}", sidecar.display()))
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
}

/// Recorded hash matches the file.
pub fn verify(lib: &Path) -> Result<()> {
    if !lib.is_file() {
        bail!(
            "stand-in library {} is missing; reinstall it with \
             `aurelia config steamworks-client --path <libsteamclient.so>`",
            lib.display()
        );
    }
    let sidecar = hash_file(lib);
    let expected = std::fs::read_to_string(&sidecar).map_err(|_| {
        anyhow!(
            "no recorded hash for the stand-in library {} ({} missing); reinstall it with \
             `aurelia config steamworks-client --path <libsteamclient.so>`",
            lib.display(),
            sidecar.display()
        )
    })?;
    let expected = expected.trim().to_ascii_lowercase();
    let actual = sha256_hex(lib)?;
    if expected != actual {
        bail!(
            "refusing to launch: the stand-in library {} changed since it was installed \
             (expected SHA-256 {expected}, found {actual}); if you rebuilt it, reinstall with \
             `aurelia config steamworks-client --path <libsteamclient.so>`",
            lib.display()
        );
    }
    Ok(())
}

#[cfg(unix)]
fn set_mode(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644))
        .with_context(|| format!("cannot set mode of {}", path.display()))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
#[path = "stand_in_lib_tests.rs"]
mod tests;
