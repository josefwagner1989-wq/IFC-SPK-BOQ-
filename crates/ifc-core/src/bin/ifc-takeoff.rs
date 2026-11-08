//! Linha de comando: ifc-takeoff <arquivo.ifc> [-o quantitativos.xlsx] [--alle] [--json] [--anlage <sistema>]
use ifc_core::{excel, Filter, Model};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Aufruf: ifc-takeoff <datei.ifc> [-o mengen.xlsx] [--alle] [--json]");
        std::process::exit(2);
    }
    let path = &args[1];
    let out = args.iter().position(|a| a == "-o").and_then(|i| args.get(i + 1)).cloned();
    let all = args.iter().any(|a| a == "--alle");
    let json = args.iter().any(|a| a == "--json");
    let anlage = args.iter().position(|a| a == "--anlage").and_then(|i| args.get(i + 1)).cloned();
    let cancel = AtomicBool::new(false);
    let t0 = Instant::now();
    let prog = |a: u64, b: u64| eprint!("\rScan {:5.1} %", a as f64 * 100.0 / b.max(1) as f64);
    let mut m = match Model::scan(path, &prog, &cancel) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("\nFehler: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("\nScan: {:.2} s, {} Entitäten indiziert, {} übersprungen, {} Bauteile",
        m.summary.seconds, m.summary.entities_indexed, m.summary.entities_skipped, m.summary.elements);
    eprintln!("Systeme:");
    for s in &m.summary.systems { eprintln!("  {}{} ({})", if s.sprinkler_like { "* " } else { "  " }, s.name, s.count); }
    eprintln!("Worksets/Layer:");
    for s in m.summary.worksets.iter().take(30) { eprintln!("    {} ({})", s.name, s.count); }
    eprintln!("Geschosse:");
    for s in &m.summary.storeys { eprintln!("    {} ({})", s.name, s.count); }
    let f = if all { Filter::default() } else { m.default_filter() };
    let n = m.apply_filter(&f);
    eprintln!("Auswahl: {n} Bauteile");
    let t1 = Instant::now();
    let g = m.load_geometry(&|a, b| eprint!("\rGeometrie {:5.1} %", a as f64 * 100.0 / b.max(1) as f64), &cancel).unwrap();
    eprintln!("\nGeometrie: {:.2} s, {} Achsen, {} Punkte", t1.elapsed().as_secs_f64(), g.line_elem.len(), g.point_elem.len());
    eprintln!("Anlagen:");
    for a in m.anlagen() { eprintln!("    {} ({})", a.name, a.count); }
    let t = m.takeoff_for(anlage.as_deref());
    if json {
        println!("{}", serde_json::to_string_pretty(&t).unwrap());
    } else {
        println!("Rohre gesamt: {:.2} m in {} Segmenten", t.total_pipe_m, t.pipe_count);
        for r in &t.pipes { println!("  {:10} {:28} {:5} Stk {:10.2} m", r.dn, r.material, r.count, r.length_m); }
        println!("Formstücke: {}", t.fitting_count);
        for r in &t.fittings { println!("  {:30} {:10} {:20} {:5}", r.kind, r.dn, r.material, r.count); }
        println!("[{}] Armaturen & Zubehör: {} (Sprinkler {}, Hydranten {}, Entnahmestellen {}, Einspeisungen {})", t.anlage, t.accessory_count, t.sprinkler_count, t.hydrant_count, t.outlet_count, t.feed_count);
        for r in &t.accessories { println!("  {:30} {:8} {:40} {:5}", r.kind, r.dn, r.type_name, r.count); }
        println!("Hinweise:");
        for nn in &t.notes { println!("  {} ({})", nn.kind, nn.count); }
    }
    if let Some(o) = out {
        excel::export(&m, &t, &o).expect("Excel-Export");
        eprintln!("-> {o}");
    }
    eprintln!("Gesamt: {:.2} s", t0.elapsed().as_secs_f64());
}
