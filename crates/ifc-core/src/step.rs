//! Leitor STEP (ISO 10303-21) econômico em memória para arquivos IFC muito grandes.
//!
//! O arquivo é mapeado via mmap. A passagem 1 cria apenas um índice compacto
//! (Id -> posição no arquivo, tipo) para as entidades de "estrutura"; as
//! entidades de geometria em massa (pontos, faces, malhas …) são
//! ignoradas. As entidades só são analisadas a partir do arquivo quando acessadas.

use memchr::memchr;
use memmap2::Mmap;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::sync::atomic::{AtomicBool, Ordering};

/// Geometria em massa que NÃO é indexada na passagem 1.
pub const SKIP_TYPES: &[&str] = &[
    "IFCCARTESIANPOINT", "IFCCARTESIANPOINTLIST3D", "IFCCARTESIANPOINTLIST2D",
    "IFCPOLYLOOP", "IFCFACE", "IFCFACEOUTERBOUND", "IFCFACEBOUND", "IFCCLOSEDSHELL",
    "IFCOPENSHELL", "IFCFACETEDBREP", "IFCTRIANGULATEDFACESET", "IFCPOLYGONALFACESET",
    "IFCINDEXEDPOLYGONALFACE", "IFCINDEXEDPOLYGONALFACEWITHVOIDS", "IFCADVANCEDFACE",
    "IFCADVANCEDBREP", "IFCEDGECURVE", "IFCORIENTEDEDGE", "IFCVERTEXPOINT", "IFCEDGELOOP",
    "IFCBSPLINESURFACEWITHKNOTS", "IFCRATIONALBSPLINESURFACEWITHKNOTS",
    "IFCBSPLINECURVEWITHKNOTS", "IFCRATIONALBSPLINECURVEWITHKNOTS",
    "IFCSTYLEDITEM", "IFCPRESENTATIONSTYLEASSIGNMENT", "IFCSURFACESTYLE",
    "IFCSURFACESTYLERENDERING", "IFCSURFACESTYLESHADING", "IFCCOLOURRGB",
    "IFCSHAPEASPECT", "IFCTRIANGULATEDIRREGULARNETWORK",
    "IFCPOLYGONALBOUNDEDHALFSPACE", "IFCPLANE",
];

#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    Null,
    Derived,
    Ref(u32),
    Int(i64),
    Real(f64),
    Str(String),
    Enum(String),
    List(Vec<Arg>),
    Typed(String, Box<Arg>),
}

impl Arg {
    pub fn as_ref(&self) -> Option<u32> {
        if let Arg::Ref(r) = self { Some(*r) } else { None }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Arg::Str(s) => Some(s),
            Arg::Typed(_, b) => b.as_str(),
            _ => None,
        }
    }
    pub fn as_enum(&self) -> Option<&str> {
        if let Arg::Enum(s) = self { Some(s) } else { None }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Arg::Real(v) => Some(*v),
            Arg::Int(v) => Some(*v as f64),
            Arg::Typed(_, b) => b.as_f64(),
            _ => None,
        }
    }
    pub fn as_list(&self) -> &[Arg] {
        if let Arg::List(l) = self { l } else { &[] }
    }
    pub fn refs(&self) -> Vec<u32> {
        self.as_list().iter().filter_map(|a| a.as_ref()).collect()
    }
}

#[derive(Debug, Clone)]
pub struct Entity {
    pub id: u32,
    pub ty: String,
    pub args: Vec<Arg>,
}

impl Entity {
    pub fn arg(&self, i: usize) -> &Arg {
        self.args.get(i).unwrap_or(&Arg::Null)
    }
    pub fn str(&self, i: usize) -> Option<&str> { self.arg(i).as_str() }
    pub fn rf(&self, i: usize) -> Option<u32> { self.arg(i).as_ref() }
    pub fn f64(&self, i: usize) -> Option<f64> { self.arg(i).as_f64() }
}

pub type Progress<'a> = &'a (dyn Fn(u64, u64) + Sync);

