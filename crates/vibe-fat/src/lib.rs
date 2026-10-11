//! FAT12/16 ファイルシステム
//!
//! ブロックデバイス（512 バイト単位で読み書きできるもの）の上で動く。FAT 1 つ分はマウント時に
//! メモリへ読み込んでおく。ファイル名は 8.3 形式（大文字小文字は区別しない）。長いファイル名の
//! エントリ（LFN）は読み飛ばす。
//!
//! パスは「/」区切りの名前の並び（`.` や `..` の解決は呼び出し側で済ませておく）。
//! 書き込み（作成・書き込み・切り詰め・削除・ディレクトリ作成）は `write.rs`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

pub const SECTOR_SIZE: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoError;

/// 512 バイト単位で読み書きできる装置
pub trait BlockDevice {
    /// 全体のセクタ数
    fn block_count(&self) -> u32;
    /// `lba` から `buf.len() / 512` セクタ読む
    fn read_blocks(&self, lba: u32, buf: &mut [u8]) -> core::result::Result<(), IoError>;
    /// `lba` から `buf.len() / 512` セクタ書く
    fn write_blocks(&mut self, lba: u32, buf: &[u8]) -> core::result::Result<(), IoError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsError {
    Io,
    /// FAT として読めない（壊れている・対応していない形式）
    BadFormat(&'static str),
    NotFound,
    NotADirectory,
    IsADirectory,
    /// 空きが無い（ディスク・ルートディレクトリ）
    NoSpace,
    AlreadyExists,
    /// 8.3 形式にできない名前
    InvalidName,
    /// 空でないディレクトリは消せない
    NotEmpty,
}

impl From<IoError> for FsError {
    fn from(_: IoError) -> Self {
        FsError::Io
    }
}

impl fmt::Display for FsError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            FsError::Io => write!(f, "I/O error"),
            FsError::BadFormat(why) => write!(f, "bad file system: {why}"),
            FsError::NotFound => write!(f, "not found"),
            FsError::NotADirectory => write!(f, "not a directory"),
            FsError::IsADirectory => write!(f, "is a directory"),
            FsError::NoSpace => write!(f, "no space left"),
            FsError::AlreadyExists => write!(f, "already exists"),
            FsError::InvalidName => write!(f, "invalid name (use 8.3 names)"),
            FsError::NotEmpty => write!(f, "directory not empty"),
        }
    }
}

pub type Result<T> = core::result::Result<T, FsError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FatType {
    Fat12,
    Fat16,
}

const ATTR_READ_ONLY: u8 = 0x01;
const ATTR_HIDDEN: u8 = 0x02;
const ATTR_SYSTEM: u8 = 0x04;
const ATTR_VOLUME_ID: u8 = 0x08;
const ATTR_DIRECTORY: u8 = 0x10;
const ATTR_LFN: u8 = ATTR_READ_ONLY | ATTR_HIDDEN | ATTR_SYSTEM | ATTR_VOLUME_ID;
const DIR_ENTRY_SIZE: usize = 32;

/// ディレクトリの場所。ルートは固定領域、それ以外はクラスタの連鎖
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Root,
    Cluster(u32),
}

/// ディレクトリエントリがディスク上のどこにあるか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryLoc {
    pub lba: u32,
    pub offset: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    /// "NAME.EXT" 形式（大文字）
    pub name: String,
    pub attr: u8,
    pub size: u32,
    pub first_cluster: u32,
    /// ディスク上の位置（ルートディレクトリそのものを表すエントリは None）
    pub loc: Option<EntryLoc>,
}

impl DirEntry {
    pub fn is_dir(&self) -> bool {
        self.attr & ATTR_DIRECTORY != 0
    }

    /// このエントリがディレクトリなら、その場所
    pub fn as_dir(&self) -> Result<Dir> {
        if !self.is_dir() {
            return Err(FsError::NotADirectory);
        }
        Ok(if self.first_cluster == 0 { Dir::Root } else { Dir::Cluster(self.first_cluster) })
    }
}

#[derive(Debug, Clone, Copy)]
struct Layout {
    sectors_per_cluster: u32,
    fat_start: u32,
    fat_sectors: u32,
    num_fats: u32,
    root_start: u32,
    root_entries: u32,
    data_start: u32,
    clusters: u32,
}

pub struct FatFs<D: BlockDevice> {
    dev: D,
    kind: FatType,
    layout: Layout,
    fat: Vec<u8>,
}

fn le16(b: &[u8], off: usize) -> u32 {
    u16::from_le_bytes([b[off], b[off + 1]]) as u32
}

fn le32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

