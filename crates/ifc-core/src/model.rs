//! Semântica: reconhecer componentes, atribuir propriedades/material/sistema/pavimento,
//! filtros, geometria simplificada (eixos de tubos + pontos) e levantamento de quantitativos.

use crate::geom::{Mat4, V3};
use crate::step::{Arg, Entity, Progress, StepError, StepFile};
use regex::Regex;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::OnceLock;
use std::time::Instant;

// ------------------------------------------------------------------ Classes
const PIPE: &[&str] = &["IFCPIPESEGMENT"];
const FITTING: &[&str] = &["IFCPIPEFITTING"];
const ACCESSORY: &[&str] = &[
    "IFCFIRESUPPRESSIONTERMINAL", "IFCVALVE", "IFCFLOWMETER", "IFCFLOWINSTRUMENT", "IFCSENSOR",
    "IFCALARM", "IFCPUMP", "IFCTANK", "IFCACTUATOR", "IFCCONTROLLER", "IFCFILTER", "IFCINTERCEPTOR",
    "IFCDISCRETEACCESSORY", "IFCMECHANICALFASTENER", "IFCFASTENER",
];
// Classes genéricas IFC2x3 (atribuição via objeto de tipo/nome)
const GENERIC: &[(&str, Cat)] = &[
    ("IFCFLOWSEGMENT", Cat::Rohr), ("IFCFLOWFITTING", Cat::Formstueck),
    ("IFCFLOWCONTROLLER", Cat::Zubehoer), ("IFCFLOWTERMINAL", Cat::Zubehoer),
    ("IFCFLOWMOVINGDEVICE", Cat::Zubehoer), ("IFCFLOWSTORAGEDEVICE", Cat::Zubehoer),
    ("IFCFLOWTREATMENTDEVICE", Cat::Zubehoer), ("IFCDISTRIBUTIONCONTROLELEMENT", Cat::Zubehoer),
    ("IFCDISTRIBUTIONFLOWELEMENT", Cat::Zubehoer),
];
/// Só considerado se atribuído a um sistema (p. ex. suportes do Revit).
const PROXY: &[&str] = &["IFCBUILDINGELEMENTPROXY", "IFCFLOWSEGMENT"];

const SYSTEM_TYPES: &[&str] = &["IFCSYSTEM", "IFCDISTRIBUTIONSYSTEM", "IFCDISTRIBUTIONCIRCUIT", "IFCBUILDINGSYSTEM"];

pub const DEFAULT_SYSTEM_RX: &str = r"(?i)sprinkler|\bspk|\bspr|feuer|lösch|loesch|fireprotection|fire|sprink|hydrant|steigleitung|\btsl\b|dry.?riser|standpipe";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, PartialOrd, Ord)]
pub enum Cat {
    Rohr,
    Formstueck,
    Zubehoer,
}

