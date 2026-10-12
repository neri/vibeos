//! FAT の書き込み
//!
//! 変更するたびに FAT 全体（すべての写し）をデバイスへ書き戻す。RAM ディスクの上では安く、
//! 実際のディスクへの書き戻しは下の層（変更したセクタだけを書く）に任せる。

use alloc::vec;

use super::*;

impl<D: BlockDevice> FatFs<D> {
    fn end_of_chain(&self) -> u32 {
        match self.kind {
            FatType::Fat12 => 0xFFF,
            FatType::Fat16 => 0xFFFF,
        }
    }

    fn set_fat_entry(&mut self, cluster: u32, value: u32) {
        let c = cluster as usize;
        match self.kind {
            FatType::Fat12 => {
                let off = c + c / 2;
                let old = le16(&self.fat, off);
                let v = if c % 2 == 0 { (old & 0xF000) | (value & 0xFFF) } else { (old & 0x000F) | ((value & 0xFFF) << 4) };
                self.fat[off..off + 2].copy_from_slice(&(v as u16).to_le_bytes());
            }
            FatType::Fat16 => self.fat[c * 2..c * 2 + 2].copy_from_slice(&(value as u16).to_le_bytes()),
        }
    }

    /// メモリ上の FAT をすべての写しに書き戻す
    fn flush_fat(&mut self) -> Result<()> {
        for i in 0..self.layout.num_fats {
            let lba = self.layout.fat_start + i * self.layout.fat_sectors;
            self.dev.write_blocks(lba, &self.fat)?;
        }
        Ok(())
    }

    /// 空きクラスタを 1 つ取り、終端の印を付けて中身を 0 にする
    fn alloc_cluster(&mut self) -> Result<u32> {
        let c = (2..self.layout.clusters + 2).find(|&c| self.fat_entry(c) == 0).ok_or(FsError::NoSpace)?;
        let eoc = self.end_of_chain();
        self.set_fat_entry(c, eoc);
        let zero = vec![0u8; self.cluster_bytes()];
        self.dev.write_blocks(self.cluster_lba(c), &zero)?;
        Ok(c)
    }

    fn free_chain(&mut self, first: u32) -> Result<()> {
        if first == 0 {
            return Ok(());
        }
        for c in self.chain(first)? {
            self.set_fat_entry(c, 0);
        }
        Ok(())
    }

    fn read_entry(&self, loc: EntryLoc) -> Result<[u8; DIR_ENTRY_SIZE]> {
        let mut sector = [0u8; SECTOR_SIZE];
        self.dev.read_blocks(loc.lba, &mut sector)?;
        let o = loc.offset as usize;
        Ok(sector[o..o + DIR_ENTRY_SIZE].try_into().unwrap())
    }

    fn write_entry(&mut self, loc: EntryLoc, raw: &[u8; DIR_ENTRY_SIZE]) -> Result<()> {
        let mut sector = [0u8; SECTOR_SIZE];
        self.dev.read_blocks(loc.lba, &mut sector)?;
        let o = loc.offset as usize;
        sector[o..o + DIR_ENTRY_SIZE].copy_from_slice(raw);
        self.dev.write_blocks(loc.lba, &sector)?;
        Ok(())
    }

    /// エントリの大きさ・先頭クラスタ・更新日時を書き戻す
    fn update_entry(&mut self, e: &mut DirEntry) -> Result<()> {
        let loc = e.loc.ok_or(FsError::InvalidName)?;
        let (date, time) = (self.clock)();
        e.modified = (date, time);
        let mut raw = self.read_entry(loc)?;
        raw[18..20].copy_from_slice(&date.to_le_bytes()); // 最終アクセス日
        raw[20..22].copy_from_slice(&[0, 0]);
        raw[22..24].copy_from_slice(&time.to_le_bytes());
        raw[24..26].copy_from_slice(&date.to_le_bytes());
        raw[26..28].copy_from_slice(&(e.first_cluster as u16).to_le_bytes());
        raw[28..32].copy_from_slice(&e.size.to_le_bytes());
        self.write_entry(loc, &raw)
    }