/// Descompacta .ifczip (primeiro arquivo .ifc do pacote) e retorna o caminho do arquivo extraído.
pub fn unpack_ifczip(path: &str, progress: Progress, cancel: &AtomicBool) -> Result<String, StepError> {
    use std::io::{Read, Write};
    let f = File::open(path).map_err(StepError::Io)?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| StepError::Zip(e.to_string()))?;
    let idx = (0..z.len())
        .find(|&i| z.by_index(i).map(|e| e.name().to_lowercase().ends_with(".ifc")).unwrap_or(false))
        .ok_or_else(|| StepError::Zip("keine .ifc-Datei im Archiv".into()))?;
    let mut entry = z.by_index(idx).map_err(|e| StepError::Zip(e.to_string()))?;
    let total = entry.size();
    let dir = std::env::temp_dir().join("spk-ifc-viewer");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(StepError::Io)?;
    let name = std::path::Path::new(entry.name()).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or("modell.ifc".into());
    let out_path = dir.join(name);
    let mut out = std::io::BufWriter::new(File::create(&out_path).map_err(StepError::Io)?);
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) { return Err(StepError::Cancelled); }
        let n = entry.read(&mut buf).map_err(StepError::Io)?;
        if n == 0 { break; }
        out.write_all(&buf[..n]).map_err(StepError::Io)?;
        done += n as u64;
        progress(done, total.max(1));
    }
    out.flush().map_err(StepError::Io)?;
    Ok(out_path.to_string_lossy().to_string())
}

pub struct StepFile {
    mmap: Mmap,
    data_start: usize,
    pub schema: String,
    types: Vec<String>,
    type_ids: HashMap<String, u16>,
    // Índice primário (ordenado por Id)
    ids: Vec<u32>,
    offs: Vec<u64>,
    tys: Vec<u16>,
    // Índice secundário para entidades em massa carregadas depois (passagem 2)
    extra: HashMap<u32, (u64, u16)>,
    pub skipped_count: u64,
    pub all_skipped_loaded: bool,
}

#[derive(Debug)]
pub enum StepError {
    Io(std::io::Error),
    NotStep,
    Revit,
    Zip(String),
    Cancelled,
}

impl std::fmt::Display for StepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StepError::Io(e) => write!(f, "Datei konnte nicht gelesen werden: {e}"),
            StepError::NotStep => write!(f, "Keine IFC-Datei (STEP-Kopf fehlt). Bitte eine .ifc-Datei wählen – Revit-Dateien (.rvt) vorher als IFC exportieren."),
            StepError::Revit => write!(f, "Das ist eine Revit-Datei (.rvt). Revit-Dateien können in der Alpha-Version noch nicht direkt gelesen werden – bitte in Revit als IFC exportieren (Datei › Exportieren › IFC) und die .ifc-Datei öffnen."),
            StepError::Zip(e) => write!(f, "IFC-ZIP konnte nicht entpackt werden: {e}"),
            StepError::Cancelled => write!(f, "Abgebrochen"),
        }
    }
}

impl std::error::Error for StepError {}

const PROGRESS_STEP: usize = 32 << 20;

impl StepFile {
    /// Passagem 1: abre o arquivo e monta o índice de estrutura.
    pub fn open(path: &str, progress: Progress, cancel: &AtomicBool) -> Result<StepFile, StepError> {
        let file = File::open(path).map_err(StepError::Io)?;
        let mmap = unsafe { Mmap::map(&file) }.map_err(StepError::Io)?;
        #[cfg(unix)]
        {
            let _ = mmap.advise(memmap2::Advice::Sequential);
        }
        let buf: &[u8] = &mmap;
        if buf.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) {
            return Err(StepError::Revit);
        }
        if buf.len() < 12 {
            return Err(StepError::NotStep);
        }
        if !buf.starts_with(b"ISO-10303-21") && !buf[..buf.len().min(4096)].windows(12).any(|w| w == b"ISO-10303-21") {
            return Err(StepError::NotStep);
        }
        let head = &buf[..buf.len().min(1 << 16)];
        let schema = find_schema(head);
        let data_start = find_sub(buf, b"DATA;").map(|p| p + 5).ok_or(StepError::NotStep)?;

