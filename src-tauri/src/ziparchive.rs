use std::io::{self, Seek, SeekFrom, Write};

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
    out: ArchiveOutput<W>,
    entries: Vec<Central>,
    time: u16,
    date: u16,
}

fn too_large() -> io::Error {
    io::Error::other("诊断包超过 4 GB 或条目过多，请缩小时间范围或减少原图")
}

struct ArchiveOutput<W> {
    inner: W,
    written: u64,
}

impl<W: Write> Write for ArchiveOutput<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() as u64 > u64::from(u32::MAX) - self.written {
            return Err(too_large());
        }
        let written = self.inner.write(bytes)?;
        self.written += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

struct DeflatedEntry<W: Write> {
    encoder: DeflateEncoder<W>,
    crc: crc32fast::Hasher,
    size: u32,
}

impl<W: Write> Write for DeflatedEntry<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() as u64 >= u64::from(u32::MAX - self.size) {
            return Err(too_large());
        }
        let written = self.encoder.write(bytes)?;
        self.crc.update(&bytes[..written]);
        self.size += written as u32;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.encoder.flush()
    }
}

impl<W: Write> ZipWriter<W> {
    pub fn new(out: W) -> Self {
        let now = Local::now();
        let time = ((now.hour() << 11) | (now.minute() << 5) | (now.second() / 2)) as u16;
        let date =
            ((((now.year() - 1980).max(0) as u32) << 9) | (now.month() << 5) | now.day()) as u16;
        Self {
            out: ArchiveOutput {
                inner: out,
                written: 0,
            },
            entries: Vec::new(),
            time,
            date,
        }
    }

    fn new_entry(&self, name: &str) -> io::Result<Central> {
        if self.entries.len() >= u16::MAX as usize - 1 {
            return Err(too_large());
        }
        let name = name.replace('\\', "/").into_bytes();
        u16::try_from(name.len()).map_err(|_| too_large())?;
        Ok(Central {
            name,
            method: 0,
            crc: 0,
            compressed: 0,
            size: 0,
            offset: u32::try_from(self.out.written).map_err(|_| too_large())?,
        })
    }

    fn local_header(&self, entry: &Central) -> Vec<u8> {
        let mut header = Vec::with_capacity(30 + entry.name.len());
        header.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        header.extend_from_slice(&20u16.to_le_bytes());
        header.extend_from_slice(&0x0800u16.to_le_bytes());
        header.extend_from_slice(&entry.method.to_le_bytes());
        header.extend_from_slice(&self.time.to_le_bytes());
        header.extend_from_slice(&self.date.to_le_bytes());
        header.extend_from_slice(&entry.crc.to_le_bytes());
        header.extend_from_slice(&entry.compressed.to_le_bytes());
        header.extend_from_slice(&entry.size.to_le_bytes());
        header.extend_from_slice(&(entry.name.len() as u16).to_le_bytes());
        header.extend_from_slice(&0u16.to_le_bytes());
        header.extend_from_slice(&entry.name);
        header
    }

    pub fn add(&mut self, name: &str, data: &[u8], compress: bool) -> io::Result<()> {
        let mut entry = self.new_entry(name)?;
        entry.size = u32::try_from(data.len()).map_err(|_| too_large())?;
        entry.crc = crc32fast::hash(data);
        let deflated;
        let (method, body): (u16, &[u8]) = if compress {
            let mut encoder =
                DeflateEncoder::new(Vec::with_capacity(data.len() / 2), Compression::default());
            encoder.write_all(data)?;
            deflated = encoder.finish()?;
            if deflated.len() < data.len() {
                (8, &deflated)
            } else {
                (0, data)
            }
        } else {
            (0, data)
        };
        entry.method = method;
        entry.compressed = u32::try_from(body.len()).map_err(|_| too_large())?;
        let header = self.local_header(&entry);
        self.out.write_all(&header)?;
        self.out.write_all(body)?;
        self.entries.push(entry);
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<W> {
        let start = u32::try_from(self.out.written).map_err(|_| too_large())?;
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
            self.out.write_all(&record)?;
        }
        let size = u32::try_from(self.out.written - u64::from(start)).map_err(|_| too_large())?;
        let mut end = Vec::with_capacity(22);
        end.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        end.extend_from_slice(&[0; 4]);
        end.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        end.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        end.extend_from_slice(&size.to_le_bytes());
        end.extend_from_slice(&start.to_le_bytes());
        end.extend_from_slice(&0u16.to_le_bytes());
        self.out.write_all(&end)?;
        self.out.flush()?;
        Ok(self.out.inner)
    }
}

impl<W: Write + Seek> ZipWriter<W> {
    pub fn add_stream(
        &mut self,
        name: &str,
        write: impl FnOnce(&mut dyn Write) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut entry = self.new_entry(name)?;
        entry.method = 8;
        let header = self.local_header(&entry);
        self.out.write_all(&header)?;
        let body_start = self.out.written;
        let (crc, size, end) = {
            let mut stream = DeflatedEntry {
                encoder: DeflateEncoder::new(&mut self.out, Compression::default()),
                crc: crc32fast::Hasher::new(),
                size: 0,
            };
            write(&mut stream)?;
            let DeflatedEntry { encoder, crc, size } = stream;
            let end = encoder.finish()?.written;
            (crc.finalize(), size, end)
        };
        entry.crc = crc;
        entry.size = size;
        entry.compressed = u32::try_from(end - body_start).map_err(|_| too_large())?;
        let header = self.local_header(&entry);
        self.out
            .inner
            .seek(SeekFrom::Start(u64::from(entry.offset)))?;
        self.out.inner.write_all(&header)?;
        self.out.inner.seek(SeekFrom::Start(end))?;
        self.entries.push(entry);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read};

    #[test]
    fn streamed_entry_preserves_its_payload_and_surrounding_entries() {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer.add("before.txt", b"before", false).unwrap();
        writer
            .add_stream("records/parts.jsonl", |out| {
                out.write_all("{\"sn\":\"工件一\"}\n".as_bytes())?;
                out.write_all("{\"sn\":\"工件二\"}\n".as_bytes())
            })
            .unwrap();
        writer.add("after.txt", b"after", true).unwrap();

        let mut archive = zip::ZipArchive::new(writer.finish().unwrap()).unwrap();
        for (name, expected) in [
            ("before.txt", "before"),
            (
                "records/parts.jsonl",
                "{\"sn\":\"工件一\"}\n{\"sn\":\"工件二\"}\n",
            ),
            ("after.txt", "after"),
        ] {
            let mut actual = String::new();
            archive
                .by_name(name)
                .unwrap()
                .read_to_string(&mut actual)
                .unwrap();
            assert_eq!(actual, expected);
        }
    }
}
