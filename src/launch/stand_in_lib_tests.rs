use super::*;

/// Minimal ELF with a `.dynsym`.
fn fixture(wide: bool, machine: u16, symbols: &[(&str, bool)]) -> Vec<u8> {
    let mut strtab = vec![0u8];
    let mut names = Vec::new();
    for (name, _) in symbols {
        names.push(strtab.len() as u32);
        strtab.extend_from_slice(name.as_bytes());
        strtab.push(0);
    }
    let (ehsize, shentsize, symsize) = if wide { (64, 64, 24) } else { (52, 40, 16) };
    let str_off = ehsize;
    let sym_off = str_off + strtab.len();
    let mut syms = vec![0u8; symsize]; // Null symbol.
    for ((_, defined), name) in symbols.iter().zip(&names) {
        let mut s = vec![0u8; symsize];
        s[0..4].copy_from_slice(&name.to_le_bytes());
        let shndx: u16 = if *defined { 7 } else { 0 };
        let at = if wide { 6 } else { 14 };
        s[at..at + 2].copy_from_slice(&shndx.to_le_bytes());
        syms.extend(s);
    }
    let shoff = sym_off + syms.len();
    let mut out = vec![0u8; ehsize];
    out[..4].copy_from_slice(b"\x7fELF");
    out[4] = if wide { 2 } else { 1 };
    out[5] = 1;
    out[6] = 1;
    out[0x12..0x14].copy_from_slice(&machine.to_le_bytes());
    if wide {
        out[0x28..0x30].copy_from_slice(&(shoff as u64).to_le_bytes());
        out[0x3A..0x3C].copy_from_slice(&(shentsize as u16).to_le_bytes());
        out[0x3C..0x3E].copy_from_slice(&3u16.to_le_bytes());
    } else {
        out[0x20..0x24].copy_from_slice(&(shoff as u32).to_le_bytes());
        out[0x2E..0x30].copy_from_slice(&(shentsize as u16).to_le_bytes());
        out[0x30..0x32].copy_from_slice(&3u16.to_le_bytes());
    }
    out.extend(&strtab);
    out.extend(&syms);
    // Sections: null, dynstr, dynsym.
    let section = |kind: u32, off: usize, size: usize, link: u32, ent: usize| {
        let mut h = vec![0u8; shentsize];
        h[4..8].copy_from_slice(&kind.to_le_bytes());
        if wide {
            h[0x18..0x20].copy_from_slice(&(off as u64).to_le_bytes());
            h[0x20..0x28].copy_from_slice(&(size as u64).to_le_bytes());
            h[0x28..0x2C].copy_from_slice(&link.to_le_bytes());
            h[0x38..0x40].copy_from_slice(&(ent as u64).to_le_bytes());
        } else {
            h[0x10..0x14].copy_from_slice(&(off as u32).to_le_bytes());
            h[0x14..0x18].copy_from_slice(&(size as u32).to_le_bytes());
            h[0x18..0x1C].copy_from_slice(&link.to_le_bytes());
            h[0x24..0x28].copy_from_slice(&(ent as u32).to_le_bytes());
        }
        h
    };
    out.extend(vec![0u8; shentsize]);
    out.extend(section(3, str_off, strtab.len(), 0, 0));
    out.extend(section(11, sym_off, syms.len(), 1, symsize));
    out
}

fn lib64() -> Vec<u8> {
    fixture(true, 62, &[("malloc", false), (ENTRY_SYMBOL, true)])
}