        let skip: HashSet<&[u8]> = SKIP_TYPES.iter().map(|s| s.as_bytes()).collect();
        let mut types: Vec<String> = Vec::new();
        let mut type_ids: HashMap<String, u16> = HashMap::new();
        let total = buf.len() as u64;
        let mut next_report = 0usize;
        let mut sorted = true;
        let mut last_id = 0u32;
        let mut skipped = 0u64;
        let mut ids = Vec::new();
        let mut offs = Vec::new();
        let mut tys = Vec::new();
        for_each_stmt(buf, data_start, |pos, id, ty| {
            if pos >= next_report {
                progress(pos as u64, total);
                next_report = pos + PROGRESS_STEP;
                if cancel.load(Ordering::Relaxed) {
                    return false;
                }
            }
            if skip.contains(ty) {
                skipped += 1;
                return true;
            }
            let t = intern(&mut types, &mut type_ids, ty);
            if id < last_id {
                sorted = false;
            }
            last_id = id;
            ids.push(id);
            offs.push(pos as u64);
            tys.push(t);
            true
        });
        if cancel.load(Ordering::Relaxed) {
            return Err(StepError::Cancelled);
        }
        if !sorted {
            let mut perm: Vec<usize> = (0..ids.len()).collect();
            perm.sort_unstable_by_key(|&i| ids[i]);
            ids = perm.iter().map(|&i| ids[i]).collect();
            offs = perm.iter().map(|&i| offs[i]).collect();
            tys = perm.iter().map(|&i| tys[i]).collect();
        }
        progress(total, total);
        #[cfg(unix)]
        {
            let _ = mmap.advise(memmap2::Advice::Random);
        }
        Ok(StepFile {
            mmap,
            data_start,
            schema,
            types,
            type_ids,
            ids,
            offs,
            tys,
            extra: HashMap::new(),
            skipped_count: skipped,
            all_skipped_loaded: false,
        })
    }

    pub fn len(&self) -> usize { self.ids.len() }
    pub fn file_size(&self) -> u64 { self.mmap.len() as u64 }

    /// Todos os Ids indexados de um tipo (maiúsculas, p. ex. "IFCPIPESEGMENT").
    pub fn ids_of_type(&self, ty: &str) -> Vec<u32> {
        match self.type_ids.get(ty) {
            Some(&t) => self.ids.iter().zip(&self.tys).filter(|(_, &x)| x == t).map(|(&i, _)| i).collect(),
            None => vec![],
        }
    }

    pub fn ids_of_types(&self, tys: &[&str]) -> Vec<(u32, String)> {
        let set: HashMap<u16, &str> = tys
            .iter()
            .filter_map(|t| self.type_ids.get(*t).map(|&i| (i, *t)))
            .collect();
        self.ids
            .iter()
            .zip(&self.tys)
            .filter_map(|(&i, t)| set.get(t).map(|s| (i, s.to_string())))
            .collect()
    }

    pub fn type_of(&self, id: u32) -> Option<&str> {
        if let Ok(k) = self.ids.binary_search(&id) {
            return Some(&self.types[self.tys[k] as usize]);
        }
        self.extra.get(&id).map(|(_, t)| self.types[*t as usize].as_str())
    }

    pub fn has(&self, id: u32) -> bool {
        self.ids.binary_search(&id).is_ok() || self.extra.contains_key(&id)
    }

    fn offset(&self, id: u32) -> Option<usize> {
        if let Ok(k) = self.ids.binary_search(&id) {
            return Some(self.offs[k] as usize);
        }
        self.extra.get(&id).map(|(o, _)| *o as usize)
    }

    /// Lê uma entidade (analisada sob demanda a partir do arquivo).
    pub fn get(&self, id: u32) -> Option<Entity> {
        let off = self.offset(id)?;
        parse_stmt_at(&self.mmap, off)
    }

    /// Passagem 2 (arquivos menores): indexa todas as entidades ignoradas.
    pub fn load_all_skipped(&mut self, progress: Progress, cancel: &AtomicBool) -> Result<usize, StepError> {
        if self.all_skipped_loaded { return Ok(0); }
        let skip: HashSet<&[u8]> = SKIP_TYPES.iter().map(|s| s.as_bytes()).collect();
        let total = self.mmap.len() as u64;
        let mut next_report = 0usize;
        let mut found: Vec<(u32, u64, Vec<u8>)> = Vec::new();
        let buf: &[u8] = &self.mmap;
        let mut tycache: HashMap<Vec<u8>, ()> = HashMap::new();
        for_each_stmt(buf, self.data_start, |pos, id, ty| {
            if pos >= next_report {
                progress(pos as u64, total);
                next_report = pos + PROGRESS_STEP;
                if cancel.load(Ordering::Relaxed) {
                    return false;
                }
            }
            if skip.contains(ty) {
                if !tycache.contains_key(ty) { tycache.insert(ty.to_vec(), ()); }
                found.push((id, pos as u64, ty.to_vec()));
            }
            true
        });
        if cancel.load(Ordering::Relaxed) {
            return Err(StepError::Cancelled);
        }
        let n = found.len();
        self.all_skipped_loaded = true;
        self.extra.reserve(n);
        for (id, pos, ty) in found {
            let t = intern(&mut self.types, &mut self.type_ids, &ty);
            self.extra.insert(id, (pos, t));
        }
        progress(total, total);
        Ok(n)
    }

    /// Passagem 2: indexa determinadas entidades ignoradas (p. ex. pontos).
    pub fn load_extra(&mut self, wanted: &HashSet<u32>, progress: Progress, cancel: &AtomicBool) -> Result<usize, StepError> {
        if wanted.is_empty() {
            return Ok(0);
        }
        let total = self.mmap.len() as u64;
        let mut next_report = 0usize;
        let mut found: Vec<(u32, u64, Vec<u8>)> = Vec::new();
        let buf: &[u8] = &self.mmap;
        for_each_stmt(buf, self.data_start, |pos, id, ty| {
            if pos >= next_report {
                progress(pos as u64, total);
                next_report = pos + PROGRESS_STEP;
                if cancel.load(Ordering::Relaxed) {
                    return false;
                }
            }
            if wanted.contains(&id) {
                found.push((id, pos as u64, ty.to_vec()));
            }
            true
        });
        if cancel.load(Ordering::Relaxed) {
            return Err(StepError::Cancelled);
        }
        let n = found.len();
        for (id, pos, ty) in found {
            let t = intern(&mut self.types, &mut self.type_ids, &ty);
            self.extra.insert(id, (pos, t));
        }
        progress(total, total);
        Ok(n)
    }
}

