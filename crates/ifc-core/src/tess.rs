//! Tesselação (malhas de triângulos) para a exibição sólida („Alto“) e modelos de referência.
//! Suporta: extrusões (retângulo, círculo, elipse, perfis, contornos arbitrários),
//! modelos de superfície/casca, B-reps facetados, face sets de triângulos/polígonos,
//! tubos SweptDisk, representações mapeadas, sólidos booleanos (primeiro operando).
//! Não suportado (ignorado): NURBS/Advanced B-reps, sólidos de revolução.

use crate::geom::{Mat4, V3};
use crate::model::{GeomCtx, Model};
use crate::step::{Progress, StepError, StepFile};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::AtomicBool;

/// Malha de triângulos de um grupo de cores. `ranges`: (primeiro triângulo, índice do elemento).
#[derive(Default)]
pub struct MeshBuf {
    pub pos: Vec<f32>,
    pub idx: Vec<u32>,
    pub ranges: Vec<(u32, u32)>,
}

impl MeshBuf {
    fn begin(&mut self, elem: u32) {
        let tri = (self.idx.len() / 3) as u32;
        match self.ranges.last_mut() {
            Some(r) if r.0 == tri => r.1 = elem,
            Some(r) if r.1 == elem => {}
            _ => self.ranges.push((tri, elem)),
        }
    }
}

pub type Groups = BTreeMap<u32, MeshBuf>;

/// Formato binário para a interface (little endian):
/// u32 grupos; por grupo: u32 classe, u32 nPos(floats), u32 nIdx, u32 nRanges, f32[nPos], u32[nIdx], u32[2*nRanges]
pub fn encode(groups: &Groups) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend((groups.len() as u32).to_le_bytes());
    for (cls, g) in groups {
        for v in [*cls, g.pos.len() as u32, g.idx.len() as u32, g.ranges.len() as u32] {
            out.extend(v.to_le_bytes());
        }
        for f in &g.pos { out.extend(f.to_le_bytes()); }
        for i in &g.idx { out.extend(i.to_le_bytes()); }
        for (a, b) in &g.ranges { out.extend(a.to_le_bytes()); out.extend(b.to_le_bytes()); }
    }
    out
}

struct Ctx<'o> {
    out: &'o mut MeshBuf,
    center: V3,
    scale: f64,
    segs: usize,
}

impl<'o> Ctx<'o> {
    fn v(&mut self, p: V3) -> u32 {
        let i = (self.out.pos.len() / 3) as u32;
        let q = p.scale(self.scale);
        self.out.pos.extend([(q.x - self.center.x) as f32, (q.y - self.center.y) as f32, (q.z - self.center.z) as f32]);
        i
    }
    fn tri(&mut self, a: u32, b: u32, c: u32) { self.out.idx.extend([a, b, c]); }
    fn poly_fan(&mut self, pts: &[V3]) {
        if pts.len() < 3 { return; }
        let ids: Vec<u32> = pts.iter().map(|p| self.v(*p)).collect();
        for k in 1..ids.len() - 1 { self.tri(ids[0], ids[k], ids[k + 1]); }
    }
}

impl<'a> GeomCtx<'a> {
    /// Tessela um componente. Retorna false se nada foi gerado.
    pub(crate) fn tess_element(&mut self, placement: Option<u32>, rep: Option<u32>, elem: u32, out: &mut MeshBuf, center: V3, segs: usize) -> bool {
        let world = self.placement(placement, 0);
        let Some(pds) = rep.and_then(|r| self.get(r)) else { return false };
        let mut items: Vec<u32> = vec![];
        let mut fallback: Vec<u32> = vec![];
        for r in pds.arg(2).refs() {
            let Some(sr) = self.get(r) else { continue };
            match sr.str(1).unwrap_or("") {
                "Body" | "Facetation" => items.extend(sr.arg(3).refs()),
                "Axis" | "FootPrint" | "Box" | "Annotation" | "Clearance" | "Reference" | "Profile" | "Lighting" => {}
                _ => fallback.extend(sr.arg(3).refs()),
            }
        }
        if items.is_empty() { items = fallback; }
        out.begin(elem);
        let before = out.idx.len();
        let scale = self.scale;
        let mut c = Ctx { out, center, scale, segs };
        let mut budget = 400usize;
        for it in items { self.tess_item(it, world, &mut c, &mut budget, 0); }
        c.out.idx.len() > before
    }