impl Cat {
    pub fn label(&self) -> &'static str {
        match self {
            Cat::Rohr => "Rohre",
            Cat::Formstueck => "Formstücke",
            Cat::Zubehoer => "Armaturen & Zubehör",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub enum PVal {
    Num { v: f64, length: bool },
    Text(String),
}

impl PVal {
    fn display(&self) -> String {
        match self {
            PVal::Num { v, .. } => fmt_num(*v),
            PVal::Text(t) => t.clone(),
        }
    }
}

fn fmt_num(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 { format!("{}", v.round() as i64) } else { format!("{:.3}", v).trim_end_matches('0').to_string() }
}

#[derive(Debug, Clone, Serialize)]
pub struct Element {
    pub id: u32,
    pub guid: String,
    pub cat: Cat,
    pub class: String,
    pub name: String,
    pub object_type: String,
    pub predefined: String,
    pub type_name: String,
    pub kind: String,
    /// Sprinklers / hidrantes / coluna seca / não atribuído
    pub anlage: String,
    pub dn: Option<f64>,
    pub dn_label: String,
    pub dn_src: String,
    pub length_m: Option<f64>,
    pub len_src: String,
    pub material: String,
    /// Isolamento térmico (somente tubos)
    pub insulation: Option<Insulation>,
    pub systems: Vec<String>,
    pub worksets: Vec<String>,
    pub storey: String,
    #[serde(skip)]
    pub placement: Option<u32>,
    #[serde(skip)]
    pub representation: Option<u32>,
    #[serde(skip)]
    pub props: Vec<(String, String, PVal)>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Insulation {
    pub kind: String,
    pub thickness_mm: Option<f64>,
    pub source: String,
}

impl Insulation {
    pub fn label(&self) -> String {
        match self.thickness_mm {
            Some(t) => format!("{} mm", t.round() as i64),
            None => "Stärke unbekannt".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Count {
    pub name: String,
    pub count: usize,
    pub sprinkler_like: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanSummary {
    pub file: String,
    pub file_size: u64,
    pub schema: String,
    pub unit_scale_m: f64,
    pub entities_indexed: usize,
    pub entities_skipped: u64,
    pub elements: usize,
    pub systems: Vec<Count>,
    pub worksets: Vec<Count>,
    pub storeys: Vec<Count>,
    pub categories: Vec<Count>,
    pub seconds: f64,
}

#[derive(Debug, Clone, serde::Deserialize, Default)]
pub struct Filter {
    /// Nomes de sistema; "(ohne System)" para componentes não atribuídos. Vazio = todos.
    pub systems: Vec<String>,
    /// Worksets / áreas / camadas; "(ohne Workset)". Vazio = todos.
    #[serde(default)]
    pub worksets: Vec<String>,
    /// Nomes de pavimento; "(ohne Geschoss)". Vazio = todos.
    pub storeys: Vec<String>,
    /// "Rohre", "Formstücke", "Armaturen & Zubehör". Vazio = todos.
    pub categories: Vec<String>,
}

pub const NO_SYSTEM: &str = "(ohne System)";
pub const NO_STOREY: &str = "(ohne Geschoss)";
pub const NO_WORKSET: &str = "(ohne Workset)";

pub struct Model {
    pub step: StepFile,
    pub path: String,
    pub scale: f64,
    pub elements: Vec<Element>,
    pub summary: ScanSummary,
    pub selection: Vec<usize>,
}

// ------------------------------------------------------------------ Varredura
impl Model {
    pub fn scan(path: &str, progress: Progress, cancel: &AtomicBool) -> Result<Model, StepError> {
        let t0 = Instant::now();
        let lower = path.to_lowercase();
        let real = if lower.ends_with(".ifczip") || lower.ends_with(".zip") {
            crate::step::unpack_ifczip(path, progress, cancel)?
        } else {
            path.to_string()
        };
        let step = StepFile::open(&real, progress, cancel)?;
        let scale = unit_scale(&step);
        let elements = collect_elements(&step, scale);

        let mut sys: BTreeMap<String, usize> = BTreeMap::new();
        let mut st: BTreeMap<String, usize> = BTreeMap::new();
        let mut ws: BTreeMap<String, usize> = BTreeMap::new();
        let mut cats: BTreeMap<Cat, usize> = BTreeMap::new();
        for e in &elements {
            if e.systems.is_empty() {
                *sys.entry(NO_SYSTEM.into()).or_default() += 1;
            }
            for s in &e.systems {
                *sys.entry(s.clone()).or_default() += 1;
            }
            if e.worksets.is_empty() {
                *ws.entry(NO_WORKSET.into()).or_default() += 1;
            }
            for w in &e.worksets {
                *ws.entry(w.clone()).or_default() += 1;
            }
            *st.entry(if e.storey.is_empty() { NO_STOREY.into() } else { e.storey.clone() }).or_default() += 1;
            *cats.entry(e.cat).or_default() += 1;
        }
        let rx = Regex::new(DEFAULT_SYSTEM_RX).unwrap();
        let systems = sys.into_iter().map(|(name, count)| Count { sprinkler_like: rx.is_match(&name), name, count }).collect();
        let summary = ScanSummary {
            file: path.to_string(),
            file_size: step.file_size(),
            schema: step.schema.clone(),
            unit_scale_m: scale,
            entities_indexed: step.len(),
            entities_skipped: step.skipped_count,
            elements: elements.len(),
            systems,
            worksets: ws.into_iter().map(|(name, count)| Count { sprinkler_like: rx.is_match(&name), name, count }).collect(),
            storeys: st.into_iter().map(|(name, count)| Count { name, count, sprinkler_like: false }).collect(),
            categories: cats.into_iter().map(|(c, count)| Count { name: c.label().into(), count, sprinkler_like: false }).collect(),
            seconds: t0.elapsed().as_secs_f64(),
        };
        Ok(Model { step, path: path.to_string(), scale, elements, summary, selection: vec![] })
    }

    /// Sugestão para o filtro de sistemas: todos os sistemas que parecem ser de sprinklers.
    /// Se não houver nenhum, todos os sistemas são selecionados.
    pub fn default_filter(&self) -> Filter {
        let s: Vec<String> = self.summary.systems.iter().filter(|c| c.sprinkler_like).map(|c| c.name.clone()).collect();
        let w: Vec<String> = if s.is_empty() {
            self.summary.worksets.iter().filter(|c| c.sprinkler_like).map(|c| c.name.clone()).collect()
        } else { vec![] };
        Filter { systems: s, worksets: w, storeys: vec![], categories: vec![] }
    }

    pub fn apply_filter(&mut self, f: &Filter) -> usize {
        let sys: HashSet<&str> = f.systems.iter().map(|s| s.as_str()).collect();
        let ws: HashSet<&str> = f.worksets.iter().map(|s| s.as_str()).collect();
        let st: HashSet<&str> = f.storeys.iter().map(|s| s.as_str()).collect();
        let cat: HashSet<&str> = f.categories.iter().map(|s| s.as_str()).collect();
        self.selection = self
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                let sys_ok = sys.is_empty()
                    || (e.systems.is_empty() && sys.contains(NO_SYSTEM))
                    || e.systems.iter().any(|s| sys.contains(s.as_str()));
                let ws_ok = ws.is_empty()
                    || (e.worksets.is_empty() && ws.contains(NO_WORKSET))
                    || e.worksets.iter().any(|w| ws.contains(w.as_str()));
                let st_name = if e.storey.is_empty() { NO_STOREY } else { e.storey.as_str() };
                let st_ok = st.is_empty() || st.contains(st_name);
                let cat_ok = cat.is_empty() || cat.contains(e.cat.label());
                sys_ok && ws_ok && st_ok && cat_ok
            })
            .map(|(i, _)| i)
            .collect();
        self.selection.len()
    }

    /// Passagem 2: carrega a geometria simplificada somente para a seleção.
    pub fn load_geometry(&mut self, progress: Progress, cancel: &AtomicBool) -> Result<Geometry, StepError> {
        // Arquivos menores: indexar todas as entidades de geometria em uma passagem.
        // Arquivos grandes: só o necessário, nível por nível (economiza memória).
        if self.step.file_size() < 1_500_000_000 && std::env::var("SPK_BIGFILE").is_err() {
            self.step.load_all_skipped(progress, cancel)?;
        } else {
            for _ in 0..7 {
                let mut missing = HashSet::new();
                {
                    let mut g = GeomCtx::new(&self.step, self.scale, Some(&mut missing));
                    for &i in &self.selection {
                        let e = &self.elements[i];
                        g.element_geometry(e.placement, e.representation);
                    }
                }
                if missing.is_empty() { break; }
                self.step.load_extra(&missing, progress, cancel)?;
            }
        }
        let mut out = Geometry::default();
        let mut g = GeomCtx::new(&self.step, self.scale, None);
        for &i in &self.selection {
            let (placement, rep) = (self.elements[i].placement, self.elements[i].representation);
            let mut eg = g.element_geometry(placement, rep);
            let e = &mut self.elements[i];
            if e.cat == Cat::Rohr && eg.segments.is_empty() {
                if let Some(ax) = eg.mesh_axis {
                    eg.segments.push(ax);
                    if eg.outer_diameter_mm.is_none() && eg.mesh_radius_mm > 0.0 {
                        eg.outer_diameter_mm = Some(2.0 * eg.mesh_radius_mm);
                    }
                }
            }
            // Comprimento a partir do eixo, se não for conhecido pelos dados
            if e.cat == Cat::Rohr && e.length_m.is_none() {
                let l: f64 = eg.segments.iter().map(|(a, b)| a.dist(b)).sum();
                if l > 0.0 {
                    e.length_m = Some(l);
                    e.len_src = if eg.from_mesh { "Geometrie (Flächenmodell)".into() } else { "Geometrie (Achse)".into() };
                }
            }
            // DN a partir do diâmetro externo da geometria
            if e.dn.is_none() {
                if let Some(od) = eg.outer_diameter_mm {
                    let (dn, lbl) = dn_from_od(od);
                    e.dn = Some(dn);
                    e.dn_label = lbl;
                    e.dn_src = "Geometrie (Außendurchmesser)".into();
                }
            }
            out.push(i, e, &eg);
        }
        out.finish();
        Ok(out)
    }

    pub fn takeoff(&self) -> Takeoff {
        Takeoff::build(&self.elements, &self.selection, None)
    }

    /// Levantamento de quantitativos de um sistema ("Sprinkler", "Hydranten", "Trockensteigleitung", "Nicht zugeordnet"); None = todos.
    pub fn takeoff_for(&self, anlage: Option<&str>) -> Takeoff {
        Takeoff::build(&self.elements, &self.selection, anlage)
    }

    /// Quantidade de componentes por sistema na seleção atual.
    pub fn anlagen(&self) -> Vec<Count> {
        let mut m: BTreeMap<usize, (String, usize)> = BTreeMap::new();
        for &i in &self.selection {
            let a = &self.elements[i].anlage;
            let k = ANLAGEN.iter().position(|x| x == a).unwrap_or(ANLAGEN.len());
            m.entry(k).or_insert((a.clone(), 0)).1 += 1;
        }
        m.into_values().map(|(name, count)| Count { name, count, sprinkler_like: false }).collect()
    }

    /// Todas as propriedades de um componente (para o painel de propriedades).
    pub fn details(&self, idx: usize) -> Option<ElementDetails> {
        let e = self.elements.get(idx)?;
        let mut props: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
        for (ps, k, v) in &e.props {
            props.entry(ps.clone()).or_default().push((k.clone(), v.display()));
        }
        Some(ElementDetails { element: e.clone(), psets: props.into_iter().collect() })
    }
}

#[derive(Debug, Serialize)]
pub struct ElementDetails {
    pub element: Element,
    pub psets: Vec<(String, Vec<(String, String)>)>,
}

// ------------------------------------------------------------------ Unidades
pub(crate) fn unit_scale_pub(step: &StepFile) -> f64 { unit_scale(step) }

fn unit_scale(step: &StepFile) -> f64 {
    for ua in step.ids_of_type("IFCUNITASSIGNMENT") {
        let Some(u) = step.get(ua) else { continue };
        for r in u.arg(0).refs() {
            let Some(ent) = step.get(r) else { continue };
            match ent.ty.as_str() {
                "IFCSIUNIT" if ent.arg(1).as_enum() == Some("LENGTHUNIT") => {
                    let pre = match ent.arg(2).as_enum() {
                        Some("MILLI") => 1e-3,
                        Some("CENTI") => 1e-2,
                        Some("DECI") => 1e-1,
                        Some("KILO") => 1e3,
                        _ => 1.0,
                    };
                    return pre;
                }
                "IFCCONVERSIONBASEDUNIT" if ent.arg(1).as_enum() == Some("LENGTHUNIT") => {
                    let name = ent.str(2).unwrap_or("").to_uppercase();
                    if let Some(m) = ent.rf(3).and_then(|x| step.get(x)) {
                        if let Some(v) = m.f64(0) {
                            let base = m.rf(1).map(|u| {
                                let su = step.get(u);
                                match su.as_ref().and_then(|s| s.arg(2).as_enum()) {
                                    Some("MILLI") => 1e-3,
                                    Some("CENTI") => 1e-2,
                                    _ => 1.0,
                                }
                            }).unwrap_or(1.0);
                            return v * base;
                        }
                    }
                    return if name.contains("FOOT") { 0.3048 } else if name.contains("INCH") { 0.0254 } else { 1.0 };
                }
                _ => {}
            }
        }
    }
    1.0
}

// ------------------------------------------------------------------ Elementos
struct Rels {
    type_of: HashMap<u32, u32>,
    psets_of: HashMap<u32, Vec<u32>>,
    groups_of: HashMap<u32, Vec<u32>>,
    material_of: HashMap<u32, u32>,
    container_of: HashMap<u32, u32>,
    parent_of: HashMap<u32, u32>,
    covers_of: HashMap<u32, Vec<u32>>,
}

fn collect_elements(step: &StepFile, scale: f64) -> Vec<Element> {
    let mut wanted: Vec<&str> = Vec::new();
    wanted.extend(PIPE);
    wanted.extend(FITTING);
    wanted.extend(ACCESSORY);
    wanted.extend(GENERIC.iter().map(|(c, _)| *c));
    wanted.push("IFCBUILDINGELEMENTPROXY");
    let cands = step.ids_of_types(&wanted);
    let cand_set: HashSet<u32> = cands.iter().map(|(i, _)| *i).collect();

    // Relações
    let mut rels = Rels {
        type_of: HashMap::new(),
        psets_of: HashMap::new(),
        groups_of: HashMap::new(),
        material_of: HashMap::new(),
        container_of: HashMap::new(),
        parent_of: HashMap::new(),
        covers_of: HashMap::new(),
    };
    for (rid, ty) in step.ids_of_types(&[
        "IFCRELDEFINESBYTYPE", "IFCRELDEFINESBYPROPERTIES", "IFCRELASSIGNSTOGROUP",
        "IFCRELASSOCIATESMATERIAL", "IFCRELCONTAINEDINSPATIALSTRUCTURE", "IFCRELAGGREGATES",
        "IFCRELCOVERSBLDGELEMENTS",
    ]) {
        let Some(r) = step.get(rid) else { continue };
        match ty.as_str() {
            "IFCRELDEFINESBYTYPE" => {
                if let Some(t) = r.rf(5) {
                    for o in r.arg(4).refs() {
                        if cand_set.contains(&o) { rels.type_of.insert(o, t); }
                    }
                }
            }
            "IFCRELDEFINESBYPROPERTIES" => {
                if let Some(p) = r.rf(5) {
                    for o in r.arg(4).refs() {
                        if cand_set.contains(&o) { rels.psets_of.entry(o).or_default().push(p); }
                    }
                }
            }
            "IFCRELASSIGNSTOGROUP" => {
                if let Some(g) = r.rf(6) {
                    for o in r.arg(4).refs() {
                        if cand_set.contains(&o) { rels.groups_of.entry(o).or_default().push(g); }
                    }
                }
            }
            "IFCRELASSOCIATESMATERIAL" => {
                if let Some(m) = r.rf(5) {
                    for o in r.arg(4).refs() { rels.material_of.insert(o, m); }
                }
            }
            "IFCRELCONTAINEDINSPATIALSTRUCTURE" => {
                if let Some(s) = r.rf(5) {
                    for o in r.arg(4).refs() {
                        if cand_set.contains(&o) { rels.container_of.insert(o, s); }
                    }
                }
            }
            "IFCRELCOVERSBLDGELEMENTS" => {
                if let Some(el) = r.rf(4) {
                    if cand_set.contains(&el) { rels.covers_of.entry(el).or_default().extend(r.arg(5).refs()); }
                }
            }
            "IFCRELAGGREGATES" => {
                if let Some(p) = r.rf(4) {
                    for o in r.arg(5).refs() { rels.parent_of.insert(o, p); }
                }
            }
            _ => {}
        }
    }

    // Camadas (worksets do Revit/layers de CAD): representação -> nome da camada
    let mut layer_of: HashMap<u32, Vec<String>> = HashMap::new();
    for (lid, _) in step.ids_of_types(&["IFCPRESENTATIONLAYERASSIGNMENT", "IFCPRESENTATIONLAYERWITHSTYLE"]) {
        let Some(l) = step.get(lid) else { continue };
        let name = l.str(0).unwrap_or("").to_string();
        if name.is_empty() { continue; }
        for it in l.arg(2).refs() { layer_of.entry(it).or_default().push(name.clone()); }
    }

    let sys_types: HashSet<&str> = SYSTEM_TYPES.iter().copied().collect();
    let mut group_name_cache: HashMap<u32, Option<Vec<String>>> = HashMap::new();
    let mut storey_cache: HashMap<u32, String> = HashMap::new();
    let mut material_cache: HashMap<u32, String> = HashMap::new();
    let mut pset_cache: HashMap<u32, Vec<(String, String, PVal)>> = HashMap::new();

    let mut out = Vec::with_capacity(cands.len());
    for (id, class) in cands {
        let Some(ent) = step.get(id) else { continue };
        // Sistemas
        let mut systems = Vec::new();
        for g in rels.groups_of.get(&id).map(|v| v.as_slice()).unwrap_or(&[]) {
            let names = group_name_cache.entry(*g).or_insert_with(|| {
                let ge = step.get(*g)?;
                if !sys_types.contains(ge.ty.as_str()) { return None; }
                let mut v = vec![];
                let name = ge.str(2).or(ge.str(5)).unwrap_or("System").to_string();
                v.push(name);
                Some(v)
            });
            if let Some(n) = names { systems.extend(n.iter().cloned()); }
        }
        let typ = rels.type_of.get(&id).and_then(|t| step.get(*t));
        // Categoria
        let cat = if PIPE.contains(&class.as_str()) {
            Some(Cat::Rohr)
        } else if FITTING.contains(&class.as_str()) {
            Some(Cat::Formstueck)
        } else if ACCESSORY.contains(&class.as_str()) {
            Some(Cat::Zubehoer)
        } else {
            let tcls = typ.as_ref().map(|t| t.ty.as_str()).unwrap_or("");
            if tcls.contains("PIPESEGMENT") {
                Some(Cat::Rohr)
            } else if tcls.contains("PIPEFITTING") {
                Some(Cat::Formstueck)
            } else if exclude_rx().is_match(tcls) || exclude_rx().is_match(ent.str(2).unwrap_or("")) {
                None
            } else if PROXY.contains(&class.as_str()) && systems.is_empty() && !piping_rx().is_match(&format!("{} {}", ent.str(2).unwrap_or(""), ent.str(4).unwrap_or(""))) {
                None
            } else if class == "IFCBUILDINGELEMENTPROXY" {
                let txt = format!("{} {} {}", ent.str(2).unwrap_or(""), ent.str(4).unwrap_or(""), typ.as_ref().and_then(|t| t.str(2)).unwrap_or(""));
                if Regex::new(r"(?i)\brohr\b|^rohr|pipe\b|leitung").unwrap().is_match(&txt) && !hints(FITTING_HINTS, &txt).is_some() {
                    Some(Cat::Rohr)
                } else if hints(FITTING_HINTS, &txt).is_some() {
                    Some(Cat::Formstueck)
                } else {
                    Some(Cat::Zubehoer)
                }
            } else {
                GENERIC.iter().find(|(c, _)| *c == class).map(|(_, c)| *c)
            }
        };
        let Some(cat) = cat else { continue };

        // Propriedades: tipo primeiro, instância sobrescreve
        let mut props: Vec<(String, String, PVal)> = Vec::new();
        if let Some(t) = &typ {
            for p in t.arg(5).refs() {
                props.extend(pset_cache.entry(p).or_insert_with(|| read_pset(step, p)).iter().cloned());
            }
        }
        for p in rels.psets_of.get(&id).map(|v| v.as_slice()).unwrap_or(&[]) {
            props.extend(pset_cache.entry(*p).or_insert_with(|| read_pset(step, *p)).iter().cloned());
        }
        let name = ent.str(2).unwrap_or("").to_string();
        let mut type_name = typ.as_ref().and_then(|t| t.str(2)).unwrap_or("").to_string();
        if type_name.is_empty() || type_name.eq_ignore_ascii_case(&name) {
            if let Some((_, _, v)) = find_prop(&props, &["Typ", "Type", "Artikel", "Bezeichnung", "Beschreibung"]) {
                let t = v.display();
                if !t.trim().is_empty() { type_name = t; }
            }
        }
        let object_type = ent.str(4).unwrap_or("").to_string();
        let predefined = ent.args.get(8).and_then(|a| a.as_enum()).filter(|_| ent.args.len() > 8)
            .map(|s| s.to_string())
            .or_else(|| typ.as_ref().and_then(|t| t.args.get(9).and_then(|a| a.as_enum()).map(|s| s.to_string())))
            .unwrap_or_default();

        // Sistema a partir das propriedades, se não houver atribuição IfcSystem (p. ex. Revit)
        if systems.is_empty() {
            if let Some((_, _, v)) = find_prop(&props, SYSTEM_PROP_KEYS) {
                let t = v.display();
                if !t.trim().is_empty() { systems.push(t); }
            }
        }
        systems.sort();
        systems.dedup();

        // Material: instância, senão tipo, senão propriedade
        let mut material = rels.material_of.get(&id).or_else(|| typ.as_ref().and_then(|t| rels.material_of.get(&t.id)))
            .map(|m| material_cache.entry(*m).or_insert_with(|| material_name(step, *m)).clone())
            .unwrap_or_default();
        if material.is_empty() {
            if let Some((_, _, v)) = find_prop(&props, &["Material", "Werkstoff", "Rohrmaterial", "Pipe Material"]) {
                material = v.display();
            }
        }

        // Pavimento
        let storey = rels.container_of.get(&id).map(|c| {
            storey_cache.entry(*c).or_insert_with(|| storey_name(step, &rels, *c)).clone()
        }).unwrap_or_default();

        // Diâmetro nominal
        let (mut dn, mut dn_src) = (None, String::new());
        for key in DN_KEYS {
            if let Some((ps, k, v)) = find_prop(&props, &[key]) {
                let val = match v {
                    PVal::Num { v, length: true } => Some(v * scale * 1000.0),
                    PVal::Num { v, .. } => Some(*v),
                    PVal::Text(t) => first_number(t),
                };
                if let Some(mut x) = val {
                    if x > 0.0 && x < 1.0 { x *= 1000.0; }
                    if x > 0.0 {
                        dn = Some(x);
                        dn_src = format!("{ps}.{k}");
                        break;
                    }
                }
            }
        }
        if dn.is_none() {
            if let Some((ps, k, v)) = find_prop(&props, OD_KEYS) {
                let od = match v { PVal::Num { v, length: true } => Some(v * scale * 1000.0), PVal::Num { v, .. } => Some(*v), PVal::Text(t) => first_number(t) };
                if let Some(od) = od.filter(|x| *x > 5.0) {
                    let (d, _) = dn_from_od(od);
                    dn = Some(d);
                    dn_src = format!("{ps}.{k} (Außendurchmesser)");
                }
            }
        }
        if dn.is_none() {
            if let Some(x) = dn_from_text(&format!("{name} {type_name} {object_type}")) {
                dn = Some(x);
                dn_src = "Name/Typname".into();
            }
        }
        let dn_label = dn.map(|d| format!("DN {}", d.round() as i64)).unwrap_or_default();

        // Comprimento (somente tubos)
        let (mut length_m, mut len_src) = (None, String::new());
        if cat == Cat::Rohr {
            for (ps, k, v) in &props {
                if len_key_rx().is_match(k) {
                    let val = match v {
                        PVal::Num { v, length: true } => Some(v * scale),
                        PVal::Num { v, .. } => Some(v * scale),
                        PVal::Text(t) => first_number(t).map(|x| x / 1000.0),
                    };
                    if let Some(x) = val.filter(|x| *x > 0.0) {
                        length_m = Some(x);
                        len_src = format!("{ps}.{k}");
                        if ps.starts_with("Qto_") || ps.contains("Quantit") { break; }
                    }
                }
            }
            if length_m.is_none() {
                if let Some(d) = extrusion_depth(step, ent.rf(6)) {
                    length_m = Some(d * scale);
                    len_src = "Extrusionslänge".into();
                }
            }
        }

        // Isolamento
        let insulation = if cat == Cat::Rohr {
            insulation_of(step, &rels, id, &props, scale, &mut pset_cache, &mut material_cache)
        } else { None };

        // Worksets: propriedade, senão camada da representação
        let mut worksets: Vec<String> = props.iter()
            .filter(|(_, k, _)| WORKSET_KEYS.iter().any(|w| k.eq_ignore_ascii_case(w)))
            .map(|(_, _, v)| v.display()).filter(|v| !v.is_empty()).collect();
        if worksets.is_empty() && !layer_of.is_empty() {
            if let Some(pds) = ent.rf(6).and_then(|r| step.get(r)) {
                for r in pds.arg(2).refs() {
                    if let Some(l) = layer_of.get(&r) { worksets.extend(l.iter().cloned()); }
                }
            }
        }
        worksets.sort();
        worksets.dedup();

        let text = format!("{name} {type_name} {object_type}");
        let kind = match cat {
            Cat::Rohr => "Rohr".to_string(),
            Cat::Formstueck => fitting_kind(&predefined, &text),
            Cat::Zubehoer => accessory_kind(&class, &predefined, &text),
        };
        let anlage = classify_anlage(&systems, &worksets, &kind, &text);

        out.push(Element {
            id,
            guid: ent.str(0).unwrap_or("").to_string(),
            cat,
            class: pretty_class(&class),
            name,
            object_type,
            predefined,
            type_name,
            kind,
            anlage,
            dn,
            dn_label,
            dn_src,
            length_m,
            len_src,
            material,
            insulation,
            systems,
            worksets,
            storey,
            placement: ent.rf(5),
            representation: ent.rf(6),
            props,
        });
    }
    out
}

fn insul_rx() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)dämm|daemm|isolier|insulat|wärmeschutz|kälteschutz|armaflex|kaiflex").unwrap())
}
fn thick_rx() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)stärke|staerke|dicke|thickness").unwrap())
}
fn none_rx() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)^\s*(keine?|ohne|none|nein|no|false|0|-|–)?\s*$").unwrap())
}

