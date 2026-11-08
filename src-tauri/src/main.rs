// Sem janela de console no Windows (release)
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use ifc_core::model::{Count, ElementDetails, Geometry};
use ifc_core::{excel, Filter, Model, ScanSummary, Takeoff};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use ifc_core::tess;
use tauri::ipc::Response;
use tauri::{AppHandle, Emitter, State};

#[derive(Default)]
struct AppState {
    model: Arc<Mutex<Option<Model>>>,
    cancel: Arc<AtomicBool>,
    /// Ponto central da vista (m) – comum para o modelo principal e os de referência
    center: Arc<Mutex<[f64; 3]>>,
    refs: Arc<Mutex<Vec<(u32, tess::RefModel, Vec<u8>)>>>,
    next_ref: Arc<Mutex<u32>>,
}

#[derive(Clone, Serialize)]
struct ProgressEvent {
    phase: String,
    done: u64,
    total: u64,
}

#[derive(Serialize)]
struct ScanResult {
    summary: ScanSummary,
    default_filter: FilterOut,
}

#[derive(Serialize)]
struct FilterOut {
    systems: Vec<String>,
    worksets: Vec<String>,
}

#[derive(Serialize)]
struct ElemLite {
    i: u32,
    cat: u8,
    kind: String,
    anlage: String,
    name: String,
    dn: String,
    mat: String,
    len: Option<f64>,
}

#[derive(Serialize)]
struct LoadResult {
    count: usize,
    geometry: Geometry,
    takeoff: Takeoff,
    anlagen: Vec<Count>,
    elements: Vec<ElemLite>,
    seconds: f64,
}

fn emitter(app: &AppHandle, phase: &str) -> impl Fn(u64, u64) + Sync {
    let app = app.clone();
    let phase = phase.to_string();
    move |done, total| {
        let _ = app.emit("progress", ProgressEvent { phase: phase.clone(), done, total });
    }
}