    fn tess_item(&mut self, id: u32, m: Mat4, c: &mut Ctx, budget: &mut usize, depth: u8) {
        if *budget == 0 || depth > 12 { return; }
        *budget -= 1;
        let Some(x) = self.get(id) else { return };
        match x.ty.as_str() {
            "IFCEXTRUDEDAREASOLID" | "IFCEXTRUDEDAREASOLIDTAPERED" => {
                let ring = self.profile_ring(x.rf(0), c.segs);
                if ring.len() < 3 { return; }
                let pos = if x.rf(1).is_some() { self.axis2(x.rf(1)) } else { Mat4::identity() };
                let d = self.dir(x.rf(2)).unwrap_or(V3::new(0.0, 0.0, 1.0)).norm().scale(x.f64(3).unwrap_or(0.0));
                let mm = m.mul(&pos);
                let bot: Vec<V3> = ring.iter().map(|p| mm.apply(V3::new(p.0, p.1, 0.0))).collect();
                let top: Vec<V3> = ring.iter().map(|p| mm.apply(V3::new(p.0 + d.x, p.1 + d.y, d.z))).collect();
                let bi: Vec<u32> = bot.iter().map(|p| c.v(*p)).collect();
                let ti: Vec<u32> = top.iter().map(|p| c.v(*p)).collect();
                let n = ring.len();
                for k in 0..n {
                    let k2 = (k + 1) % n;
                    c.tri(bi[k], bi[k2], ti[k2]);
                    c.tri(bi[k], ti[k2], ti[k]);
                }
                for (a, b, cc) in earclip(&ring) {
                    c.tri(bi[cc], bi[b], bi[a]);
                    c.tri(ti[a], ti[b], ti[cc]);
                }
            }
            "IFCSWEPTDISKSOLID" | "IFCSWEPTDISKSOLIDPOLYGONAL" => {
                let r = x.f64(1).unwrap_or(0.0);
                let pts: Vec<V3> = self.curve_points(x.rf(0)).into_iter().map(|p| m.apply(p)).collect();
                // O raio está em comprimento global (unidade do projeto) – tubo ao longo dos pontos globais
                for w in pts.windows(2) { tube(c, w[0], w[1], r * m.x.len(), c.segs); }
            }
            "IFCSHELLBASEDSURFACEMODEL" | "IFCFACEBASEDSURFACEMODEL" => {
                for sh in x.arg(0).refs() { self.tess_shell(sh, m, c); }
            }
            "IFCFACETEDBREP" | "IFCFACETEDBREPWITHVOIDS" | "IFCMANIFOLDSOLIDBREP" => {
                if let Some(sh) = x.rf(0) { self.tess_shell(sh, m, c); }
            }
            "IFCOPENSHELL" | "IFCCLOSEDSHELL" | "IFCCONNECTEDFACESET" => self.tess_shell(id, m, c),
            "IFCTRIANGULATEDFACESET" | "IFCTRIANGULATEDIRREGULARNETWORK" => {
                let coords = self.coord_list(x.rf(0));
                let pn: Vec<usize> = x.arg(4).as_list().iter().filter_map(|v| v.as_f64()).map(|v| v as usize).collect();
                let map = |i: usize| -> usize { if pn.is_empty() { i } else { pn.get(i - 1).copied().unwrap_or(1) } };
                let base = (c.out.pos.len() / 3) as u32;
                for p in &coords { c.v(m.apply(*p)); }
                for t in x.arg(3).as_list() {
                    let l: Vec<usize> = t.as_list().iter().filter_map(|v| v.as_f64()).map(|v| v as usize).collect();
                    if l.len() == 3 && l.iter().all(|&i| i >= 1) {
                        let [a, b, d] = [map(l[0]), map(l[1]), map(l[2])];
                        if a <= coords.len() && b <= coords.len() && d <= coords.len() && a >= 1 && b >= 1 && d >= 1 {
                            c.tri(base + a as u32 - 1, base + b as u32 - 1, base + d as u32 - 1);
                        }
                    }
                }
            }
            "IFCPOLYGONALFACESET" => {
                let coords = self.coord_list(x.rf(0));
                let pn: Vec<usize> = x.arg(3).as_list().iter().filter_map(|v| v.as_f64()).map(|v| v as usize).collect();
                let base = (c.out.pos.len() / 3) as u32;
                for p in &coords { c.v(m.apply(*p)); }
                for f in x.arg(2).refs() {
                    let Some(fe) = self.get(f) else { continue };
                    let l: Vec<u32> = fe.arg(0).as_list().iter().filter_map(|v| v.as_f64())
                        .map(|v| v as usize)
                        .map(|i| if pn.is_empty() { i } else { pn.get(i.saturating_sub(1)).copied().unwrap_or(1) })
                        .filter(|&i| i >= 1 && i <= coords.len())
                        .map(|i| base + i as u32 - 1).collect();
                    for k in 1..l.len().saturating_sub(1) { c.tri(l[0], l[k], l[k + 1]); }
                }
            }
            "IFCMAPPEDITEM" => {
                let Some(src) = x.rf(0).and_then(|s| self.get(s)) else { return };
                let origin = self.axis2(src.rf(0));
                let target = self.cto(x.rf(1));
                let t = m.mul(&target).mul(&origin.inverse_rigid());
                if let Some(rr) = src.rf(1).and_then(|r| self.get(r)) {
                    for i in rr.arg(3).refs() { self.tess_item(i, t, c, budget, depth + 1); }
                }
            }
            "IFCBOOLEANCLIPPINGRESULT" | "IFCBOOLEANRESULT" => {
                if let Some(f) = x.rf(1) { self.tess_item(f, m, c, budget, depth + 1); }
            }
            _ => {}
        }
    }