fn thickness_mm(v: &PVal, scale: f64) -> Option<f64> {
    let t = match v {
        PVal::Num { v, length: true } => *v * scale * 1000.0,
        PVal::Num { v, .. } => *v,
        PVal::Text(t) => first_number(t)?,
    };
    let t = if t > 0.0 && t < 1.0 { t * 1000.0 } else { t };
    (t > 0.0).then_some(t)
}

/// Isolamento de um tubo: componentes IfcCovering atribuídos, senão propriedades no tubo.
fn insulation_of(
    step: &StepFile, rels: &Rels, id: u32, props: &[(String, String, PVal)], scale: f64,
    pset_cache: &mut HashMap<u32, Vec<(String, String, PVal)>>, material_cache: &mut HashMap<u32, String>,
) -> Option<Insulation> {
    for cov in rels.covers_of.get(&id).map(|v| v.as_slice()).unwrap_or(&[]) {
        let Some(c) = step.get(*cov) else { continue };
        if c.ty != "IFCCOVERING" { continue; }
        let pre = c.args.get(8).and_then(|a| a.as_enum()).unwrap_or("");
        let name = c.str(2).unwrap_or("").to_string();
        let otype = c.str(4).unwrap_or("").to_string();
        let mut cprops: Vec<(String, String, PVal)> = vec![];
        for p in rels.psets_of.get(cov).map(|v| v.as_slice()).unwrap_or(&[]) {
            cprops.extend(pset_cache.entry(*p).or_insert_with(|| read_pset(step, *p)).iter().cloned());
        }
        let mat = rels.material_of.get(cov).map(|m| material_cache.entry(*m).or_insert_with(|| material_name(step, *m)).clone()).unwrap_or_default();
        let is_ins = matches!(pre, "INSULATION" | "WRAPPING") || insul_rx().is_match(&format!("{name} {otype} {mat}"));
        if !is_ins { continue; }
        let mut thick = cprops.iter().find(|(_, k, _)| thick_rx().is_match(k) || k.eq_ignore_ascii_case("NominalThickness")).and_then(|(_, _, v)| thickness_mm(v, scale));
        if thick.is_none() {
            thick = rels.material_of.get(cov).and_then(|m| layer_thickness(step, *m)).map(|t| t * scale * 1000.0);
        }
        if thick.is_none() {
            thick = first_number_mm(&format!("{name} {otype}"));
        }
        let kind = [otype.as_str(), name.as_str(), mat.as_str()].into_iter().find(|x| !x.is_empty()).unwrap_or("Dämmung").to_string();
        return Some(Insulation { kind, thickness_mm: thick, source: "Dämm-Bauteil (IfcCovering)".into() });
    }
    // Propriedades no tubo
    let ins: Vec<&(String, String, PVal)> = props.iter().filter(|(ps, k, _)| insul_rx().is_match(k) || insul_rx().is_match(ps)).collect();
    if ins.is_empty() { return None; }
    let thick = ins.iter().filter(|(_, k, _)| thick_rx().is_match(k)).find_map(|(_, _, v)| thickness_mm(v, scale));
    let kind = ins.iter().filter(|(_, k, _)| !thick_rx().is_match(k)).map(|(_, _, v)| v.display()).find(|t| !none_rx().is_match(t) && first_number(t).map_or(true, |_| t.chars().any(|c| c.is_alphabetic())));
    if kind.is_none() && thick.is_none() { return None; }
    if kind.is_none() && thick.is_some() && thick.unwrap() <= 0.0 { return None; }
    let src = ins.first().map(|(ps, k, _)| format!("{ps}.{k}")).unwrap_or_default();
    Some(Insulation { kind: kind.unwrap_or_else(|| "Dämmung".into()), thickness_mm: thick, source: src })
}