fn intern(types: &mut Vec<String>, type_ids: &mut HashMap<String, u16>, ty: &[u8]) -> u16 {
    let s = String::from_utf8_lossy(ty);
    if let Some(&i) = type_ids.get(s.as_ref()) {
        return i;
    }
    let i = types.len() as u16;
    types.push(s.to_string());
    type_ids.insert(s.to_string(), i);
    i
}

fn find_sub(h: &[u8], n: &[u8]) -> Option<usize> {
    memchr::memmem::find(h, n)
}

fn find_schema(head: &[u8]) -> String {
    if let Some(p) = find_sub(head, b"FILE_SCHEMA") {
        let rest = &head[p..head.len().min(p + 200)];
        if let Some(q) = memchr(b'\'', rest) {
            if let Some(e) = memchr(b'\'', &rest[q + 1..]) {
                return String::from_utf8_lossy(&rest[q + 1..q + 1 + e]).to_string();
            }
        }
    }
    "unbekannt".into()
}

/// Chama `f(pos, id, typname)` para cada instrução `#id=TYPE(...);`.
/// `f` retorna false para interromper.
fn for_each_stmt<F: FnMut(usize, u32, &[u8]) -> bool>(buf: &[u8], start: usize, mut f: F) {
    let n = buf.len();
    let mut pos = start;
    while pos < n {
        // próximo '#'
        let Some(h) = memchr(b'#', &buf[pos..]) else { break };
        let s = pos + h;
        // procura o fim da instrução (';' fora de strings)
        let end = stmt_end(buf, s);
        // id
        let mut i = s + 1;
        let mut id: u32 = 0;
        let mut digits = 0;
        while i < end && buf[i].is_ascii_digit() {
            id = id.wrapping_mul(10).wrapping_add((buf[i] - b'0') as u32);
            i += 1;
            digits += 1;
        }
        if digits > 0 {
            while i < end && (buf[i] == b' ' || buf[i] == b'\r' || buf[i] == b'\n' || buf[i] == b'\t') { i += 1; }
            if i < end && buf[i] == b'=' {
                i += 1;
                while i < end && (buf[i] == b' ' || buf[i] == b'\r' || buf[i] == b'\n' || buf[i] == b'\t') { i += 1; }
                let ts = i;
                while i < end && (buf[i].is_ascii_alphanumeric() || buf[i] == b'_') { i += 1; }
                if i > ts && !f(s, id, &buf[ts..i]) {
                    return;
                }
            } else if end >= n {
                break;
            }
        }
        if end >= n {
            break;
        }
        // ENDSEC alcançado?
        pos = end + 1;
    }
}

