// Módulo `pkg`: lê o formato de arquivo do Wallpaper Engine (scene.pkg, "PKGV").
// É um arquivo-diretório: header + lista de entradas + blob de dados.
//
// Layout (little-endian):
//   [u32 len][bytes]            versão (sized string, começa com "PKGV")
//   [u32 filesCount]
//   filesCount x:
//     [u32 len][bytes]          nome do arquivo (sized string)
//     [u32 offset]              deslocamento do dado, relativo ao base
//     [u32 length]              tamanho do dado
//   <base_offset = aqui>        início do blob; dado i em [base+offset, +length]

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

// Leitor sequencial simples sobre um &[u8] (cursor + helpers little-endian).
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

    // "sized string": um u32 com o tamanho, seguido dos bytes.
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

    // Parse PURO (bytes -> índice). Não toca em disco -> testável.
    pub fn parse(data: Vec<u8>) -> Result<Pkg, String> {
        let mut c = Cursor {
            data: &data,
            pos: 0,
        };

        let version = c.sized_string().ok_or("header ausente")?;
        if !version.starts_with("PKGV") {
            return Err(format!("header inesperado: {version:?} (esperava PKGV*)"));
        }

        let count = c.u32().ok_or("contagem de arquivos ausente")?;
        let mut entries = Vec::with_capacity(count as usize);
        for i in 0..count {
            let name = c
                .sized_string()
                .ok_or(format!("nome do arquivo {i} ausente"))?;
            let offset = c.u32().ok_or(format!("offset do arquivo {i} ausente"))?;
            let length = c.u32().ok_or(format!("length do arquivo {i} ausente"))?;
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

    // Nomes dos arquivos contidos no pacote.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|e| e.name.as_str())
    }

    // Bytes de um arquivo pelo nome (None se não existe ou está fora dos limites).
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

    // Monta um .pkg mínimo em memória com um arquivo "a.txt" = "hello".
    fn build_minimal_pkg() -> Vec<u8> {
        let mut b = Vec::new();
        let sized = |b: &mut Vec<u8>, s: &str| {
            b.extend_from_slice(&(s.len() as u32).to_le_bytes());
            b.extend_from_slice(s.as_bytes());
        };
        sized(&mut b, "PKGV0001"); // versão
        b.extend_from_slice(&1u32.to_le_bytes()); // 1 arquivo
        sized(&mut b, "a.txt"); // nome
        b.extend_from_slice(&0u32.to_le_bytes()); // offset 0
        b.extend_from_slice(&5u32.to_le_bytes()); // length 5
        b.extend_from_slice(b"hello"); // blob de dados
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
        // "XXXX" não começa com PKGV.
        let mut b = Vec::new();
        b.extend_from_slice(&4u32.to_le_bytes());
        b.extend_from_slice(b"XXXX");
        assert!(Pkg::parse(b).is_err());
    }

    #[test]
    fn two_files_with_offsets() {
        // Verifica que offsets relativos ao base apontam pro pedaço certo do blob.
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