impl<D: BlockDevice> FatFs<D> {
    pub fn mount(dev: D) -> Result<Self> {
        let mut boot = [0u8; SECTOR_SIZE];
        dev.read_blocks(0, &mut boot)?;
        if boot[510] != 0x55 || boot[511] != 0xAA {
            return Err(FsError::BadFormat("no boot signature"));
        }
        if le16(&boot, 11) as usize != SECTOR_SIZE {
            return Err(FsError::BadFormat("sector size is not 512"));
        }
        let spc = boot[13] as u32;
        let reserved = le16(&boot, 14);
        let num_fats = boot[16] as u32;
        let root_entries = le16(&boot, 17);
        let total = match le16(&boot, 19) {
            0 => le32(&boot, 32),
            n => n,
        };
        let fat_sectors = le16(&boot, 22);
        if spc == 0 || !spc.is_power_of_two() || reserved == 0 || num_fats == 0 || fat_sectors == 0 {
            return Err(FsError::BadFormat("invalid BPB"));
        }
        let root_start = reserved + num_fats * fat_sectors;
        let root_sectors = (root_entries * DIR_ENTRY_SIZE as u32).div_ceil(SECTOR_SIZE as u32);
        let data_start = root_start + root_sectors;
        if total <= data_start || total > dev.block_count() {
            return Err(FsError::BadFormat("sizes do not fit the device"));
        }
        let clusters = (total - data_start) / spc;
        let kind = if clusters < 4085 {
            FatType::Fat12
        } else if clusters < 65525 {
            FatType::Fat16
        } else {
            return Err(FsError::BadFormat("FAT32 is not supported"));
        };
        let mut fat = vec![0u8; fat_sectors as usize * SECTOR_SIZE];
        dev.read_blocks(reserved, &mut fat)?;
        let layout = Layout {
            sectors_per_cluster: spc,
            fat_start: reserved,
            fat_sectors,
            num_fats,
            root_start,
            root_entries,
            data_start,
            clusters,
        };
        Ok(FatFs { dev, kind, layout, fat })
    }

    pub fn fat_type(&self) -> FatType {
        self.kind
    }

    pub fn cluster_bytes(&self) -> usize {
        self.layout.sectors_per_cluster as usize * SECTOR_SIZE
    }

    /// 空きクラスタの数
    pub fn free_clusters(&self) -> u32 {
        (2..self.layout.clusters + 2).filter(|&c| self.fat_entry(c) == 0).count() as u32
    }

    pub fn total_clusters(&self) -> u32 {
        self.layout.clusters
    }

    fn fat_entry(&self, cluster: u32) -> u32 {
        let c = cluster as usize;
        match self.kind {
            FatType::Fat12 => {
                let off = c + c / 2;
                let v = le16(&self.fat, off);
                if c % 2 == 0 { v & 0xFFF } else { v >> 4 }
            }
            FatType::Fat16 => le16(&self.fat, c * 2),
        }
    }

    fn is_end_of_chain(&self, v: u32) -> bool {
        match self.kind {
            FatType::Fat12 => v >= 0xFF8,
            FatType::Fat16 => v >= 0xFFF8,
        }
    }

    fn valid_cluster(&self, c: u32) -> bool {
        c >= 2 && c < self.layout.clusters + 2
    }

    /// クラスタの連鎖をたどる（壊れた FAT で無限ループしないよう、クラスタ数で打ち切る）
    fn chain(&self, first: u32) -> Result<Vec<u32>> {
        let mut out = Vec::new();
        let mut c = first;
        while !self.is_end_of_chain(c) {
            if !self.valid_cluster(c) || out.len() > self.layout.clusters as usize {
                return Err(FsError::BadFormat("broken cluster chain"));
            }
            out.push(c);
            c = self.fat_entry(c);
        }
        Ok(out)
    }

    fn cluster_lba(&self, c: u32) -> u32 {
        self.layout.data_start + (c - 2) * self.layout.sectors_per_cluster
    }

    fn read_cluster(&self, c: u32, buf: &mut [u8]) -> Result<()> {
        self.dev.read_blocks(self.cluster_lba(c), buf)?;
        Ok(())
    }

