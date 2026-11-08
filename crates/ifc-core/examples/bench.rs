use std::sync::atomic::AtomicBool;
use std::time::Instant;
fn main() {
    let p = std::env::args().nth(1).unwrap();
    let c = AtomicBool::new(false);
    let t = Instant::now();
    let s = ifc_core::step::StepFile::open(&p, &|_, _| {}, &c).unwrap();
    println!("open {:.2}s idx {}", t.elapsed().as_secs_f64(), s.len());
}
