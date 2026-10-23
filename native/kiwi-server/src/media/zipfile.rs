//! `zipfile.ZipFile(BytesIO(), "w", ZIP_STORED)` + `writestr(name, data)`
//! as CPython 3.13 lays it out on Linux: local headers rewritten with the
//! real CRC and sizes (no data descriptor), version 20, create system 3,
//! external attributes `0o600 << 16`, the UTF-8 flag only for non-ASCII
//! names, and the local time of each `writestr` call.

use chrono::{Datelike, Local, Timelike};

struct Entry {
    name: Vec<u8>,
    flags: u16,
    time: u16,
    date: u16,
    crc: u32,
    size: u32,
    offset: u32,
}

pub struct ZipWriter {
    out: Vec<u8>,
    entries: Vec<Entry>,
    /// Fixed `date_time` for tests; None is `time.localtime()`.
    pub clock: Option<[u16; 6]>,
}

impl Default for ZipWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl ZipWriter {
    pub fn new() -> Self {
        ZipWriter { out: Vec::new(), entries: Vec::new(), clock: None }
    }

    pub fn len(&self) -> usize {
        self.out.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn now(&self) -> [u16; 6] {
        self.clock.unwrap_or_else(|| {
            let t = Local::now();
            [t.year() as u16, t.month() as u16, t.day() as u16, t.hour() as u16, t.minute() as u16, t.second() as u16]
        })
    }

    pub fn writestr(&mut self, name: &str, data: &[u8]) {
        let dt = self.now();
        let date = (dt[0].saturating_sub(1980) << 9) | (dt[1] << 5) | dt[2];
        let time = (dt[3] << 11) | (dt[4] << 5) | (dt[5] / 2);
        let flags = if name.is_ascii() { 0 } else { 0x800 };
        let name = name.as_bytes().to_vec();
        let e = Entry {
            flags,
            time,
            date,
            crc: crc32fast::hash(data),
            size: data.len() as u32,
            offset: self.out.len() as u32,
            name,
        };
        let o = &mut self.out;
        o.extend_from_slice(b"PK\x03\x04");
        o.extend_from_slice(&20u16.to_le_bytes());
        o.extend_from_slice(&e.flags.to_le_bytes());
        o.extend_from_slice(&0u16.to_le_bytes());
        o.extend_from_slice(&e.time.to_le_bytes());
        o.extend_from_slice(&e.date.to_le_bytes());
        o.extend_from_slice(&e.crc.to_le_bytes());
        o.extend_from_slice(&e.size.to_le_bytes());
        o.extend_from_slice(&e.size.to_le_bytes());
        o.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
        o.extend_from_slice(&0u16.to_le_bytes());
        o.extend_from_slice(&e.name);
        o.extend_from_slice(data);
        self.entries.push(e);
    }

    pub fn finish(mut self) -> Vec<u8> {
        let start = self.out.len() as u32;
        let o = &mut self.out;
        for e in &self.entries {
            o.extend_from_slice(b"PK\x01\x02");
            o.push(20);
            o.push(3);
            o.extend_from_slice(&20u16.to_le_bytes());
            o.extend_from_slice(&e.flags.to_le_bytes());
            o.extend_from_slice(&0u16.to_le_bytes());
            o.extend_from_slice(&e.time.to_le_bytes());
            o.extend_from_slice(&e.date.to_le_bytes());
            o.extend_from_slice(&e.crc.to_le_bytes());
            o.extend_from_slice(&e.size.to_le_bytes());
            o.extend_from_slice(&e.size.to_le_bytes());
            o.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
            o.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
            o.extend_from_slice(&(0o600u32 << 16).to_le_bytes());
            o.extend_from_slice(&e.offset.to_le_bytes());
            o.extend_from_slice(&e.name);
        }
        let size = o.len() as u32 - start;
        let count = self.entries.len() as u16;
        o.extend_from_slice(b"PK\x05\x06");
        o.extend_from_slice(&[0, 0, 0, 0]);
        o.extend_from_slice(&count.to_le_bytes());
        o.extend_from_slice(&count.to_le_bytes());
        o.extend_from_slice(&size.to_le_bytes());
        o.extend_from_slice(&start.to_le_bytes());
        o.extend_from_slice(&0u16.to_le_bytes());
        self.out
    }
}