fn first_number_mm(t: &str) -> Option<f64> {
    static R: OnceLock<Regex> = OnceLock::new();
    let r = R.get_or_init(|| Regex::new(r"(?i)(\d+(?:[.,]\d+)?)\s?mm").unwrap());
    r.captures(t).and_then(|c| c[1].replace(',', ".").parse().ok())
}

fn layer_thickness(step: &StepFile, id: u32) -> Option<f64> {
    let e = step.get(id)?;
    match e.ty.as_str() {
        "IFCMATERIALLAYERSETUSAGE" => layer_thickness(step, e.rf(0)?),
        "IFCMATERIALLAYERSET" => {
            let s: f64 = e.arg(0).refs().iter().filter_map(|l| step.get(*l)).filter_map(|l| l.f64(1)).sum();
            (s > 0.0).then_some(s)
        }
        "IFCMATERIALLAYER" => e.f64(1),
        _ => None,
    }
}

fn pretty_class(c: &str) -> String {
    // IFCPIPESEGMENT -> IfcPipeSegment (grafia simplificada)
    const KNOWN: &[(&str, &str)] = &[
        ("IFCPIPESEGMENT", "IfcPipeSegment"), ("IFCPIPEFITTING", "IfcPipeFitting"),
        ("IFCFIRESUPPRESSIONTERMINAL", "IfcFireSuppressionTerminal"), ("IFCVALVE", "IfcValve"),
        ("IFCFLOWMETER", "IfcFlowMeter"), ("IFCFLOWINSTRUMENT", "IfcFlowInstrument"),
        ("IFCSENSOR", "IfcSensor"), ("IFCALARM", "IfcAlarm"), ("IFCPUMP", "IfcPump"),
        ("IFCFLOWSEGMENT", "IfcFlowSegment"), ("IFCFLOWFITTING", "IfcFlowFitting"),
        ("IFCFLOWCONTROLLER", "IfcFlowController"), ("IFCFLOWTERMINAL", "IfcFlowTerminal"),
        ("IFCBUILDINGELEMENTPROXY", "IfcBuildingElementProxy"), ("IFCDISCRETEACCESSORY", "IfcDiscreteAccessory"),
        ("IFCMECHANICALFASTENER", "IfcMechanicalFastener"), ("IFCTANK", "IfcTank"),
    ];
    KNOWN.iter().find(|(k, _)| *k == c).map(|(_, v)| v.to_string()).unwrap_or_else(|| c.to_string())
}

fn piping_rx() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)sprinkler|rohr|pipe|bogen|t-stück|muffe|kupplung|ventil|valve|düse|hydrant|strömungsmelder|alarm").unwrap())
}

fn exclude_rx() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)duct|cable|kanal|kabel|electric|luft|lüftung|air terminal").unwrap())
}
fn len_key_rx() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)^(length|länge|laenge|netlength|grosslength|rohrlänge|lange)$").unwrap())
}

const SYSTEM_PROP_KEYS: &[&str] = &[
    "System Name", "Systemname", "SystemName", "System", "System Classification", "Systemklassifizierung",
    "System Type", "Systemtyp", "Anlage", "Anlagenname", "Netz",
];

const OD_KEYS: &[&str] = &["Außendurchmesser", "Außenduchmesser", "Aussendurchmesser", "OuterDiameter", "Outside Diameter", "Outer Diameter"];

const WORKSET_KEYS: &[&str] = &["Workset", "Bearbeitungsbereich", "Arbeitsbereich", "Teilprojekt", "Layer", "Ebene", "Bauabschnitt"];

/// Ordem = prioridade
const DN_KEYS: &[&str] = &[
    "NominalDiameter", "Nominal Diameter", "Nennweite", "Nenndurchmesser", "DN", "Dimension (DN)", "Dimension DN", "Nominal Size",
    "Size", "Größe", "Groesse", "Rohrgröße", "Diameter", "Durchmesser",
];

fn find_prop<'a>(props: &'a [(String, String, PVal)], keys: &[&str]) -> Option<&'a (String, String, PVal)> {
    // o último resultado vence (instância sobrescreve tipo)
    for k in keys {
        if let Some(p) = props.iter().rev().find(|(_, key, _)| key.eq_ignore_ascii_case(k)) {
            return Some(p);
        }
    }
    None
}

fn first_number(t: &str) -> Option<f64> {
    static R: OnceLock<Regex> = OnceLock::new();
    let r = R.get_or_init(|| Regex::new(r"(\d+(?:[.,]\d+)?)").unwrap());
    r.captures(t).and_then(|c| c[1].replace(',', ".").parse().ok())
}

fn dn_from_text(t: &str) -> Option<f64> {
    static R: OnceLock<Regex> = OnceLock::new();
    let r = R.get_or_init(|| Regex::new(r"(?i)\bDN\s?(\d{2,3})\b").unwrap());
    r.captures(t).and_then(|c| c[1].parse().ok())
}