    fn tess_shell(&mut self, sh: u32, m: Mat4, c: &mut Ctx) {
        let Some(she) = self.get(sh) else { return };
        for f in she.arg(0).refs() {
            let Some(fe) = self.get(f) else { continue };
            // contorno externo (furos são ignorados)
            let bounds = fe.arg(0).refs();
            let outer = bounds.iter().find(|b| self.step.type_of(**b) == Some("IFCFACEOUTERBOUND")).copied().or(bounds.first().copied());
            let Some(be) = outer.and_then(|b| self.get(b)) else { continue };
            let Some(lp) = be.rf(0).and_then(|l| self.get(l)) else { continue };
            let pts: Vec<V3> = lp.arg(0).refs().into_iter().filter_map(|p| self.point(Some(p))).map(|p| m.apply(p)).collect();
            c.poly_fan(&pts);
        }
    }

    fn coord_list(&mut self, id: Option<u32>) -> Vec<V3> {
        let Some(pl) = id.and_then(|p| self.get(p)) else { return vec![] };
        pl.arg(0).as_list().iter().map(|t| {
            let l = t.as_list();
            V3::new(l.first().and_then(|v| v.as_f64()).unwrap_or(0.0), l.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0), l.get(2).and_then(|v| v.as_f64()).unwrap_or(0.0))
        }).collect()
    }

    /// Contorno do perfil (2D, sentido anti-horário não garantido).
    fn profile_ring(&mut self, id: Option<u32>, segs: usize) -> Vec<(f64, f64)> {
        let Some(p) = id.and_then(|i| self.get(i)) else { return vec![] };
        let rect = |w: f64, h: f64| vec![(-w / 2.0, -h / 2.0), (w / 2.0, -h / 2.0), (w / 2.0, h / 2.0), (-w / 2.0, h / 2.0)];
        let ellipse = |a: f64, b: f64, n: usize| (0..n).map(|k| { let t = k as f64 / n as f64 * std::f64::consts::TAU; (a * t.cos(), b * t.sin()) }).collect::<Vec<_>>();
        let local: Vec<(f64, f64)> = match p.ty.as_str() {
            "IFCRECTANGLEPROFILEDEF" | "IFCRECTANGLEHOLLOWPROFILEDEF" | "IFCROUNDEDRECTANGLEPROFILEDEF" => rect(p.f64(3).unwrap_or(0.0), p.f64(4).unwrap_or(0.0)),
            "IFCCIRCLEPROFILEDEF" | "IFCCIRCLEHOLLOWPROFILEDEF" => { let r = p.f64(3).unwrap_or(0.0); ellipse(r, r, segs) }
            "IFCELLIPSEPROFILEDEF" => ellipse(p.f64(3).unwrap_or(0.0), p.f64(4).unwrap_or(0.0), segs),
            "IFCISHAPEPROFILEDEF" | "IFCASYMMETRICISHAPEPROFILEDEF" | "IFCCSHAPEPROFILEDEF" | "IFCLSHAPEPROFILEDEF" => {
                let (w, h) = if p.ty == "IFCISHAPEPROFILEDEF" || p.ty == "IFCASYMMETRICISHAPEPROFILEDEF" { (p.f64(3).unwrap_or(0.0), p.f64(4).unwrap_or(0.0)) } else { (p.f64(4).unwrap_or(0.0), p.f64(3).unwrap_or(0.0)) };
                rect(w, h)
            }
            "IFCTSHAPEPROFILEDEF" | "IFCUSHAPEPROFILEDEF" | "IFCZSHAPEPROFILEDEF" => rect(p.f64(4).unwrap_or(0.0), p.f64(3).unwrap_or(0.0)),
            "IFCARBITRARYCLOSEDPROFILEDEF" | "IFCARBITRARYPROFILEDEFWITHVOIDS" => {
                let mut v: Vec<(f64, f64)> = self.curve_points(p.rf(2)).into_iter().map(|q| (q.x, q.y)).collect();
                if v.len() > 2 && (v[0].0 - v[v.len() - 1].0).abs() < 1e-9 && (v[0].1 - v[v.len() - 1].1).abs() < 1e-9 { v.pop(); }
                return v;
            }
            "IFCDERIVEDPROFILEDEF" => {
                let parent = self.profile_ring(p.rf(2), segs);
                let t = self.cto(p.rf(3));
                return parent.into_iter().map(|(a, b)| { let q = t.apply(V3::new(a, b, 0.0)); (q.x, q.y) }).collect();
            }
            "IFCCOMPOSITEPROFILEDEF" => return p.arg(2).refs().first().map(|f| self.profile_ring(Some(*f), segs)).unwrap_or_default(),
            _ => return vec![],
        };
        // Posição do perfil (IfcAxis2Placement2D, atributo 2)
        if p.rf(2).is_some() {
            let pm = self.axis2(p.rf(2));
            return local.into_iter().map(|(a, b)| { let q = pm.apply(V3::new(a, b, 0.0)); (q.x, q.y) }).collect();
        }
        local
    }
}

