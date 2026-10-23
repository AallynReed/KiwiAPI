//! `zipfile.ZipFile(BytesIO(), "w", ZIP_DEFLATED)` + `writestr(path, data)`
//! as CPython 3.13 lays it out on Linux: raw deflate at zlib's default level,
//! local headers rewritten with the real CRC and sizes, version 20, create
//! system 3, `0o600 << 16` attributes, the UTF-8 flag for non-ASCII names and
//! the local time of each `writestr` call.

use chrono::{Datelike, Local, Timelike};

use super::odb::deflate;

fn dos_time(clock: Option<[u16; 6]>) -> (u16, u16) {
    let dt = clock.unwrap_or_else(|| {
        let t = Local::now();
        [t.year() as u16, t.month() as u16, t.day() as u16, t.hour() as u16, t.minute() as u16, t.second() as u16]
    });
    let date = (dt[0].saturating_sub(1980) << 9) | (dt[1] << 5) | dt[2];
    let time = (dt[3] << 11) | (dt[4] << 5) | (dt[5] / 2);
    (date, time)
}

/// `_build_zip(files)`; `clock` pins `time.localtime()` for tests.
pub fn build_zip(files: &[(String, Vec<u8>)], clock: Option<[u16; 6]>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let name = match name.find('\0') {
            Some(i) => &name[..i],
            None => name.as_str(),
        };
        let (date, time) = dos_time(clock);
        let flags: u16 = if name.is_ascii() { 0 } else { 0x800 };
        let dir = name.ends_with('/');
        let attr: u32 = if dir { (0o40775 << 16) | 0x10 } else { 0o600 << 16 };
        let comp = deflate(&[data], -1, -15).unwrap_or_default();
        let crc = crc32fast::hash(data);
        let offset = out.len() as u32;
        let header = |o: &mut Vec<u8>, sig: &[u8; 4], central: bool| {
            o.extend_from_slice(sig);
            if central {
                o.push(20);
                o.push(3);
            }
            o.extend_from_slice(&20u16.to_le_bytes());
            o.extend_from_slice(&flags.to_le_bytes());
            o.extend_from_slice(&8u16.to_le_bytes());
            o.extend_from_slice(&time.to_le_bytes());
            o.extend_from_slice(&date.to_le_bytes());
            o.extend_from_slice(&crc.to_le_bytes());
            o.extend_from_slice(&(comp.len() as u32).to_le_bytes());
            o.extend_from_slice(&(data.len() as u32).to_le_bytes());
            o.extend_from_slice(&(name.len() as u16).to_le_bytes());
            o.extend_from_slice(&0u16.to_le_bytes());
        };
        header(&mut out, b"PK\x03\x04", false);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&comp);
        header(&mut central, b"PK\x01\x02", true);
        central.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        central.extend_from_slice(&attr.to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let start = out.len() as u32;
    let size = central.len() as u32;
    out.extend_from_slice(&central);
    let count = files.len() as u16;
    out.extend_from_slice(b"PK\x05\x06");
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&start.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}
