//! Darwin's complete Unix PCB inventories, including bound nonlisteners.
//!
//! ABI: Apple XNU bsd/sys/unpcb.h `xunpcb64` and socketvar.h `xsocket64`
//! use pack(4). unp_pcblist64 in bsd/kern/uipc_usrreq.c exports every PCB
//! without a per-user filter, bracketed by xunpgen. Unknown layouts, partial
//! reads and changing generations never authorize an unlink.

use std::{
    ffi::CStr,
    fs, io,
    os::unix::ffi::OsStringExt,
    path::{Path, PathBuf},
};

const STREAM: &CStr = c"net.local.stream.pcblist64";
const DGRAM: &CStr = c"net.local.dgram.pcblist64";
const GENERATION_LEN: usize = 24;
const PCB_LEN: usize = 724;

pub(super) fn read_table(name: &CStr) -> io::Result<Vec<u8>> {
    let mut length = 0;
    // SAFETY: the name is NUL terminated and the size output is writable.
    if unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    if !(GENERATION_LEN..=16 * 1024 * 1024).contains(&length) {
        return Err(io::Error::other("unknown Unix PCB inventory size"));
    }
    let mut bytes = vec![0; length];
    // SAFETY: bytes owns length writable bytes; sysctl reports its actual size.
    if unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            bytes.as_mut_ptr().cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    if length > bytes.len() {
        return Err(io::Error::other("incomplete Unix PCB inventory"));
    }
    bytes.truncate(length);
    Ok(bytes)
}

#[derive(PartialEq, Eq)]
struct Generation {
    count: u32,
    pcb: u64,
}

fn generation(bytes: &[u8]) -> Option<Generation> {
    if bytes.len() != GENERATION_LEN || u32_at(bytes, 0)? != GENERATION_LEN as u32 {
        return None;
    }
    Some(Generation {
        count: u32_at(bytes, 4)?,
        pcb: u64_at(bytes, 8)?,
    })
}

struct Table {
    generation: Generation,
    count: usize,
    paths: Vec<PathBuf>,
}

fn parse(bytes: &[u8], socket_type: i32) -> Option<Table> {
    let first = generation(bytes.get(..GENERATION_LEN)?)?;
    // XNU omits the closing generation only for a globally empty table.
    if bytes.len() == GENERATION_LEN && first.count == 0 {
        return Some(Table {
            generation: first,
            count: 0,
            paths: vec![],
        });
    }
    let end = bytes.len().checked_sub(GENERATION_LEN)?;
    if first != generation(bytes.get(end..)?)? {
        return None;
    }
    let records = bytes.get(GENERATION_LEN..end)?;
    if records.len() % PCB_LEN != 0 {
        return None;
    }
    let count = records.len() / PCB_LEN;
    if count > first.count as usize {
        return None;
    }
    let mut paths = Vec::new();
    for record in records.chunks_exact(PCB_LEN) {
        // Pin both exported structure lengths, socket family/type, and the
        // PCB generation before trusting the packed sockaddr at offset 104.
        if u32_at(record, 0)? != PCB_LEN as u32
            || u32_at(record, 616)? != 108
            || u16_at(record, 628)? != socket_type as u16
            || u32_at(record, 648)? != libc::AF_UNIX as u32
            || u64_at(record, 92)? > first.pcb
        {
            return None;
        }
        let length = record[104] as usize;
        if length == 0 {
            if record[105] != 0 {
                return None;
            }
            continue; // Unnamed socket, not a filesystem holder.
        }
        if !(2..=106).contains(&length) || record[105] != libc::AF_UNIX as u8 {
            return None;
        }
        let address = &record[106..104 + length];
        let nul = address
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(address.len());
        if address[nul..].iter().any(|byte| *byte != 0) {
            return None;
        }
        if nul != 0 {
            paths.push(PathBuf::from(std::ffi::OsString::from_vec(
                address[..nul].to_vec(),
            )));
        }
    }
    Some(Table {
        generation: first,
        count,
        paths,
    })
}