fn tube(c: &mut Ctx, a: V3, b: V3, r: f64, segs: usize) {
    let d = b.sub(&a);
    if d.len() < 1e-9 || r <= 0.0 { return; }
    let m = Mat4::from_axes(a, d, if d.norm().x.abs() < 0.9 { V3::new(1.0, 0.0, 0.0) } else { V3::new(0.0, 1.0, 0.0) });
    let n = segs.max(6);
    let ring = |c: &mut Ctx, base: V3| -> Vec<u32> {
        (0..n).map(|k| {
            let t = k as f64 / n as f64 * std::f64::consts::TAU;
            let p = base.add(&m.x.scale(r * t.cos())).add(&m.y.scale(r * t.sin()));
            c.v(p)
        }).collect()
    };
    let r0 = ring(c, a);
    let r1 = ring(c, b);
    for k in 0..n {
        let k2 = (k + 1) % n;
        c.tri(r0[k], r0[k2], r1[k2]);
        c.tri(r0[k], r1[k2], r1[k]);
    }
}

/// Corte de orelhas (ear clipping) para polígonos simples; alternativa: leque.
fn earclip(poly: &[(f64, f64)]) -> Vec<(usize, usize, usize)> {
    let n = poly.len();
    if n < 3 { return vec![]; }
    let area: f64 = (0..n).map(|i| { let (a, b) = (poly[i], poly[(i + 1) % n]); a.0 * b.1 - b.0 * a.1 }).sum();
    let mut idx: Vec<usize> = (0..n).collect();
    if area < 0.0 { idx.reverse(); }
    let cross = |o: (f64, f64), a: (f64, f64), b: (f64, f64)| (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0);
    let inside = |p: (f64, f64), a: (f64, f64), b: (f64, f64), c: (f64, f64)| cross(a, b, p) >= 0.0 && cross(b, c, p) >= 0.0 && cross(c, a, p) >= 0.0;
    let mut out = Vec::with_capacity(n - 2);
    let mut guard = 0;
    while idx.len() > 3 && guard < n * n {
        guard += 1;
        let m = idx.len();
        let mut clipped = false;
        for i in 0..m {
            let (ia, ib, ic) = (idx[(i + m - 1) % m], idx[i], idx[(i + 1) % m]);
            let (a, b, c) = (poly[ia], poly[ib], poly[ic]);
            if cross(a, b, c) <= 1e-12 { continue; }
            if idx.iter().any(|&j| j != ia && j != ib && j != ic && inside(poly[j], a, b, c)) { continue; }
            out.push((ia, ib, ic));
            idx.remove(i);
            clipped = true;
            break;
        }
        if !clipped { break; }
    }
    if idx.len() == 3 {
        out.push((idx[0], idx[1], idx[2]));
    } else if idx.len() > 3 {
        for k in 1..idx.len() - 1 { out.push((idx[0], idx[k], idx[k + 1])); }
    }
    // Volta à orientação original do contorno
    if area < 0.0 { out.into_iter().map(|(a, b, c)| (c, b, a)).collect() } else { out }
}