/// Posição do ';' final da instrução a partir de `s` (considerando strings).
#[inline]
fn stmt_end(buf: &[u8], s: usize) -> usize {
    let n = buf.len();
    let mut p = s;
    loop {
        let semi = match memchr(b';', &buf[p..]) { Some(x) => p + x, None => return n };
        // conta as aspas entre p e semi
        let seg = &buf[p..semi];
        let quotes = memchr::memchr_iter(b'\'', seg).count();
        if quotes % 2 == 0 {
            return semi;
        }
        // ímpar -> estamos dentro de uma string: pular até o ' de fechamento
        let mut q = p + seg.iter().rposition(|&c| c == b'\'').unwrap() + 1;
        loop {
            match memchr(b'\'', &buf[q..]) {
                None => return n,
                Some(x) => {
                    let at = q + x;
                    if at + 1 < n && buf[at + 1] == b'\'' {
                        q = at + 2; // '' escapado
                    } else {
                        q = at + 1;
                        break;
                    }
                }
            }
        }
        p = q;
    }
}

pub fn parse_stmt_at(buf: &[u8], off: usize) -> Option<Entity> {
    let end = stmt_end(buf, off);
    let s = &buf[off..end];
    let mut i = 1;
    let mut id = 0u32;
    while i < s.len() && s[i].is_ascii_digit() {
        id = id * 10 + (s[i] - b'0') as u32;
        i += 1;
    }
    while i < s.len() && s[i] != b'=' { i += 1; }
    i += 1;
    while i < s.len() && s[i].is_ascii_whitespace() { i += 1; }
    let ts = i;
    while i < s.len() && (s[i].is_ascii_alphanumeric() || s[i] == b'_') { i += 1; }
    let ty = String::from_utf8_lossy(&s[ts..i]).to_string();
    while i < s.len() && s[i] != b'(' { i += 1; }
    let mut p = Parser { s, i };
    let args = match p.list() { Arg::List(l) => l, _ => vec![] };
    Some(Entity { id, ty, args })
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl<'a> Parser<'a> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() { self.i += 1; }
    }
    fn list(&mut self) -> Arg {
        // espera '('
        self.ws();
        if self.i >= self.s.len() || self.s[self.i] != b'(' {
            return Arg::Null;
        }
        self.i += 1;
        let mut out = Vec::new();
        loop {
            self.ws();
            if self.i >= self.s.len() { break; }
            if self.s[self.i] == b')' { self.i += 1; break; }
            out.push(self.value());
            self.ws();
            if self.i < self.s.len() && self.s[self.i] == b',' { self.i += 1; }
        }
        Arg::List(out)
    }
    fn value(&mut self) -> Arg {
        self.ws();
        if self.i >= self.s.len() { return Arg::Null; }
        let c = self.s[self.i];
        match c {
            b'$' => { self.i += 1; Arg::Null }
            b'*' => { self.i += 1; Arg::Derived }
            b'#' => {
                self.i += 1;
                let mut v = 0u32;
                while self.i < self.s.len() && self.s[self.i].is_ascii_digit() {
                    v = v * 10 + (self.s[self.i] - b'0') as u32;
                    self.i += 1;
                }
                Arg::Ref(v)
            }
            b'\'' => Arg::Str(self.string()),
            b'.' => {
                self.i += 1;
                let st = self.i;
                while self.i < self.s.len() && self.s[self.i] != b'.' { self.i += 1; }
                let e = String::from_utf8_lossy(&self.s[st..self.i]).to_string();
                self.i += 1;
                Arg::Enum(e)
            }
            b'(' => self.list(),
            b'"' => {
                self.i += 1;
                while self.i < self.s.len() && self.s[self.i] != b'"' { self.i += 1; }
                self.i += 1;
                Arg::Null
            }
            c if c.is_ascii_alphabetic() => {
                let st = self.i;
                while self.i < self.s.len() && (self.s[self.i].is_ascii_alphanumeric() || self.s[self.i] == b'_') { self.i += 1; }
                let name = String::from_utf8_lossy(&self.s[st..self.i]).to_string();
                self.ws();
                if self.i < self.s.len() && self.s[self.i] == b'(' {
                    let inner = self.list();
                    let v = match inner {
                        Arg::List(mut l) if l.len() == 1 => l.pop().unwrap(),
                        other => other,
                    };
                    Arg::Typed(name, Box::new(v))
                } else {
                    Arg::Enum(name)
                }
            }
            _ => {
                let st = self.i;
                while self.i < self.s.len() && !matches!(self.s[self.i], b',' | b')') && !self.s[self.i].is_ascii_whitespace() {
                    self.i += 1;
                }
                let t = std::str::from_utf8(&self.s[st..self.i]).unwrap_or("");
                if t.contains('.') || t.contains('E') || t.contains('e') {
                    t.parse::<f64>().map(Arg::Real).unwrap_or(Arg::Null)
                } else {
                    t.parse::<i64>().map(Arg::Int).unwrap_or(Arg::Null)
                }
            }
        }
    }
    fn string(&mut self) -> String {
        self.i += 1;
        let mut raw = Vec::new();
        while self.i < self.s.len() {
            let c = self.s[self.i];
            if c == b'\'' {
                if self.i + 1 < self.s.len() && self.s[self.i + 1] == b'\'' {
                    raw.push(b'\'');
                    self.i += 2;
                    continue;
                }
                self.i += 1;
                break;
            }
            raw.push(c);
            self.i += 1;
        }
        decode_step_string(&raw)
    }
}

