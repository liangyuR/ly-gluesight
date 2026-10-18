use std::io::{self, Write};

use chrono::{Datelike, Local, Timelike};
use flate2::write::DeflateEncoder;
use flate2::Compression;

struct Central {
    name: Vec<u8>,
    method: u16,
    crc: u32,
    compressed: u32,
    size: u32,
    offset: u32,
}

/// 最小 ZIP 写入器：只写普通（非 ZIP64）条目，文件名按 UTF-8 标记；CRC32 是 ZIP 格式要求的传输校验，不参与任何业务匹配。
pub struct ZipWriter<W: Write> {
    out: W,
    written: u64,
    entries: Vec<Central>,
    time: u16,
    date: u16,
}

fn too_large() -> io::Error {
    io::Error::other("诊断包超过 4 GB 或条目过多，请缩小时间范围或减少原图")
}

impl<W: Write> ZipWriter<W> {
    pub fn new(out: W) -> Self {
        let now = Local::now();
        let time = ((now.hour() << 11) | (now.minute() << 5) | (now.second() / 2)) as u16;
        let date = ((((now.year() - 1980).max(0) as u32) << 9) | (now.month() << 5) | now.day()) as u16;
        Self { out, written: 0, entries: Vec::new(), time, date }
    }

    fn put(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.out.write_all(bytes)?;
        self.written += bytes.len() as u64;
        Ok(())
    }

    pub fn add(&mut self, name: &str, data: &[u8], compress: bool) -> io::Result<()> {
        if self.entries.len() >= u16::MAX as usize - 1 { return Err(too_large()); }
        let name = name.replace('\\', "/").into_bytes();
        let crc = crc32fast::hash(data);
        let deflated;
        let (method, body): (u16, &[u8]) = if compress {
            let mut encoder = DeflateEncoder::new(Vec::with_capacity(data.len() / 2), Compression::default());
            encoder.write_all(data)?;
            deflated = encoder.finish()?;
            if deflated.len() < data.len() { (8, &deflated) } else { (0, data) }
        } else { (0, data) };
        let size = u32::try_from(data.len()).map_err(|_| too_large())?;
        let compressed = u32::try_from(body.len()).map_err(|_| too_large())?;
        let offset = u32::try_from(self.written).map_err(|_| too_large())?;
        let mut header = Vec::with_capacity(30 + name.len());
        header.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        header.extend_from_slice(&20u16.to_le_bytes());
        header.extend_from_slice(&0x0800u16.to_le_bytes());
        header.extend_from_slice(&method.to_le_bytes());
        header.extend_from_slice(&self.time.to_le_bytes());
        header.extend_from_slice(&self.date.to_le_bytes());
        header.extend_from_slice(&crc.to_le_bytes());
        header.extend_from_slice(&compressed.to_le_bytes());
        header.extend_from_slice(&size.to_le_bytes());
        header.extend_from_slice(&(name.len() as u16).to_le_bytes());
        header.extend_from_slice(&0u16.to_le_bytes());
        header.extend_from_slice(&name);
        self.put(&header)?;
        self.put(body)?;
        if self.written > u32::MAX as u64 { return Err(too_large()); }
        self.entries.push(Central { name, method, crc, compressed, size, offset });
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<W> {
        let start = u32::try_from(self.written).map_err(|_| too_large())?;
        let entries = std::mem::take(&mut self.entries);
        for e in &entries {
            let mut record = Vec::with_capacity(46 + e.name.len());
            record.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            record.extend_from_slice(&20u16.to_le_bytes());
            record.extend_from_slice(&20u16.to_le_bytes());
            record.extend_from_slice(&0x0800u16.to_le_bytes());
            record.extend_from_slice(&e.method.to_le_bytes());
            record.extend_from_slice(&self.time.to_le_bytes());
            record.extend_from_slice(&self.date.to_le_bytes());
            record.extend_from_slice(&e.crc.to_le_bytes());
            record.extend_from_slice(&e.compressed.to_le_bytes());
            record.extend_from_slice(&e.size.to_le_bytes());
            record.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
            record.extend_from_slice(&[0; 12]);
            record.extend_from_slice(&e.offset.to_le_bytes());
            record.extend_from_slice(&e.name);
            self.put(&record)?;
        }
        let size = u32::try_from(self.written - start as u64).map_err(|_| too_large())?;
        let mut end = Vec::with_capacity(22);
        end.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        end.extend_from_slice(&[0; 4]);
        end.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        end.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        end.extend_from_slice(&size.to_le_bytes());
        end.extend_from_slice(&start.to_le_bytes());
        end.extend_from_slice(&0u16.to_le_bytes());
        self.put(&end)?;
        self.out.flush()?;
        Ok(self.out)
    }
}