// ------------------------------------------------------------------ Modelo principal „Alto“
/// Classe de cor: sistema (0 sprinklers, 1 hidrantes, 2 coluna seca, 3 não atribuído) * 4 + categoria (0 tubo, 1 conexão, 2 sprinkler, 3 outros acessórios)
pub fn main_class(anlage: &str, cat: crate::model::Cat, kind: &str) -> u32 {
    let a = crate::model::ANLAGEN.iter().position(|x| *x == anlage).unwrap_or(3) as u32;
    let k = match cat {
        crate::model::Cat::Rohr => 0,
        crate::model::Cat::Formstueck => 1,
        crate::model::Cat::Zubehoer => if kind == "Sprinkler" { 2 } else { 3 },
    };
    a * 4 + k
}

/// Garante as entidades em massa necessárias (arquivos pequenos: tudo em uma passagem).
fn ensure_loaded<F: FnMut(&mut GeomCtx)>(step: &mut StepFile, scale: f64, progress: Progress, cancel: &AtomicBool, mut walk: F) -> Result<(), StepError> {
    if step.all_skipped_loaded { return Ok(()); }
    if step.file_size() < 1_500_000_000 && std::env::var("SPK_BIGFILE").is_err() {
        step.load_all_skipped(progress, cancel)?;
        return Ok(());
    }
    for _ in 0..8 {
        let mut missing = HashSet::new();
        {
            let mut g = GeomCtx::new(step, scale, Some(&mut missing));
            walk(&mut g);
        }
        if missing.is_empty() { break; }
        step.load_extra(&missing, progress, cancel)?;
    }
    Ok(())
}

impl Model {
    /// Malhas sólidas da seleção atual, agrupadas por classe de cor. `center` = ponto central da vista (m).
    pub fn tess_selection(&mut self, center: [f64; 3], segs: usize, progress: Progress, cancel: &AtomicBool) -> Result<Groups, StepError> {
        let sel: Vec<(Option<u32>, Option<u32>)> = self.selection.iter().map(|&i| (self.elements[i].placement, self.elements[i].representation)).collect();
        let scale = self.scale;
        let mut dummy = MeshBuf::default();
        ensure_loaded(&mut self.step, scale, progress, cancel, |g| {
            for (p, r) in &sel { dummy.pos.clear(); dummy.idx.clear(); g.tess_element(*p, *r, 0, &mut dummy, V3::default(), 6); }
        })?;
        let c = V3::new(center[0], center[1], center[2]);
        let mut groups = Groups::new();
        let mut g = GeomCtx::new(&self.step, scale, None);
        for &i in &self.selection {
            let e = &self.elements[i];
            let cls = main_class(&e.anlage, e.cat, &e.kind);
            let buf = groups.entry(cls).or_default();
            g.tess_element(e.placement, e.representation, i as u32, buf, c, segs);
        }
        Ok(groups)
    }
}

