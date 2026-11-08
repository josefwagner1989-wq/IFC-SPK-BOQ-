use std::sync::atomic::AtomicBool;
use std::time::Instant;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let c = AtomicBool::new(false);
    let np = |_: u64, _: u64| {};
    let t = Instant::now();
    let mut m = ifc_core::Model::scan(&a[1], &np, &c).unwrap();
    let f = ifc_core::Filter::default();
    m.apply_filter(&f);
    let g = m.load_geometry(&np, &c).unwrap();
    let groups = m.tess_selection(g.center, 12, &np, &c).unwrap();
    let tris: usize = groups.values().map(|b| b.idx.len() / 3).sum();
    let bytes = ifc_core::tess::encode(&groups).len();
    println!("main: {} groups, {} tris, {:.1} MB, {:.2}s", groups.len(), tris, bytes as f64 / 1e6, t.elapsed().as_secs_f64());
    if let Some(r) = a.get(2) {
        let t = Instant::now();
        let (rm, rg) = ifc_core::tess::load_reference(r, g.center, 16, &np, &c).unwrap();
        let tris: usize = rg.values().map(|b| b.idx.len() / 3).sum();
        println!("ref {}: {} elems, {} groups, {} tris, {:.2}s", rm.name, rm.elements.len(), rg.len(), tris, t.elapsed().as_secs_f64());
    }
}