    /// ディレクトリの空いているスロット。サブディレクトリが一杯ならクラスタを足す
    fn free_slot(&mut self, dir: Dir) -> Result<EntryLoc> {
        if let Some((_, loc)) = self.dir_slots(dir)?.into_iter().find(|(e, _)| e[0] == 0x00 || e[0] == 0xE5) {
            return Ok(loc);
        }
        let Dir::Cluster(first) = dir else {
            return Err(FsError::NoSpace); // ルートディレクトリの大きさは決まっている
        };
        let last = *self.chain(first)?.last().unwrap();
        let c = self.alloc_cluster()?;
        self.set_fat_entry(last, c);
        self.flush_fat()?;
        Ok(EntryLoc { lba: self.cluster_lba(c), offset: 0 })
    }

    fn new_entry(&mut self, dir: Dir, name: &str, attr: u8, first_cluster: u32) -> Result<DirEntry> {
        let raw_name = name_to_raw(name)?;
        let display = format_name(&raw_name);
        if self.read_dir(dir)?.iter().any(|e| e.name == display) {
            return Err(FsError::AlreadyExists);
        }
        let loc = self.free_slot(dir)?;
        let now = (self.clock)();
        self.write_entry(loc, &raw_entry(&raw_name, attr, first_cluster, 0, now))?;
        Ok(DirEntry { name: display, attr, size: 0, first_cluster, loc: Some(loc), modified: now })
    }

    /// 空のファイルを作る
    pub fn create(&mut self, dir: Dir, name: &str) -> Result<DirEntry> {
        self.new_entry(dir, name, 0, 0)
    }

    /// ディレクトリを作る（"." と ".." を入れる）
    pub fn mkdir(&mut self, dir: Dir, name: &str) -> Result<DirEntry> {
        name_to_raw(name)?;
        let c = self.alloc_cluster()?;
        let parent = match dir {
            Dir::Root => 0,
            Dir::Cluster(p) => p,
        };
        let mut first = vec![0u8; self.cluster_bytes()];
        let now = (self.clock)();
        first[0..32].copy_from_slice(&raw_entry(b".          ", ATTR_DIRECTORY, c, 0, now));
        first[32..64].copy_from_slice(&raw_entry(b"..         ", ATTR_DIRECTORY, parent, 0, now));
        self.dev.write_blocks(self.cluster_lba(c), &first)?;
        match self.new_entry(dir, name, ATTR_DIRECTORY, c) {
            Ok(e) => {
                self.flush_fat()?;
                Ok(e)
            }
            Err(err) => {
                self.set_fat_entry(c, 0);
                Err(err)
            }
        }
    }