/// Codificação de strings STEP (\X2\..\X0\, \X\hh, \S\c, \X4\) para UTF-8.
pub fn decode_step_string(raw: &[u8]) -> String {
    let mut out = String::new();
    let mut i = 0;
    let n = raw.len();
    let hex = |b: &[u8]| -> Option<u32> { u32::from_str_radix(std::str::from_utf8(b).ok()?, 16).ok() };
    while i < n {
        if raw[i] == b'\\' && i + 1 < n {
            if raw[i..].starts_with(b"\\X2\\") {
                i += 4;
                while i + 4 <= n && !raw[i..].starts_with(b"\\X0\\") {
                    if let Some(cp) = hex(&raw[i..i + 4]) {
                        if let Some(ch) = char::from_u32(cp) { out.push(ch); }
                    }
                    i += 4;
                }
                i += 4;
                continue;
            }
            if raw[i..].starts_with(b"\\X4\\") {
                i += 4;
                while i + 8 <= n && !raw[i..].starts_with(b"\\X0\\") {
                    if let Some(cp) = hex(&raw[i..i + 8]) {
                        if let Some(ch) = char::from_u32(cp) { out.push(ch); }
                    }
                    i += 8;
                }
                i += 4;
                continue;
            }
            if raw[i..].starts_with(b"\\X\\") && i + 5 <= n {
                if let Some(cp) = hex(&raw[i + 3..i + 5]) {
                    if let Some(ch) = char::from_u32(cp) { out.push(ch); }
                }
                i += 5;
                continue;
            }
            if raw[i..].starts_with(b"\\S\\") && i + 4 <= n {
                out.push(char::from_u32(raw[i + 3] as u32 + 128).unwrap_or('?'));
                i += 4;
                continue;
            }
            if raw[i..].starts_with(b"\\\\") {
                out.push('\\');
                i += 2;
                continue;
            }
        }
        // tolerante a UTF-8 (alguns exportadores escrevem UTF-8 diretamente)
        let st = i;
        i += 1;
        while i < n && raw[i] != b'\\' { i += 1; }
        out.push_str(&String::from_utf8_lossy(&raw[st..i]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decode() {
        assert_eq!(decode_step_string(b"Gr\\X2\\00F6\\X0\\\\X2\\00DF\\X0\\e"), "Größe");
        assert_eq!(decode_step_string(b"L\\X\\E4nge"), "Länge");
    }
    #[test]
    fn parse() {
        let s = b"#12= IFCPROPERTYSINGLEVALUE('Nennweite',$,IFCLENGTHMEASURE(50.),$);";
        let e = parse_stmt_at(s, 0).unwrap();
        assert_eq!(e.id, 12);
        assert_eq!(e.ty, "IFCPROPERTYSINGLEVALUE");
        assert_eq!(e.str(0), Some("Nennweite"));
        assert_eq!(e.f64(2), Some(50.0));
        let s = b"#5=IFCLABEL('a;b''c');";
        assert_eq!(stmt_end(s, 0), s.len() - 1);
    }
}