/// Diâmetro externo (tubo de aço EN 10255/10220) -> DN
pub fn dn_from_od(od: f64) -> (f64, String) {
    const T: &[(f64, f64)] = &[
        (15.0, 21.3), (20.0, 26.9), (25.0, 33.7), (32.0, 42.4), (40.0, 48.3), (50.0, 60.3),
        (65.0, 76.1), (80.0, 88.9), (100.0, 114.3), (125.0, 139.7), (150.0, 168.3),
        (200.0, 219.1), (250.0, 273.0), (300.0, 323.9),
    ];
    for (dn, d) in T {
        if (od - d).abs() / d < 0.04 {
            return (*dn, format!("DN {}", *dn as i64));
        }
    }
    (od, format!("Ø {} mm", od.round() as i64))
}

fn read_pset(step: &StepFile, id: u32) -> Vec<(String, String, PVal)> {
    let mut out = Vec::new();
    let Some(ps) = step.get(id) else { return out };
    let name = ps.str(2).unwrap_or("").to_string();
    let items = match ps.ty.as_str() {
        "IFCPROPERTYSET" => ps.arg(4).refs(),
        "IFCELEMENTQUANTITY" => ps.arg(5).refs(),
        _ => return out,
    };
    for p in items {
        let Some(pe) = step.get(p) else { continue };
        let key = pe.str(0).unwrap_or("").to_string();
        let val = match pe.ty.as_str() {
            "IFCPROPERTYSINGLEVALUE" => arg_to_pval(pe.arg(2)),
            "IFCPROPERTYENUMERATEDVALUE" => pe.arg(2).as_list().first().and_then(arg_to_pval),
            "IFCQUANTITYLENGTH" => pe.f64(3).map(|v| PVal::Num { v, length: true }),
            "IFCQUANTITYAREA" | "IFCQUANTITYVOLUME" | "IFCQUANTITYCOUNT" | "IFCQUANTITYWEIGHT" => {
                pe.f64(3).map(|v| PVal::Num { v, length: false })
            }
            _ => None,
        };
        if let Some(v) = val {
            out.push((name.clone(), key, v));
        }
    }
    out
}

fn arg_to_pval(a: &Arg) -> Option<PVal> {
    match a {
        Arg::Typed(t, inner) => {
            let length = t.contains("LENGTHMEASURE");
            match inner.as_ref() {
                Arg::Real(v) => Some(PVal::Num { v: *v, length }),
                Arg::Int(v) => Some(PVal::Num { v: *v as f64, length }),
                Arg::Str(s) => Some(PVal::Text(s.clone())),
                Arg::Enum(e) => Some(PVal::Text(match e.as_str() { "T" => "ja".into(), "F" => "nein".into(), x => x.into() })),
                _ => None,
            }
        }
        Arg::Str(s) => Some(PVal::Text(s.clone())),
        Arg::Real(v) => Some(PVal::Num { v: *v, length: false }),
        _ => None,
    }
}

fn material_name(step: &StepFile, id: u32) -> String {
    let mut names: Vec<String> = Vec::new();
    collect_materials(step, id, &mut names, 0);
    let mut seen = HashSet::new();
    names.retain(|n| !n.is_empty() && seen.insert(n.clone()));
    names.join(", ")
}

fn collect_materials(step: &StepFile, id: u32, out: &mut Vec<String>, depth: u8) {
    if depth > 4 { return; }
    let Some(e) = step.get(id) else { return };
    match e.ty.as_str() {
        "IFCMATERIAL" => out.push(e.str(0).unwrap_or("").to_string()),
        "IFCMATERIALLIST" => for r in e.arg(0).refs() { collect_materials(step, r, out, depth + 1) },
        "IFCMATERIALLAYERSETUSAGE" | "IFCMATERIALPROFILESETUSAGE" | "IFCMATERIALPROFILESETUSAGETAPERING" => {
            if let Some(r) = e.rf(0) { collect_materials(step, r, out, depth + 1) }
        }
        "IFCMATERIALLAYERSET" => for r in e.arg(0).refs() { collect_materials(step, r, out, depth + 1) },
        "IFCMATERIALLAYER" => if let Some(r) = e.rf(0) { collect_materials(step, r, out, depth + 1) },
        "IFCMATERIALPROFILESET" | "IFCMATERIALCONSTITUENTSET" => for r in e.arg(2).refs() { collect_materials(step, r, out, depth + 1) },
        "IFCMATERIALPROFILE" | "IFCMATERIALCONSTITUENT" => if let Some(r) = e.rf(2) { collect_materials(step, r, out, depth + 1) },
        _ => {}
    }
}

fn storey_name(step: &StepFile, rels: &Rels, mut id: u32) -> String {
    for _ in 0..8 {
        let Some(e) = step.get(id) else { break };
        if e.ty == "IFCBUILDINGSTOREY" {
            return e.str(2).or(e.str(7)).unwrap_or("Geschoss").to_string();
        }
        if e.ty == "IFCBUILDING" || e.ty == "IFCSITE" { break; }
        match rels.parent_of.get(&id) { Some(p) => id = *p, None => break }
    }
    String::new()
}

fn extrusion_depth(step: &StepFile, rep: Option<u32>) -> Option<f64> {
    let pds = step.get(rep?)?;
    let mut best: Option<f64> = None;
    for r in pds.arg(2).refs() {
        let Some(sr) = step.get(r) else { continue };
        if matches!(sr.str(1), Some("Axis") | Some("FootPrint") | Some("Box")) { continue; }
        let mut stack = sr.arg(3).refs();
        let mut guard = 0;
        while let Some(it) = stack.pop() {
            guard += 1;
            if guard > 64 { break; }
            let Some(x) = step.get(it) else { continue };
            match x.ty.as_str() {
                "IFCEXTRUDEDAREASOLID" | "IFCEXTRUDEDAREASOLIDTAPERED" => {
                    if let Some(d) = x.f64(3) { best = Some(best.map_or(d, |b: f64| b.max(d))); }
                }
                "IFCMAPPEDITEM" => {
                    if let Some(m) = x.rf(0).and_then(|m| step.get(m)) {
                        if let Some(rr) = m.rf(1).and_then(|r| step.get(r)) { stack.extend(rr.arg(3).refs()); }
                    }
                }
                "IFCBOOLEANCLIPPINGRESULT" | "IFCBOOLEANRESULT" => { if let Some(f) = x.rf(1) { stack.push(f) } }
                _ => {}
            }
        }
    }
    best
}

const FITTING_DE: &[(&str, &str)] = &[
    ("BEND", "Bogen"), ("JUNCTION", "T-Stück / Abzweig"), ("TRANSITION", "Reduzierung / Übergang"),
    ("CONNECTOR", "Verbinder / Muffe / Kupplung"), ("ENTRY", "Eintritt"), ("EXIT", "Kappe / Austritt"),
    ("OBSTRUCTION", "Blende / Stopfen"), ("WYE", "Y-Stück"),
];
const FITTING_HINTS: &[(&str, &str)] = &[
    (r"bogen|elbow|bend|winkel|90°|45°|knie", "Bogen"),
    (r"kreuz|\bcross\b", "Kreuzstück"),
    (r"\bt-?st|\btee\b|abzweig|junction|t-stück", "T-Stück / Abzweig"),
    (r"reduz|redukt|reducer|transition|übergang|uebergang", "Reduzierung / Übergang"),
    (r"muffe|coupling|kupplung|nippel|verbinder|connector|grooved|rillen|union|verschraub", "Verbinder / Muffe / Kupplung"),
    (r"kappe|\bcap\b|stopfen|plug|blind|endkappe", "Kappe / Stopfen"),
    (r"flansch|flange", "Flansch"),
    (r"sattel|saddle|anbohr|outlet|stutzen", "Anbohrschelle / Stutzen"),
];
const ACC_HINTS: &[(&str, &str)] = &[
    (r"alarm.?ventil|alarm.?valve|nassalarm|trockenalarm|vorgesteuert|deluge|ventilstation|alarmventilstation", "Alarmventilstation"),
    (r"einspeis|breeching|fire.?department.?connection|\bfdc\b", "Einspeisung (Feuerwehr)"),
    (r"wandhydrant|hose.?reel|schlauchhaspel|hydrantenschrank|löschwasserschrank|wh-schrank", "Wandhydrant"),
    (r"überflurhydrant|ueberflurhydrant|unterflurhydrant|außenhydrant|aussenhydrant|fire.?hydrant|\bhydrant", "Hydrant (außen)"),
    (r"schlauchanschluss|entnahme|landing.?valve|[bc]-?anschluss|storz", "Schlauchanschlussventil / Entnahmestelle"),
    (r"sprinkler|\bspk\b|düse|duese|nozzle|sprinkler.?head", "Sprinkler"),
    (r"strömungs|stroemungs|flow.?switch|flow.?detector|flow.?sensor", "Strömungsmelder"),
    (r"manometer|pressure.?gauge|druckschalter|pressure.?switch|druckmess", "Manometer / Druckschalter"),
    (r"prüf|pruef|test|entleer|drain|spül", "Prüf-/Entleerungsventil"),
    (r"rückschlag|rueckschlag|check.?valve|rückfluss", "Rückschlagventil"),
    (r"absperr|schieber|gate|butterfly|klappe|kugelhahn|ball.?valve|isolat", "Absperrarmatur"),
    (r"be-?\s?und\s?entlüft|entlüft|entlueft|air.?vent|schnellentl|belüft", "Be-/Entlüftung"),
    (r"pumpe|pump", "Pumpe"),
    (r"behälter|tank|vorrat", "Behälter"),
    (r"halter|hanger|abhäng|abhaeng|befestig|support|schelle|clamp|gewindestange|konsole", "Befestigung"),
    (r"schlauch|hose", "Wandhydrant"),
];