    /// ディレクトリの全スロット（32 バイトずつ）とその位置
    fn dir_slots(&self, dir: Dir) -> Result<Vec<([u8; DIR_ENTRY_SIZE], EntryLoc)>> {
        let mut out = Vec::new();
        let push_sectors = |out: &mut Vec<_>, first_lba: u32, buf: &[u8]| {
            for (i, e) in buf.chunks_exact(DIR_ENTRY_SIZE).enumerate() {
                let byte = i * DIR_ENTRY_SIZE;
                let loc = EntryLoc { lba: first_lba + (byte / SECTOR_SIZE) as u32, offset: (byte % SECTOR_SIZE) as u16 };
                out.push((e.try_into().unwrap(), loc));
            }
        };
        match dir {
            Dir::Root => {
                let sectors = (self.layout.root_entries as usize * DIR_ENTRY_SIZE).div_ceil(SECTOR_SIZE);
                let mut buf = vec![0u8; sectors * SECTOR_SIZE];
                self.dev.read_blocks(self.layout.root_start, &mut buf)?;
                buf.truncate(self.layout.root_entries as usize * DIR_ENTRY_SIZE);
                push_sectors(&mut out, self.layout.root_start, &buf);
            }
            Dir::Cluster(first) => {
                let mut buf = vec![0u8; self.cluster_bytes()];
                for c in self.chain(first)? {
                    self.read_cluster(c, &mut buf)?;
                    push_sectors(&mut out, self.cluster_lba(c), &buf);
                }
            }
        }
        Ok(out)
    }

    /// ディレクトリの中身（"." と ".." は含まない）
    pub fn read_dir(&self, dir: Dir) -> Result<Vec<DirEntry>> {
        let mut out = Vec::new();
        for (e, loc) in self.dir_slots(dir)? {
            match e[0] {
                0x00 => break,    // 以降は未使用
                0xE5 => continue, // 削除済み
                _ => {}
            }
            let attr = e[11];
            if attr == ATTR_LFN || attr & ATTR_VOLUME_ID != 0 {
                continue;
            }
            let name = format_name(&e[0..11]);
            if name == "." || name == ".." {
                continue;
            }
            out.push(DirEntry { name, attr, size: le32(&e, 28), first_cluster: le16(&e, 26), loc: Some(loc) });
        }
        Ok(out)
    }

    /// パス（名前の並び）を探す。空ならルートを表すエントリを返す
    pub fn lookup<'a>(&self, path: impl IntoIterator<Item = &'a str>) -> Result<DirEntry> {
        let mut cur = DirEntry { name: String::from("/"), attr: ATTR_DIRECTORY, size: 0, first_cluster: 0, loc: None };
        for part in path {
            let dir = cur.as_dir()?;
            cur = self
                .read_dir(dir)?
                .into_iter()
                .find(|e| e.name.eq_ignore_ascii_case(part))
                .ok_or(FsError::NotFound)?;
        }
        Ok(cur)
    }

    /// ファイルの `offset` から読めるだけ読み、読んだバイト数を返す（終わりなら 0）
    pub fn read(&self, file: &DirEntry, offset: u32, buf: &mut [u8]) -> Result<usize> {
        if file.is_dir() {
            return Err(FsError::IsADirectory);
        }
        if offset >= file.size || buf.is_empty() {
            return Ok(0);
        }
        let len = buf.len().min((file.size - offset) as usize);
        let cb = self.cluster_bytes();
        let chain = self.chain(file.first_cluster)?;
        let mut cluster_buf = vec![0u8; cb];
        let mut done = 0;
        while done < len {
            let pos = offset as usize + done;
            let idx = pos / cb;
            let in_cluster = pos % cb;
            let c = *chain.get(idx).ok_or(FsError::BadFormat("file is shorter than its size"))?;
            self.read_cluster(c, &mut cluster_buf)?;
            let n = (cb - in_cluster).min(len - done);
            buf[done..done + n].copy_from_slice(&cluster_buf[in_cluster..in_cluster + n]);
            done += n;
        }
        Ok(len)
    }

    /// ファイル全体を読む
    pub fn read_to_vec(&self, file: &DirEntry) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; file.size as usize];
        let n = self.read(file, 0, &mut buf)?;
        buf.truncate(n);
        Ok(buf)
    }

    pub fn device(&self) -> &D {
        &self.dev
    }

    pub fn device_mut(&mut self) -> &mut D {
        &mut self.dev
    }
}

/// ディレクトリエントリの 11 バイトの名前を "NAME.EXT" にする
fn format_name(raw: &[u8]) -> String {
    let mut base: Vec<u8> = raw[0..8].to_vec();
    if base[0] == 0x05 {
        base[0] = 0xE5;
    }
    let trim = |s: &[u8]| -> String {
        let end = s.iter().rposition(|&b| b != b' ').map_or(0, |p| p + 1);
        s[..end].iter().map(|&b| if b.is_ascii() { b as char } else { '?' }).collect()
    };
    let mut name = trim(&base);
    let ext = trim(&raw[8..11]);
    if !ext.is_empty() {
        name.push('.');
        name.push_str(&ext);
    }
    name
}

mod write;

#[cfg(test)]
mod tests;