    /// `offset` から書く（ファイルの終わりより後ろなら間を 0 で埋める）。書いたバイト数を返す
    pub fn write(&mut self, file: &mut DirEntry, offset: u32, data: &[u8]) -> Result<usize> {
        if file.is_dir() {
            return Err(FsError::IsADirectory);
        }
        if offset > file.size {
            let gap = vec![0u8; (offset - file.size) as usize];
            let size = file.size;
            self.write(file, size, &gap)?;
        }
        if data.is_empty() {
            return Ok(0);
        }
        let end = offset.checked_add(data.len() as u32).ok_or(FsError::NoSpace)?;
        let cb = self.cluster_bytes();
        let mut chain = if file.first_cluster == 0 { Vec::new() } else { self.chain(file.first_cluster)? };
        let needed = (end as usize).div_ceil(cb);
        let old_len = chain.len();
        let grew = chain.len() < needed;
        while chain.len() < needed {
            let c = match self.alloc_cluster() {
                Ok(c) => c,
                Err(e) => {
                    // 空きが足りない: この書き込みで取ったクラスタを返して、元の状態に戻す
                    for &c in &chain[old_len..] {
                        self.set_fat_entry(c, 0);
                    }
                    if old_len == 0 {
                        file.first_cluster = 0;
                    } else {
                        let eoc = self.end_of_chain();
                        self.set_fat_entry(chain[old_len - 1], eoc);
                    }
                    return Err(e);
                }
            };
            match chain.last() {
                Some(&last) => self.set_fat_entry(last, c),
                None => file.first_cluster = c,
            }
            chain.push(c);
        }
        if grew {
            self.flush_fat()?;
        }
        let mut buf = vec![0u8; cb];
        let mut done = 0;
        while done < data.len() {
            let pos = offset as usize + done;
            let (idx, within) = (pos / cb, pos % cb);
            let n = (cb - within).min(data.len() - done);
            let c = chain[idx];
            if n < cb {
                self.read_cluster(c, &mut buf)?;
            }
            buf[within..within + n].copy_from_slice(&data[done..done + n]);
            self.dev.write_blocks(self.cluster_lba(c), &buf)?;
            done += n;
        }
        file.size = file.size.max(end);
        self.update_entry(file)?;
        Ok(data.len())
    }

    /// ファイルを `size` バイトに縮める（大きくはしない）
    pub fn truncate(&mut self, file: &mut DirEntry, size: u32) -> Result<()> {
        if file.is_dir() {
            return Err(FsError::IsADirectory);
        }
        if size >= file.size {
            return Ok(());
        }
        let keep = (size as usize).div_ceil(self.cluster_bytes());
        if file.first_cluster != 0 {
            let chain = self.chain(file.first_cluster)?;
            if keep == 0 {
                self.free_chain(file.first_cluster)?;
                file.first_cluster = 0;
            } else if keep < chain.len() {
                let eoc = self.end_of_chain();
                self.set_fat_entry(chain[keep - 1], eoc);
                for &c in &chain[keep..] {
                    self.set_fat_entry(c, 0);
                }
            }
            self.flush_fat()?;
        }
        file.size = size;
        self.update_entry(file)
    }

    /// ファイルか空のディレクトリを消す
    pub fn remove(&mut self, entry: &DirEntry) -> Result<()> {
        let loc = entry.loc.ok_or(FsError::InvalidName)?; // ルートは消せない
        if entry.is_dir() && !self.read_dir(entry.as_dir()?)?.is_empty() {
            return Err(FsError::NotEmpty);
        }
        self.free_chain(entry.first_cluster)?;
        self.flush_fat()?;
        let mut raw = self.read_entry(loc)?;
        raw[0] = 0xE5;
        self.write_entry(loc, &raw)
    }