/// Sistemas (ordem = prioridade na atribuição por nome de sistema/workset)
pub const ANLAGEN: &[&str] = &["Sprinkler", "Hydranten", "Trockensteigleitung"];
pub const ANLAGE_NONE: &str = "Nicht zugeordnet";

fn classify_anlage(systems: &[String], worksets: &[String], kind: &str, text: &str) -> String {
    static R: OnceLock<[Regex; 3]> = OnceLock::new();
    let [tsl, hyd, spk] = R.get_or_init(|| [
        Regex::new(r"(?i)trocken.?steig|\btsl\b|dry.?riser|trockenleitung|steigleitung.?trocken").unwrap(),
        Regex::new(r"(?i)hydrant|nass.?steig|steigleitung.?nass|\bwh\b|\bwhyd|löschwasser|loeschwasser|standpipe|hose|feuerlösch.?leitung").unwrap(),
        Regex::new(r"(?i)sprinkler|\bspk|\bspr\b|\bspr_|sprink|esfr").unwrap(),
    ]);
    // 1) Nomes de sistema/workset
    for names in [systems, worksets] {
        let t = names.join(" | ");
        if t.is_empty() { continue; }
        if tsl.is_match(&t) { return "Trockensteigleitung".into(); }
        if hyd.is_match(&t) { return "Hydranten".into(); }
        if spk.is_match(&t) { return "Sprinkler".into(); }
    }
    // 2) Tipo de componente / nome
    match kind {
        "Sprinkler" | "Alarmventilstation" | "Strömungsmelder" => return "Sprinkler".into(),
        "Wandhydrant" | "Hydrant (außen)" => return "Hydranten".into(),
        _ => {}
    }
    if tsl.is_match(text) { return "Trockensteigleitung".into(); }
    if hyd.is_match(text) { return "Hydranten".into(); }
    if spk.is_match(text) { return "Sprinkler".into(); }
    ANLAGE_NONE.into()
}

fn hints(table: &[(&str, &str)], text: &str) -> Option<String> {
    static CACHE: OnceLock<std::sync::Mutex<HashMap<String, Regex>>> = OnceLock::new();
    let c = CACHE.get_or_init(Default::default);
    let mut c = c.lock().unwrap();
    for (rx, label) in table {
        let r = c.entry(rx.to_string()).or_insert_with(|| Regex::new(&format!("(?i){rx}")).unwrap());
        if r.is_match(text) {
            return Some(label.to_string());
        }
    }
    None
}

fn fitting_kind(pre: &str, text: &str) -> String {
    if let Some((_, de)) = FITTING_DE.iter().find(|(k, _)| *k == pre) {
        return de.to_string();
    }
    hints(FITTING_HINTS, text).unwrap_or_else(|| "sonstiges Formstück".into())
}

fn accessory_kind(class: &str, pre: &str, text: &str) -> String {
    if let Some(h) = hints(ACC_HINTS, text) {
        return h;
    }
    match class {
        "IFCFIRESUPPRESSIONTERMINAL" => match pre {
            "SPRINKLER" | "SPRINKLERDEFLECTOR" => "Sprinkler".into(),
            "BREECHINGINLET" => "Einspeisung (Feuerwehr)".into(),
            "FIREHYDRANT" => "Hydrant (außen)".into(),
            "HOSEREEL" => "Wandhydrant".into(),
            _ => "Löschwasser-Endgerät".into(),
        },
        "IFCVALVE" => match pre {
            "CHECK" => "Rückschlagventil".into(),
            "ISOLATING" | "GASTAP" | "STOPCOCK" => "Absperrarmatur".into(),
            "DRAWOFFCOCK" | "FAUCET" => "Prüf-/Entleerungsventil".into(),
            "" | "NOTDEFINED" | "USERDEFINED" => "Ventil".into(),
            p => format!("Ventil ({p})"),
        },
        "IFCSENSOR" if pre == "FLOWSENSOR" => "Strömungsmelder".into(),
        "IFCFLOWINSTRUMENT" if pre == "PRESSUREGAUGE" => "Manometer / Druckschalter".into(),
        "IFCDISCRETEACCESSORY" | "IFCMECHANICALFASTENER" | "IFCFASTENER" => "Befestigung".into(),
        "IFCPUMP" => "Pumpe".into(),
        "IFCTANK" => "Behälter".into(),
        c => pretty_class(c).trim_start_matches("Ifc").to_string(),
    }
}

// ------------------------------------------------------------------ Geometria
#[derive(Default)]
pub struct ElemGeom {
    pub from_mesh: bool,
    pub segments: Vec<(V3, V3)>,
    pub point: Option<V3>,
    pub outer_diameter_mm: Option<f64>,
    pub mesh_axis: Option<(V3, V3)>,
    pub mesh_radius_mm: f64,
}

/// Eixo principal de uma nuvem de pontos: (centro, eixo, meio comprimento, raio máx.)
fn principal_extent(pts: &[V3]) -> (V3, V3, f64, f64) {
    let n = pts.len() as f64;
    let mut c = V3::default();
    for p in pts { c = c.add(p); }
    let c = c.scale(1.0 / n);
    let mut m = [[0.0f64; 3]; 3];
    for p in pts {
        let d = [p.x - c.x, p.y - c.y, p.z - c.z];
        for i in 0..3 { for j in 0..3 { m[i][j] += d[i] * d[j]; } }
    }
    let mut v = V3::new(1.0, 0.7, 0.3).norm();
    for _ in 0..40 {
        let w = V3::new(
            m[0][0] * v.x + m[0][1] * v.y + m[0][2] * v.z,
            m[1][0] * v.x + m[1][1] * v.y + m[1][2] * v.z,
            m[2][0] * v.x + m[2][1] * v.y + m[2][2] * v.z,
        );
        if w.len() < 1e-15 { break; }
        v = w.norm();
    }
    let (mut lo, mut hi, mut r) = (f64::MAX, f64::MIN, 0.0f64);
    for p in pts {
        let d = p.sub(&c);
        let t = d.dot(&v);
        lo = lo.min(t);
        hi = hi.max(t);
        r = r.max(d.sub(&v.scale(t)).len());
    }
    // simétrico em torno do centro da extensão
    let mid = c.add(&v.scale((lo + hi) / 2.0));
    (mid, v, (hi - lo) / 2.0, r)
}

pub(crate) struct GeomCtx<'a> {
    pub(crate) step: &'a StepFile,
    pub(crate) scale: f64,
    pub(crate) missing: Option<&'a mut HashSet<u32>>,
    pub(crate) placements: HashMap<u32, Mat4>,
}

impl<'a> GeomCtx<'a> {
    pub(crate) fn new(step: &'a StepFile, scale: f64, missing: Option<&'a mut HashSet<u32>>) -> Self {
        GeomCtx { step, scale, missing, placements: HashMap::new() }
    }
    pub(crate) fn get(&mut self, id: u32) -> Option<Entity> {
        match self.step.get(id) {
            Some(e) => Some(e),
            None => {
                if let Some(m) = self.missing.as_deref_mut() { m.insert(id); }
                None
            }
        }
    }
    pub(crate) fn point(&mut self, id: Option<u32>) -> Option<V3> {
        let e = self.get(id?)?;
        let c = e.arg(0).as_list();
        Some(V3::new(c.first()?.as_f64()?, c.get(1).and_then(|x| x.as_f64()).unwrap_or(0.0), c.get(2).and_then(|x| x.as_f64()).unwrap_or(0.0)))
    }
    pub(crate) fn dir(&mut self, id: Option<u32>) -> Option<V3> {
        let e = self.get(id?)?;
        let c = e.arg(0).as_list();
        Some(V3::new(c.first()?.as_f64()?, c.get(1).and_then(|x| x.as_f64()).unwrap_or(0.0), c.get(2).and_then(|x| x.as_f64()).unwrap_or(0.0)))
    }
    pub(crate) fn axis2(&mut self, id: Option<u32>) -> Mat4 {
        let Some(id) = id else { return Mat4::identity() };
        let Some(e) = self.get(id) else { return Mat4::identity() };
        let loc = self.point(e.rf(0)).unwrap_or_default();
        if e.ty == "IFCAXIS2PLACEMENT2D" {
            let x = self.dir(e.rf(1)).unwrap_or(V3::new(1.0, 0.0, 0.0));
            return Mat4::from_axes(loc, V3::new(0.0, 0.0, 1.0), x);
        }
        let z = self.dir(e.rf(1)).unwrap_or(V3::new(0.0, 0.0, 1.0));
        let x = self.dir(e.rf(2)).unwrap_or(V3::new(1.0, 0.0, 0.0));
        Mat4::from_axes(loc, z, x)
    }
    pub(crate) fn placement(&mut self, id: Option<u32>, depth: u8) -> Mat4 {
        let Some(id) = id else { return Mat4::identity() };
        if let Some(m) = self.placements.get(&id) { return *m; }
        let Some(e) = self.get(id) else { return Mat4::identity() };
        let m = if e.ty == "IFCLOCALPLACEMENT" && depth < 32 {
            let parent = self.placement(e.rf(0), depth + 1);
            let local = self.axis2(e.rf(1));
            parent.mul(&local)
        } else {
            Mat4::identity()
        };
        if self.missing.is_none() { self.placements.insert(id, m); }
        m
    }
    pub(crate) fn cto(&mut self, id: Option<u32>) -> Mat4 {
        let Some(e) = id.and_then(|i| self.get(i)) else { return Mat4::identity() };
        let x = self.dir(e.rf(0)).unwrap_or(V3::new(1.0, 0.0, 0.0));
        let o = self.point(e.rf(2)).unwrap_or_default();
        let s = e.f64(3).unwrap_or(1.0);
        let z = if e.args.len() > 4 { self.dir(e.rf(4)) } else { None }.unwrap_or(V3::new(0.0, 0.0, 1.0));
        Mat4::from_axes(o, z, x).scaled(s)
    }