pub(super) fn proves_unheld(
    path: &Path,
    candidate: &fs::Metadata,
    mut read: impl FnMut(&CStr) -> io::Result<Vec<u8>>,
) -> bool {
    use std::os::unix::fs::MetadataExt;
    // Both protocol lists share a global PCB generation/count. Bounded
    // recollection handles churn between reads; uncertainty retains the path.
    for _ in 0..3 {
        let Some(stream) = read(STREAM)
            .ok()
            .and_then(|bytes| parse(&bytes, libc::SOCK_STREAM))
        else {
            return false;
        };
        let Some(dgram) = read(DGRAM)
            .ok()
            .and_then(|bytes| parse(&bytes, libc::SOCK_DGRAM))
        else {
            return false;
        };
        if stream.generation != dgram.generation {
            continue;
        }
        if stream.count + dgram.count != stream.generation.count as usize {
            return false;
        }
        for held in stream.paths.iter().chain(&dgram.paths) {
            if held == path {
                return false;
            }
            // As on Linux, recognize resolvable filesystem aliases too. The
            // complete kernel table (not filesystem lookup success) proves
            // holder absence; unrelated private/relative names are common.
            if let Ok(meta) = fs::symlink_metadata(held) {
                if meta.dev() == candidate.dev() && meta.ino() == candidate.ino() {
                    return false;
                }
            }
        }
        return true;
    }
    false
}

fn u16_at(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_ne_bytes(
        bytes.get(offset..offset + 2)?.try_into().ok()?,
    ))
}
fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_ne_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}
fn u64_at(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_ne_bytes(
        bytes.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    #[test]
    fn uncertain_inventory_and_replaced_inode_retain_owned_socket_cas_f2c7() {
        let home = tempfile::tempdir_in("/tmp").unwrap();
        let sessions = home.path().join("sessions");
        fs::create_dir(&sessions).unwrap();
        let path = home.path().join("factory-proof.gui.sock");
        drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
        let _lock = super::super::transition_lock(home.path()).unwrap();

        let header = |count: u32| {
            let mut bytes = vec![0u8; GENERATION_LEN];
            bytes[..4].copy_from_slice(&(GENERATION_LEN as u32).to_ne_bytes());
            bytes[4..8].copy_from_slice(&count.to_ne_bytes());
            bytes[8..16].copy_from_slice(&1u64.to_ne_bytes());
            bytes
        };
        let mut record = vec![0u8; PCB_LEN];
        record[..4].copy_from_slice(&(PCB_LEN as u32).to_ne_bytes());
        record[616..620].copy_from_slice(&108u32.to_ne_bytes());
        record[628..630].copy_from_slice(&(libc::SOCK_STREAM as u16).to_ne_bytes());
        record[648..652].copy_from_slice(&(libc::AF_UNIX as u32).to_ne_bytes());
        let mut valid = header(1);
        valid.extend(record);
        valid.extend(header(1));
        let mut truncated = valid.clone();
        truncated.pop();
        let mut unknown_abi = valid.clone();
        unknown_abi[GENERATION_LEN..GENERATION_LEN + 4].copy_from_slice(&728u32.to_ne_bytes());
        let mut changed_generation = valid.clone();
        let end = changed_generation.len();
        changed_generation[end - 16..end - 8].copy_from_slice(&2u64.to_ne_bytes());
        // A syntactically valid but incomplete global inventory is unknown.
        let mut missing_protocol = header(1);
        missing_protocol.extend(header(1));
        for (label, stream) in [
            ("permission", None),
            ("truncated", Some(truncated)),
            ("ABI", Some(unknown_abi)),
            ("generation", Some(changed_generation)),
            ("missing protocol", Some(missing_protocol)),
        ] {
            super::super::remove_locked_with(&path, &sessions, false, |path, meta| {
                proves_unheld(path, meta, |name| match &stream {
                    None => Err(io::Error::from(io::ErrorKind::PermissionDenied)),
                    Some(bytes) if name == STREAM => Ok(bytes.clone()),
                    Some(_) => {
                        let mut empty = header(1);
                        empty.extend(header(1));
                        Ok(empty)
                    }
                })
            })
            .unwrap();
            assert!(path.exists(), "{label} must retain the socket");
        }

        let original = fs::symlink_metadata(&path).unwrap().ino();
        let mut replacement = None;
        super::super::remove_locked_with(&path, &sessions, false, |path, _| {
            fs::rename(path, home.path().join("original.sock")).unwrap();
            replacement = Some(std::os::unix::net::UnixListener::bind(path).unwrap());
            true
        })
        .unwrap();
        assert!(
            path.exists(),
            "replacement must survive an earlier absence proof"
        );
        assert_ne!(fs::symlink_metadata(&path).unwrap().ino(), original);
        drop(replacement);
    }
}