    /// 名前を変える・別のディレクトリ `to_dir` へ移す。ディレクトリを移すときは中の ".." も直す。
    /// ディレクトリを自分の中（子孫）へは移せない（`InvalidName`）。新しいエントリを返す。
    /// 開いているファイルの表のエントリは直さないので、開いたまま移したファイルに書かないこと
    pub fn rename(&mut self, entry: &DirEntry, to_dir: Dir, new_name: &str) -> Result<DirEntry> {
        let loc = entry.loc.ok_or(FsError::InvalidName)?; // ルートは動かせない
        let raw_name = name_to_raw(new_name)?;
        let display = format_name(&raw_name);
        let slots = self.dir_slots(to_dir)?;
        let same_dir = slots.iter().any(|(_, l)| *l == loc);
        if let Some(e) = self.read_dir(to_dir)?.iter().find(|e| e.name == display) {
            if e.loc == Some(loc) {
                return Ok(entry.clone()); // 同じ名前
            }
            return Err(FsError::AlreadyExists);
        }
        if entry.is_dir() && !same_dir {
            // 移す先から ".." をたどってルートまでに自分があれば、自分の中へ移そうとしている
            let mut d = to_dir;
            while let Dir::Cluster(c) = d {
                if c == entry.first_cluster {
                    return Err(FsError::InvalidName);
                }
                d = self.parent_of(c)?;
            }
        }
        let mut raw = self.read_entry(loc)?;
        raw[0..11].copy_from_slice(&raw_name);
        if same_dir {
            self.write_entry(loc, &raw)?;
            return Ok(DirEntry { name: display, ..entry.clone() });
        }
        // 先に新しい場所へ書いてから元を消す（途中で失敗しても消えない）
        let new_loc = self.free_slot(to_dir)?;
        self.write_entry(new_loc, &raw)?;
        let mut old = self.read_entry(loc)?;
        old[0] = 0xE5;
        self.write_entry(loc, &old)?;
        if entry.is_dir() {
            let parent = match to_dir {
                Dir::Root => 0,
                Dir::Cluster(p) => p,
            };
            let dotdot = EntryLoc { lba: self.cluster_lba(entry.first_cluster), offset: DIR_ENTRY_SIZE as u16 };
            let mut dd = self.read_entry(dotdot)?;
            dd[26..28].copy_from_slice(&(parent as u16).to_le_bytes());
            self.write_entry(dotdot, &dd)?;
        }
        Ok(DirEntry { name: display, loc: Some(new_loc), ..entry.clone() })
    }

    /// サブディレクトリ（先頭クラスタ `c`）の親（".." の先）
    fn parent_of(&self, c: u32) -> Result<Dir> {
        let dd = self.read_entry(EntryLoc { lba: self.cluster_lba(c), offset: DIR_ENTRY_SIZE as u16 })?;
        let p = u16::from_le_bytes([dd[26], dd[27]]) as u32;
        Ok(if p == 0 { Dir::Root } else { Dir::Cluster(p) })
    }

    /// 空いている容量（バイト）
    pub fn free_bytes(&self) -> u64 {
        self.free_clusters() as u64 * self.cluster_bytes() as u64
    }
}

/// "NAME.EXT" を 11 バイトの形式にする（大文字にする。使えない文字・長すぎる名前はエラー）
pub(crate) fn name_to_raw(name: &str) -> Result<[u8; 11]> {
    let (base, ext) = match name.rfind('.') {
        Some(0) | None => (name, ""),
        Some(i) => (&name[..i], &name[i + 1..]),
    };
    let ok = |s: &str, max: usize| {
        !s.is_empty() && s.len() <= max && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'()-@^_`{}~".contains(&b))
    };
    if !ok(base, 8) || !(ext.is_empty() || ok(ext, 3)) {
        return Err(FsError::InvalidName);
    }
    let mut raw = [b' '; 11];
    for (i, b) in base.bytes().enumerate() {
        raw[i] = b.to_ascii_uppercase();
    }
    for (i, b) in ext.bytes().enumerate() {
        raw[8 + i] = b.to_ascii_uppercase();
    }
    Ok(raw)
}

fn raw_entry(name: &[u8; 11], attr: u8, cluster: u32, size: u32, (date, time): (u16, u16)) -> [u8; DIR_ENTRY_SIZE] {
    let mut e = [0u8; DIR_ENTRY_SIZE];
    e[0..11].copy_from_slice(name);
    e[11] = attr;
    e[14..16].copy_from_slice(&time.to_le_bytes()); // 作成時刻
    e[16..18].copy_from_slice(&date.to_le_bytes()); // 作成日
    e[18..20].copy_from_slice(&date.to_le_bytes()); // 最終アクセス日
    e[22..24].copy_from_slice(&time.to_le_bytes()); // 更新時刻
    e[24..26].copy_from_slice(&date.to_le_bytes()); // 更新日
    e[26..28].copy_from_slice(&(cluster as u16).to_le_bytes());
    e[28..32].copy_from_slice(&size.to_le_bytes());
    e
}