    pub(crate) fn element_geometry(&mut self, placement: Option<u32>, rep: Option<u32>) -> ElemGeom {
        let world = self.placement(placement, 0);
        let mut g = ElemGeom::default();
        let origin = world.apply(V3::default());
        g.point = Some(origin.scale(self.scale));
        let Some(pds) = rep.and_then(|r| self.get(r)) else { return g };
        let reps = pds.arg(2).refs();
        // preferir "Body", usar "Axis" como eixo
        let mut items: Vec<(u32, Mat4)> = vec![];
        let mut axis_items: Vec<(u32, Mat4)> = vec![];
        for r in reps {
            let Some(sr) = self.get(r) else { continue };
            let ident = sr.str(1).unwrap_or("");
            let list: Vec<(u32, Mat4)> = sr.arg(3).refs().into_iter().map(|i| (i, world)).collect();
            match ident {
                "Axis" => axis_items.extend(list),
                "Body" | "" | "Facetation" => items.extend(list),
                _ => {}
            }
        }
        let mut stack = if axis_items.is_empty() { items } else { axis_items };
        let mut guard = 0;
        let mut mesh: Vec<V3> = Vec::new();
        while let Some((it, m)) = stack.pop() {
            guard += 1;
            if guard > 256 { break; }
            let Some(x) = self.get(it) else { continue };
            match x.ty.as_str() {
                "IFCEXTRUDEDAREASOLID" | "IFCEXTRUDEDAREASOLIDTAPERED" => {
                    let pos = if x.rf(1).is_some() { self.axis2(x.rf(1)) } else { Mat4::identity() };
                    let d = self.dir(x.rf(2)).unwrap_or(V3::new(0.0, 0.0, 1.0)).norm();
                    let depth = x.f64(3).unwrap_or(0.0);
                    // Centro do perfil
                    let mut c = V3::default();
                    if let Some(p) = x.rf(0).and_then(|p| self.get(p)) {
                        if p.ty.starts_with("IFCCIRCLE") {
                            if let Some(r) = p.f64(3) {
                                let od = 2.0 * r * self.scale * 1000.0;
                                g.outer_diameter_mm = Some(g.outer_diameter_mm.map_or(od, |o: f64| o.max(od)));
                            }
                            if p.rf(2).is_some() {
                                let pp = self.axis2(p.rf(2));
                                c = pp.apply(V3::default());
                            }
                        }
                    }
                    let mm = m.mul(&pos);
                    let a = mm.apply(c).scale(self.scale);
                    let b = mm.apply(c.add(&d.scale(depth))).scale(self.scale);
                    g.segments.push((a, b));
                }
                "IFCSWEPTDISKSOLID" | "IFCSWEPTDISKSOLIDPOLYGONAL" => {
                    if let Some(r) = x.f64(1) {
                        g.outer_diameter_mm = Some(2.0 * r * self.scale * 1000.0);
                    }
                    let pts = self.curve_points(x.rf(0));
                    for w in pts.windows(2) {
                        g.segments.push((m.apply(w[0]).scale(self.scale), m.apply(w[1]).scale(self.scale)));
                    }
                }
                "IFCPOLYLINE" | "IFCINDEXEDPOLYCURVE" => {
                    let pts = self.curve_points(Some(it));
                    for w in pts.windows(2) {
                        g.segments.push((m.apply(w[0]).scale(self.scale), m.apply(w[1]).scale(self.scale)));
                    }
                }
                "IFCMAPPEDITEM" => {
                    let Some(src) = x.rf(0).and_then(|s| self.get(s)) else { continue };
                    let origin = self.axis2(src.rf(0));
                    let target = self.cto(x.rf(1));
                    let t = m.mul(&target).mul(&origin.inverse_rigid());
                    if let Some(rr) = src.rf(1).and_then(|r| self.get(r)) {
                        for i in rr.arg(3).refs() { stack.push((i, t)); }
                    }
                }
                "IFCBOOLEANCLIPPINGRESULT" | "IFCBOOLEANRESULT" => {
                    if let Some(f) = x.rf(1) { stack.push((f, m)); }
                }
                "IFCSHELLBASEDSURFACEMODEL" | "IFCFACEBASEDSURFACEMODEL" | "IFCFACETEDBREP" | "IFCFACETEDBREPWITHVOIDS"
                | "IFCTRIANGULATEDFACESET" | "IFCPOLYGONALFACESET" => {
                    let before = mesh.len();
                    self.mesh_points(&x, &mut mesh);
                    for p in &mut mesh[before..] { *p = m.apply(*p).scale(self.scale); }
                }
                _ => {}
            }
        }
        if g.segments.is_empty() && mesh.len() >= 3 {
            g.from_mesh = true;
            let (c, axis, half, radius) = principal_extent(&mesh);
            g.point = Some(c);
            g.mesh_axis = Some((c.sub(&axis.scale(half)), c.add(&axis.scale(half))));
            g.mesh_radius_mm = radius * 1000.0;
        } else if let Some((a, b)) = g.segments.first() {
            g.point = Some(a.add(b).scale(0.5));
        }
        g
    }