#[tauri::command]
async fn scan(app: AppHandle, state: State<'_, AppState>, path: String) -> Result<ScanResult, String> {
    state.cancel.store(false, Ordering::Relaxed);
    let cancel = state.cancel.clone();
    let slot = state.model.clone();
    tauri::async_runtime::spawn_blocking(move || {
        // libera primeiro o modelo antigo (memória)
        *slot.lock().unwrap() = None;
        let prog = emitter(&app, "scan");
        let m = Model::scan(&path, &prog, &cancel).map_err(|e| e.to_string())?;
        let f = m.default_filter();
        let res = ScanResult {
            summary: m.summary.clone(),
            default_filter: FilterOut { systems: f.systems, worksets: f.worksets },
        };
        *slot.lock().unwrap() = Some(m);
        Ok(res)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn cancel(state: State<'_, AppState>) {
    state.cancel.store(true, Ordering::Relaxed);
}

#[tauri::command]
async fn load(app: AppHandle, state: State<'_, AppState>, filter: Filter) -> Result<LoadResult, String> {
    state.cancel.store(false, Ordering::Relaxed);
    let cancel = state.cancel.clone();
    let slot = state.model.clone();
    let center_slot = state.center.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let t0 = Instant::now();
        let mut guard = slot.lock().unwrap();
        let m = guard.as_mut().ok_or("Kein Modell geladen")?;
        let count = m.apply_filter(&filter);
        let prog = emitter(&app, "geometry");
        let geometry = m.load_geometry(&prog, &cancel).map_err(|e| e.to_string())?;
        *center_slot.lock().unwrap() = geometry.center;
        let takeoff = m.takeoff();
        let anlagen = m.anlagen();
        let elements = m
            .elements
            .iter()
            .enumerate()
            .map(|(i, e)| ElemLite {
                i: i as u32,
                cat: e.cat as u8,
                kind: e.kind.clone(),
                anlage: e.anlage.clone(),
                name: e.name.clone(),
                dn: e.dn_label.clone(),
                mat: e.material.clone(),
                len: e.length_m,
            })
            .collect();
        Ok(LoadResult { count, geometry, takeoff, anlagen, elements, seconds: t0.elapsed().as_secs_f64() })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn details(state: State<'_, AppState>, index: usize) -> Result<ElementDetails, String> {
    let guard = state.model.lock().unwrap();
    let m = guard.as_ref().ok_or("Kein Modell geladen")?;
    m.details(index).ok_or_else(|| "Bauteil nicht gefunden".into())
}

/// Levantamento de quantitativos de um sistema (None/"Alle" = todos os sistemas)
#[tauri::command]
fn takeoff(state: State<'_, AppState>, anlage: Option<String>) -> Result<Takeoff, String> {
    let guard = state.model.lock().unwrap();
    let m = guard.as_ref().ok_or("Kein Modell geladen")?;
    Ok(m.takeoff_for(anlage.as_deref().filter(|a| *a != "Alle")))
}

#[tauri::command]
async fn export_excel(state: State<'_, AppState>, path: String, anlage: Option<String>) -> Result<String, String> {
    let slot = state.model.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let guard = slot.lock().unwrap();
        let m = guard.as_ref().ok_or("Kein Modell geladen")?;
        let t = m.takeoff_for(anlage.as_deref().filter(|a| *a != "Alle"));
        let path = if path.to_lowercase().ends_with(".xlsx") { path } else { format!("{path}.xlsx") };
        excel::export(m, &t, &path).map_err(|e| e.to_string())?;
        Ok(path)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Malhas sólidas da seleção (nível de detalhe „Alto“), binário
#[tauri::command]
async fn mesh_main(app: AppHandle, state: State<'_, AppState>, segs: usize) -> Result<Response, String> {
    let slot = state.model.clone();
    let center = *state.center.lock().unwrap();
    let cancel = state.cancel.clone();
    cancel.store(false, Ordering::Relaxed);
    tauri::async_runtime::spawn_blocking(move || {
        let mut guard = slot.lock().unwrap();
        let m = guard.as_mut().ok_or("Kein Modell geladen")?;
        let prog = emitter(&app, "mesh");
        let groups = m.tess_selection(center, segs.clamp(6, 32), &prog, &cancel).map_err(|e| e.to_string())?;
        Ok(Response::new(tess::encode(&groups)))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize)]
struct RefInfo {
    id: u32,
    name: String,
    elements: usize,
}

/// Carrega IFC de referência (somente visualização, sem levantamento de quantitativos)
#[tauri::command]
async fn add_reference(app: AppHandle, state: State<'_, AppState>, path: String, segs: usize) -> Result<RefInfo, String> {
    let center = *state.center.lock().unwrap();
    let refs = state.refs.clone();
    let next = state.next_ref.clone();
    let cancel = state.cancel.clone();
    cancel.store(false, Ordering::Relaxed);
    tauri::async_runtime::spawn_blocking(move || {
        let prog = emitter(&app, "ref");
        let (rm, groups) = tess::load_reference(&path, center, segs.clamp(6, 32), &prog, &cancel).map_err(|e| e.to_string())?;
        let mut n = next.lock().unwrap();
        *n += 1;
        let info = RefInfo { id: *n, name: rm.name.clone(), elements: rm.elements.len() };
        refs.lock().unwrap().push((*n, rm, tess::encode(&groups)));
        Ok(info)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn ref_mesh(state: State<'_, AppState>, id: u32) -> Result<Response, String> {
    let refs = state.refs.lock().unwrap();
    let r = refs.iter().find(|r| r.0 == id).ok_or("Referenz nicht gefunden")?;
    Ok(Response::new(r.2.clone()))
}

#[tauri::command]
fn ref_element(state: State<'_, AppState>, id: u32, index: usize) -> Result<tess::RefElem, String> {
    let refs = state.refs.lock().unwrap();
    let r = refs.iter().find(|r| r.0 == id).ok_or("Referenz nicht gefunden")?;
    r.1.elements.get(index).cloned().ok_or_else(|| "Bauteil nicht gefunden".into())
}

#[tauri::command]
fn remove_reference(state: State<'_, AppState>, id: u32) {
    state.refs.lock().unwrap().retain(|r| r.0 != id);
}

/// Arquivo com o qual o app foi iniciado (duplo clique em .ifc)
#[tauri::command]
fn startup_file() -> Option<String> {
    std::env::args().skip(1).find(|a| {
        let l = a.to_lowercase();
        l.ends_with(".ifc") || l.ends_with(".ifczip") || l.ends_with(".rvt")
    })
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![scan, cancel, load, details, takeoff, export_excel, mesh_main, add_reference, ref_mesh, ref_element, remove_reference, startup_file])
        .run(tauri::generate_context!())
        .expect("Fehler beim Starten der App");
}