// ------------------------------------------------------------------ Modelos de referência
const REF_TYPES: &[(&str, u32)] = &[
    ("IFCWALL", 0), ("IFCWALLSTANDARDCASE", 0), ("IFCWALLELEMENTEDCASE", 0),
    ("IFCSLAB", 1), ("IFCSLABSTANDARDCASE", 1), ("IFCSLABELEMENTEDCASE", 1), ("IFCROOF", 1), ("IFCCOVERING", 1), ("IFCFOOTING", 1),
    ("IFCWINDOW", 2), ("IFCWINDOWSTANDARDCASE", 2), ("IFCCURTAINWALL", 2), ("IFCPLATE", 2), ("IFCPLATESTANDARDCASE", 2),
    ("IFCDOOR", 3), ("IFCDOORSTANDARDCASE", 3),
    ("IFCCOLUMN", 4), ("IFCCOLUMNSTANDARDCASE", 4), ("IFCBEAM", 4), ("IFCBEAMSTANDARDCASE", 4), ("IFCMEMBER", 4), ("IFCMEMBERSTANDARDCASE", 4), ("IFCPILE", 4),
    ("IFCSTAIR", 5), ("IFCSTAIRFLIGHT", 5), ("IFCRAMP", 5), ("IFCRAMPFLIGHT", 5), ("IFCRAILING", 5),
    ("IFCDUCTSEGMENT", 6), ("IFCDUCTFITTING", 6), ("IFCPIPESEGMENT", 6), ("IFCPIPEFITTING", 6), ("IFCFLOWSEGMENT", 6), ("IFCFLOWFITTING", 6),
    ("IFCFLOWTERMINAL", 6), ("IFCFLOWCONTROLLER", 6), ("IFCAIRTERMINAL", 6), ("IFCCABLECARRIERSEGMENT", 6), ("IFCCABLECARRIERFITTING", 6),
    ("IFCLIGHTFIXTURE", 6), ("IFCFIRESUPPRESSIONTERMINAL", 6), ("IFCVALVE", 6), ("IFCDAMPER", 6), ("IFCUNITARYEQUIPMENT", 6),
    ("IFCBUILDINGELEMENTPROXY", 7), ("IFCFURNISHINGELEMENT", 7), ("IFCFURNITURE", 7), ("IFCCHIMNEY", 7), ("IFCSHADINGDEVICE", 7),
];

#[derive(Serialize, Clone)]
pub struct RefElem {
    pub class: String,
    pub name: String,
    pub guid: String,
}

pub struct RefModel {
    pub name: String,
    pub elements: Vec<RefElem>,
}

/// Carrega IFC de referência (somente visualização, sem levantamento de quantitativos). Retorna malhas por classe.
pub fn load_reference(path: &str, center: [f64; 3], segs: usize, progress: Progress, cancel: &AtomicBool) -> Result<(RefModel, Groups), StepError> {
    let lower = path.to_lowercase();
    let real = if lower.ends_with(".ifczip") || lower.ends_with(".zip") { crate::step::unpack_ifczip(path, progress, cancel)? } else { path.to_string() };
    let mut step = StepFile::open(&real, progress, cancel)?;
    let scale = crate::model::unit_scale_pub(&step);
    let types: Vec<&str> = REF_TYPES.iter().map(|(t, _)| *t).collect();
    let cands = step.ids_of_types(&types);
    let mut items: Vec<(u32, Option<u32>, Option<u32>, u32, String, String, String)> = Vec::with_capacity(cands.len());
    for (id, ty) in &cands {
        let Some(e) = step.get(*id) else { continue };
        let cls = REF_TYPES.iter().find(|(t, _)| t == ty).map(|(_, c)| *c).unwrap_or(7);
        items.push((*id, e.rf(5), e.rf(6), cls, ty.clone(), e.str(2).unwrap_or("").to_string(), e.str(0).unwrap_or("").to_string()));
    }
    let walk_items: Vec<(Option<u32>, Option<u32>)> = items.iter().map(|x| (x.1, x.2)).collect();
    let mut dummy = MeshBuf::default();
    ensure_loaded(&mut step, scale, progress, cancel, |g| {
        for (p, r) in &walk_items { dummy.pos.clear(); dummy.idx.clear(); g.tess_element(*p, *r, 0, &mut dummy, V3::default(), 6); }
    })?;
    let c = V3::new(center[0], center[1], center[2]);
    let mut groups = Groups::new();
    let mut elements = Vec::with_capacity(items.len());
    let mut g = GeomCtx::new(&step, scale, None);
    for (k, (_, pl, rep, cls, ty, name, guid)) in items.into_iter().enumerate() {
        let buf = groups.entry(cls).or_default();
        g.tess_element(pl, rep, k as u32, buf, c, segs);
        elements.push(RefElem { class: pretty(&ty), name, guid });
    }
    let name = std::path::Path::new(path).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    Ok((RefModel { name, elements }, groups))
}

fn pretty(t: &str) -> String {
    let s = t.trim_start_matches("IFC").to_lowercase();
    let mut c = s.chars();
    match c.next() { Some(f) => format!("Ifc{}{}", f.to_uppercase(), c.as_str()), None => t.to_string() }
}
