// Module `pkg`: reads the Wallpaper Engine archive format (scene.pkg, "PKGV").
// It is a directory-style archive: header + entry list + data blob.
//
// Layout (little-endian):
//   [u32 len][bytes]            version (sized string, starts with "PKGV")
//   [u32 filesCount]
//   filesCount x:
//     [u32 len][bytes]          file name (sized string)
//     [u32 offset]              data offset, relative to the base
//     [u32 length]              data length
//   <base_offset = here>        start of the blob; data i at [base+offset, +length]

use std::path::Path;

struct Entry {
    name: String,
    offset: u32,
    length: u32,
}

pub struct Pkg {
    data: Vec<u8>,
    base_offset: usize,
    entries: Vec<Entry>,
}

// Simple sequential reader over a &[u8] (cursor + little-endian helpers).
struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn u32(&mut self) -> Option<u32> {
        let bytes = self.data.get(self.pos..self.pos + 4)?;
        self.pos += 4;
        Some(u32::from_le_bytes(bytes.try_into().unwrap()))
    }

    // "sized string": a u32 with the length, followed by the bytes.
    fn sized_string(&mut self) -> Option<String> {
        let len = self.u32()? as usize;
        let bytes = self.data.get(self.pos..self.pos + len)?;
        self.pos += len;
        Some(String::from_utf8_lossy(bytes).into_owned())
    }
}

impl Pkg {
    pub fn open(path: &Path) -> std::io::Result<Pkg> {
        let data = std::fs::read(path)?;
        Pkg::parse(data).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    // PURE parse (bytes -> index). Does not touch the disk -> testable.
    pub fn parse(data: Vec<u8>) -> Result<Pkg, String> {
        let mut c = Cursor {
            data: &data,
            pos: 0,
        };

        let version = c.sized_string().ok_or("header ausente")?;
        if !version.starts_with("PKGV") {
            return Err(format!("header inesperado: {version:?} (esperava PKGV*)"));
        }

        let count = c.u32().ok_or("file count missing")?;
        let mut entries = Vec::with_capacity(count as usize);
        for i in 0..count {
            let name = c.sized_string().ok_or(format!("file name {i} missing"))?;
            let offset = c.u32().ok_or(format!("offset of file {i} missing"))?;
            let length = c.u32().ok_or(format!("length of file {i} missing"))?;
            entries.push(Entry {
                name,
                offset,
                length,
            });
        }

        let base_offset = c.pos;
        Ok(Pkg {
            data,
            base_offset,
            entries,
        })
    }

    // Names of the files contained in the package.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|e| e.name.as_str())
    }

    // Bytes of a file by name (None if it does not exist or is out of bounds).
    pub fn read(&self, name: &str) -> Option<&[u8]> {
        let e = self.entries.iter().find(|e| e.name == name)?;
        let start = self.base_offset + e.offset as usize;
        let end = start + e.length as usize;
        self.data.get(start..end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Builds a minimal .pkg in memory with one file "a.txt" = "hello".
    fn build_minimal_pkg() -> Vec<u8> {
        let mut b = Vec::new();
        let sized = |b: &mut Vec<u8>, s: &str| {
            b.extend_from_slice(&(s.len() as u32).to_le_bytes());
            b.extend_from_slice(s.as_bytes());
        };
        sized(&mut b, "PKGV0001"); // version
        b.extend_from_slice(&1u32.to_le_bytes()); // 1 file
        sized(&mut b, "a.txt"); // name
        b.extend_from_slice(&0u32.to_le_bytes()); // offset 0
        b.extend_from_slice(&5u32.to_le_bytes()); // length 5
        b.extend_from_slice(b"hello"); // data blob
        b
    }

    #[test]
    fn lists_and_reads_entry() {
        let pkg = Pkg::parse(build_minimal_pkg()).unwrap();
        assert_eq!(pkg.names().collect::<Vec<_>>(), vec!["a.txt"]);
        assert_eq!(pkg.read("a.txt"), Some(&b"hello"[..]));
    }

    #[test]
    fn unknown_file_is_none() {
        let pkg = Pkg::parse(build_minimal_pkg()).unwrap();
        assert_eq!(pkg.read("nao-existe"), None);
    }

    #[test]
    fn rejects_bad_header() {
        // "XXXX" does not start with PKGV.
        let mut b = Vec::new();
        b.extend_from_slice(&4u32.to_le_bytes());
        b.extend_from_slice(b"XXXX");
        assert!(Pkg::parse(b).is_err());
    }

    #[test]
    fn two_files_with_offsets() {
        // Checks that offsets relative to the base point to the right slice of the blob.
        let mut b = Vec::new();
        let sized = |b: &mut Vec<u8>, s: &str| {
            b.extend_from_slice(&(s.len() as u32).to_le_bytes());
            b.extend_from_slice(s.as_bytes());
        };
        sized(&mut b, "PKGV0001");
        b.extend_from_slice(&2u32.to_le_bytes());
        sized(&mut b, "one");
        b.extend_from_slice(&0u32.to_le_bytes()); // offset 0
        b.extend_from_slice(&3u32.to_le_bytes()); // "AAA"
        sized(&mut b, "two");
        b.extend_from_slice(&3u32.to_le_bytes()); // offset 3
        b.extend_from_slice(&2u32.to_le_bytes()); // "BB"
        b.extend_from_slice(b"AAABB");
        let pkg = Pkg::parse(b).unwrap();
        assert_eq!(pkg.read("one"), Some(&b"AAA"[..]));
        assert_eq!(pkg.read("two"), Some(&b"BB"[..]));
    }
}