    /// Todos os vértices de um modelo de superfície/malha (coordenadas locais).
    pub(crate) fn mesh_points(&mut self, x: &Entity, out: &mut Vec<V3>) {
        const MAX: usize = 50_000;
        match x.ty.as_str() {
            "IFCTRIANGULATEDFACESET" | "IFCPOLYGONALFACESET" => {
                if let Some(pl) = x.rf(0).and_then(|p| self.get(p)) {
                    for t in pl.arg(0).as_list().iter().take(MAX) {
                        let l = t.as_list();
                        out.push(V3::new(l.first().and_then(|v| v.as_f64()).unwrap_or(0.0), l.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0), l.get(2).and_then(|v| v.as_f64()).unwrap_or(0.0)));
                    }
                }
            }
            _ => {
                // Cascas -> faces -> contornos -> polígonos -> pontos
                let mut seen: HashSet<u32> = HashSet::new();
                let mut shells: Vec<u32> = match x.ty.as_str() {
                    "IFCFACETEDBREP" | "IFCFACETEDBREPWITHVOIDS" => x.rf(0).into_iter().collect(),
                    _ => x.arg(0).refs(),
                };
                while let Some(sh) = shells.pop() {
                    let Some(she) = self.get(sh) else { continue };
                    for f in she.arg(0).refs() {
                        let Some(fe) = self.get(f) else { continue };
                        for b in fe.arg(0).refs() {
                            let Some(be) = self.get(b) else { continue };
                            let Some(lp) = be.rf(0).and_then(|l| self.get(l)) else { continue };
                            for p in lp.arg(0).refs() {
                                if out.len() >= MAX { return; }
                                if seen.insert(p) {
                                    if let Some(v) = self.point(Some(p)) { out.push(v); }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    pub(crate) fn curve_points(&mut self, id: Option<u32>) -> Vec<V3> {
        let Some(c) = id.and_then(|i| self.get(i)) else { return vec![] };
        match c.ty.as_str() {
            "IFCPOLYLINE" => {
                let refs = c.arg(0).refs();
                refs.into_iter().filter_map(|p| self.point(Some(p))).collect()
            }
            "IFCINDEXEDPOLYCURVE" => {
                let Some(pl) = c.rf(0).and_then(|p| self.get(p)) else { return vec![] };
                pl.arg(0).as_list().iter().map(|t| {
                    let l = t.as_list();
                    V3::new(l.first().and_then(|v| v.as_f64()).unwrap_or(0.0), l.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0), l.get(2).and_then(|v| v.as_f64()).unwrap_or(0.0))
                }).collect()
            }
            "IFCTRIMMEDCURVE" => self.curve_points(c.rf(0)),
            "IFCCIRCLE" => {
                let m = self.axis2(c.rf(0));
                let r = c.f64(1).unwrap_or(0.0);
                (0..=24).map(|k| {
                    let a = k as f64 / 24.0 * std::f64::consts::TAU;
                    m.apply(V3::new(r * a.cos(), r * a.sin(), 0.0))
                }).collect()
            }
            "IFCCOMPOSITECURVE" => {
                let mut out = vec![];
                for s in c.arg(0).refs() {
                    if let Some(seg) = self.get(s) { out.extend(self.curve_points(seg.rf(2))); }
                }
                out
            }
            _ => vec![],
        }
    }
}

/// Geometria compacta para a exibição.
#[derive(Debug, Default, Serialize)]
pub struct Geometry {
    /// Eixos de tubos: x1,y1,z1,x2,y2,z2 (metros, deslocados por `center`)
    pub lines: Vec<f32>,
    /// Índice no array de elementos por segmento de linha
    pub line_elem: Vec<u32>,
    /// Pontos (conexões/acessórios): x,y,z
    pub points: Vec<f32>,
    pub point_elem: Vec<u32>,
    pub center: [f64; 3],
    pub bbox_min: [f32; 3],
    pub bbox_max: [f32; 3],
    #[serde(skip)]
    raw_lines: Vec<(V3, V3, u32)>,
    #[serde(skip)]
    raw_points: Vec<(V3, u32)>,
}

impl Geometry {
    fn push(&mut self, idx: usize, e: &Element, g: &ElemGeom) {
        if e.cat == Cat::Rohr && !g.segments.is_empty() {
            for (a, b) in &g.segments {
                self.raw_lines.push((*a, *b, idx as u32));
            }
        } else if let Some(p) = g.point {
            self.raw_points.push((p, idx as u32));
        }
    }
    fn finish(&mut self) {
        let n = (self.raw_lines.len() * 2 + self.raw_points.len()).max(1) as f64;
        let mut c = V3::default();
        for (a, b, _) in &self.raw_lines { c = c.add(a).add(b); }
        for (p, _) in &self.raw_points { c = c.add(p); }
        let c = c.scale(1.0 / n);
        self.center = [c.x, c.y, c.z];
        let mut mn = [f32::MAX; 3];
        let mut mx = [f32::MIN; 3];
        let mut upd = |v: &V3| {
            let a = [(v.x - c.x) as f32, (v.y - c.y) as f32, (v.z - c.z) as f32];
            for k in 0..3 { mn[k] = mn[k].min(a[k]); mx[k] = mx[k].max(a[k]); }
            a
        };
        for (a, b, i) in std::mem::take(&mut self.raw_lines) {
            self.lines.extend(upd(&a));
            self.lines.extend(upd(&b));
            self.line_elem.push(i);
        }
        for (p, i) in std::mem::take(&mut self.raw_points) {
            self.points.extend(upd(&p));
            self.point_elem.push(i);
        }
        if mn[0] == f32::MAX { mn = [0.0; 3]; mx = [0.0; 3]; }
        self.bbox_min = mn;
        self.bbox_max = mx;
    }
}

// ------------------------------------------------------------------ Levantamento de quantitativos
#[derive(Debug, Serialize, Clone)]
pub struct Row {
    pub kind: String,
    pub dn: String,
    pub dn_sort: f64,
    pub material: String,
    pub type_name: String,
    pub count: usize,
    pub length_m: f64,
    pub elements: Vec<u32>,
}

#[derive(Debug, Serialize, Clone)]
pub struct Note {
    pub kind: String,
    pub count: usize,
    pub elements: Vec<u32>,
}

#[derive(Debug, Serialize, Clone)]
pub struct Takeoff {
    /// "Alle" ou um sistema
    pub anlage: String,
    pub hydrant_count: usize,
    pub outlet_count: usize,
    pub feed_count: usize,
    pub pipes: Vec<Row>,
    pub insulation: Vec<Row>,
    pub insulated_m: f64,
    pub insulation_in_model: bool,
    pub fittings: Vec<Row>,
    pub accessories: Vec<Row>,
    pub notes: Vec<Note>,
    pub total_pipe_m: f64,
    pub pipe_count: usize,
    pub fitting_count: usize,
    pub accessory_count: usize,
    pub sprinkler_count: usize,
    pub pipe_by_dn: Vec<(String, f64)>,
    /// 0 = verde, 1 = amarelo, 2 = vermelho
    pub quality: u8,
}

impl Takeoff {
    pub fn build(all: &[Element], sel: &[usize], anlage: Option<&str>) -> Takeoff {
        let sel: Vec<usize> = sel.iter().copied().filter(|&i| anlage.map_or(true, |a| all[i].anlage == a)).collect();
        let sel = &sel[..];
        let (mut hyd, mut outl, mut feed) = (0usize, 0usize, 0usize);
        for &i in sel {
            match all[i].kind.as_str() {
                "Wandhydrant" | "Hydrant (außen)" => hyd += 1,
                "Schlauchanschlussventil / Entnahmestelle" => outl += 1,
                "Einspeisung (Feuerwehr)" => feed += 1,
                _ => {}
            }
        }
        let mut pipes: BTreeMap<(i64, String, String), Row> = BTreeMap::new();
        let mut fit: BTreeMap<(String, i64, String, String), Row> = BTreeMap::new();
        let mut acc: BTreeMap<(String, i64, String, String), Row> = BTreeMap::new();
        let mut notes: BTreeMap<String, Vec<u32>> = BTreeMap::new();
        let mut ins: BTreeMap<(u8, String, i64, i64), Row> = BTreeMap::new();
        let mut insulated_m = 0.0;
        let mut uninsulated: Vec<u32> = vec![];
        let mut by_dn: BTreeMap<i64, (String, f64)> = BTreeMap::new();
        let (mut total, mut pc, mut fc, mut ac, mut spk) = (0.0, 0, 0, 0, 0);
        for &i in sel {
            let e = &all[i];
            let dn_lbl = if e.dn_label.is_empty() { "ohne DN".to_string() } else { e.dn_label.clone() };
            let dn_key = e.dn.map(|d| (d * 10.0).round() as i64).unwrap_or(-1);
            let mat = if e.material.is_empty() { "ohne Material".to_string() } else { e.material.clone() };
            if e.systems.is_empty() { notes.entry("Keinem System zugeordnet".into()).or_default().push(i as u32); }
            match e.cat {
                Cat::Rohr => {
                    pc += 1;
                    let l = e.length_m.unwrap_or(0.0);
                    total += l;
                    let r = pipes.entry((dn_key, mat.clone(), String::new())).or_insert_with(|| Row {
                        kind: "Rohr".into(), dn: dn_lbl.clone(), dn_sort: e.dn.unwrap_or(-1.0), material: mat.clone(),
                        type_name: String::new(), count: 0, length_m: 0.0, elements: vec![],
                    });
                    r.count += 1;
                    r.length_m += l;
                    r.elements.push(i as u32);
                    let d = by_dn.entry(dn_key).or_insert((dn_lbl.clone(), 0.0));
                    d.1 += l;
                    {
                        let (ord, kind, th, thk) = match &e.insulation {
                            Some(x) => (0u8, x.kind.clone(), x.label(), x.thickness_mm.map(|t| (t * 10.0).round() as i64).unwrap_or(-1)),
                            None => (1u8, "ohne Dämmung".to_string(), "–".to_string(), -1),
                        };
                        if e.insulation.is_some() { insulated_m += l; } else { uninsulated.push(i as u32); }
                        let r = ins.entry((ord, kind.clone(), thk, dn_key)).or_insert_with(|| Row {
                            kind, dn: dn_lbl.clone(), dn_sort: e.dn.unwrap_or(-1.0), material: th,
                            type_name: String::new(), count: 0, length_m: 0.0, elements: vec![],
                        });
                        r.count += 1;
                        r.length_m += l;
                        r.elements.push(i as u32);
                    }
                    if e.length_m.is_none() { notes.entry("Rohr ohne Länge".into()).or_default().push(i as u32); }
                    if e.dn.is_none() { notes.entry("Rohr ohne Nennweite".into()).or_default().push(i as u32); }
                    if e.material.is_empty() { notes.entry("Rohr ohne Material".into()).or_default().push(i as u32); }
                }
                Cat::Formstueck => {
                    fc += 1;
                    let r = fit.entry((e.kind.clone(), dn_key, mat.clone(), String::new())).or_insert_with(|| Row {
                        kind: e.kind.clone(), dn: dn_lbl.clone(), dn_sort: e.dn.unwrap_or(-1.0), material: mat.clone(),
                        type_name: String::new(), count: 0, length_m: 0.0, elements: vec![],
                    });
                    r.count += 1;
                    r.elements.push(i as u32);
                    if e.dn.is_none() { notes.entry("Formstück ohne Nennweite".into()).or_default().push(i as u32); }
                    if e.kind == "sonstiges Formstück" { notes.entry("Formstück-Art nicht erkannt".into()).or_default().push(i as u32); }
                }
                Cat::Zubehoer => {
                    ac += 1;
                    if e.kind == "Sprinkler" { spk += 1; }
                    let tn = if e.type_name.is_empty() { e.name.clone() } else { e.type_name.clone() };
                    let r = acc.entry((e.kind.clone(), dn_key, tn.clone(), String::new())).or_insert_with(|| Row {
                        kind: e.kind.clone(), dn: if e.dn.is_some() { dn_lbl.clone() } else { "–".into() }, dn_sort: e.dn.unwrap_or(-1.0),
                        material: mat.clone(), type_name: tn.clone(), count: 0, length_m: 0.0, elements: vec![],
                    });
                    r.count += 1;
                    r.elements.push(i as u32);
                }
            }
        }
        let insulation_in_model = sel.iter().any(|&i| all[i].insulation.is_some());
        if insulation_in_model && !uninsulated.is_empty() {
            notes.insert("Rohr ohne Dämmung".into(), uninsulated);
        }
        let notes: Vec<Note> = notes.into_iter().map(|(kind, elements)| Note { kind, count: elements.len(), elements }).collect();
        let critical = notes.iter().filter(|n| n.kind.starts_with("Rohr ohne Länge") || n.kind.starts_with("Rohr ohne Nennweite")).map(|n| n.count).sum::<usize>();
        let quality = if notes.is_empty() { 0 } else if pc > 0 && critical * 10 > pc { 2 } else { 1 };
        let mut pipes: Vec<Row> = pipes.into_values().collect();
        pipes.sort_by(|a, b| a.dn_sort.partial_cmp(&b.dn_sort).unwrap().then(a.material.cmp(&b.material)));
        let mut fittings: Vec<Row> = fit.into_values().collect();
        fittings.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.dn_sort.partial_cmp(&b.dn_sort).unwrap()));
        let mut accessories: Vec<Row> = acc.into_values().collect();
        accessories.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.dn_sort.partial_cmp(&b.dn_sort).unwrap()).then(a.type_name.cmp(&b.type_name)));
        let insulation: Vec<Row> = ins.into_values().collect();
        Takeoff {
            anlage: anlage.unwrap_or("Alle").to_string(),
            hydrant_count: hyd,
            outlet_count: outl,
            feed_count: feed,
            pipes,
            insulation,
            insulated_m,
            insulation_in_model,
            fittings,
            accessories,
            notes,
            total_pipe_m: total,
            pipe_count: pc,
            fitting_count: fc,
            accessory_count: ac,
            sprinkler_count: spk,
            pipe_by_dn: by_dn.into_values().collect(),
            quality,
        }
    }
}