fn lib32() -> Vec<u8> {
    fixture(false, 3, &[(ENTRY_SYMBOL, true)])
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn elf_header_gives_the_arch() {
    assert_eq!(elf_arch(&lib64()).unwrap(), ElfArch::X86_64);
    assert_eq!(elf_arch(&lib32()).unwrap(), ElfArch::I386);
    assert!(elf_arch(b"#!/bin/sh\n").is_err());
    let arm = fixture(true, 183, &[(ENTRY_SYMBOL, true)]);
    assert!(elf_arch(&arm).unwrap_err().to_string().contains("183"));
}

#[test]
fn only_defined_symbols_count() {
    assert!(exports_symbol(&lib64(), ENTRY_SYMBOL).unwrap());
    assert!(exports_symbol(&lib32(), ENTRY_SYMBOL).unwrap());
    // Imported (undefined) does not count.
    let import = fixture(true, 62, &[(ENTRY_SYMBOL, false)]);
    assert!(!exports_symbol(&import, ENTRY_SYMBOL).unwrap());
    assert!(!exports_symbol(&lib64(), "malloc").unwrap());
}

#[test]
fn real_system_library_parses() {
    // Host libc, when present.
    let Some(libc) = ["/usr/lib/libc.so.6", "/lib/x86_64-linux-gnu/libc.so.6"]
        .iter()
        .map(Path::new)
        .find(|p| p.is_file())
    else {
        return;
    };
    let bytes = std::fs::read(libc).unwrap();
    assert!(exports_symbol(&bytes, "malloc").unwrap());
    assert!(!exports_symbol(&bytes, ENTRY_SYMBOL).unwrap());
}

#[test]
fn paths_become_absolute() {
    let cwd = Path::new("/work");
    let home = Path::new("/home/u");
    assert_eq!(absolute("lib.so", cwd, home), PathBuf::from("/work/lib.so"));
    assert_eq!(
        absolute("~/x/lib.so", cwd, home),
        PathBuf::from("/home/u/x/lib.so")
    );
    assert_eq!(absolute("~", cwd, home), PathBuf::from("/home/u"));
    assert_eq!(absolute("/abs.so", cwd, home), PathBuf::from("/abs.so"));
    assert_eq!(
        absolute("~other/x", cwd, home),
        PathBuf::from("/work/~other/x")
    );
}

#[test]
fn single_files_go_by_their_arch() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("any-name.so"), &lib32());
    let found = resolve("any-name.so", dir.path(), dir.path()).unwrap();
    assert_eq!(found.sdk64, None);
    assert_eq!(
        found.sdk32,
        Some(dir.path().join("any-name.so").canonicalize().unwrap())
    );
}

#[test]
fn bad_files_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("text.so"), b"not elf");
    write(
        &dir.path().join("nolib.so"),
        &fixture(true, 62, &[("other", true)]),
    );
    let err = resolve("text.so", dir.path(), dir.path()).unwrap_err();
    assert!(format!("{err:#}").contains("not an ELF"), "{err:#}");
    let err = resolve("nolib.so", dir.path(), dir.path()).unwrap_err();
    assert!(format!("{err:#}").contains(ENTRY_SYMBOL), "{err:#}");
    assert!(resolve("missing.so", dir.path(), dir.path()).is_err());
    assert!(resolve("  ", dir.path(), dir.path()).is_err());
}

#[test]
fn checkout_and_target_dirs_are_searched() {
    let checkout = tempfile::tempdir().unwrap();
    let t = checkout.path().join("target");
    write(&t.join("release/libsteamclient.so"), &lib64());
    write(
        &t.join("i686-unknown-linux-gnu/release/libsteamclient.so"),
        &lib32(),
    );
    for input in [checkout.path(), t.as_path()] {
        let found = resolve(input.to_str().unwrap(), Path::new("/"), Path::new("/")).unwrap();
        assert!(found.sdk64.unwrap().ends_with("release/libsteamclient.so"));
        assert!(
            found
                .sdk32
                .unwrap()
                .ends_with("i686-unknown-linux-gnu/release/libsteamclient.so")
        );
    }
}

#[test]
fn layout_dir_and_empty_dirs() {
    let layout = tempfile::tempdir().unwrap();
    write(&layout.path().join("sdk64/steamclient.so"), &lib64());
    let found = resolve(
        layout.path().to_str().unwrap(),
        Path::new("/"),
        Path::new("/"),
    )
    .unwrap();
    assert!(found.sdk64.is_some() && found.sdk32.is_none());
    let empty = tempfile::tempdir().unwrap();
    let err = resolve(
        empty.path().to_str().unwrap(),
        Path::new("/"),
        Path::new("/"),
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("no stand-in steamclient.so found"),
        "{err}"
    );
}

#[test]
fn install_copies_by_arch_and_replaces() {
    let src = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    write(&src.path().join("a.so"), &lib64());
    let found = resolve("a.so", src.path(), src.path()).unwrap();
    let installed = install(&found, dest.path()).unwrap();
    let so64 = dest.path().join("sdk64/steamclient.so");
    assert_eq!(installed.sdk64, Some(so64.clone()));
    assert_eq!(installed.sdk32, None);
    assert_eq!(std::fs::read(&so64).unwrap(), lib64());
    assert!(!dest.path().join("sdk32").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&so64).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o644);
    }
    // A rebuilt library replaces it.
    let rebuilt = fixture(true, 62, &[(ENTRY_SYMBOL, true), ("extra", true)]);
    write(&src.path().join("a.so"), &rebuilt);
    install(&found, dest.path()).unwrap();
    assert_eq!(std::fs::read(&so64).unwrap(), rebuilt);
    // No temp files left over.
    let names: Vec<_> = std::fs::read_dir(dest.path().join("sdk64"))
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(names.len(), 1);
    // Reinstalling the copy: no-op.
    let again = resolve(so64.to_str().unwrap(), Path::new("/"), Path::new("/")).unwrap();
    install(&again, dest.path()).unwrap();
    assert_eq!(std::fs::read(&so64).unwrap(), rebuilt);
}
