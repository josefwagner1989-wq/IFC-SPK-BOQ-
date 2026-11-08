//! Exportação para Excel do levantamento de quantitativos.

use crate::model::{Element, Model, Row, Takeoff};
use rust_xlsxwriter::{Color, Format, FormatAlign, Workbook, Worksheet, XlsxError};

fn header(ws: &mut Worksheet, cols: &[&str], widths: &[f64]) -> Result<(), XlsxError> {
    let h = Format::new().set_bold().set_font_color(Color::White).set_background_color(Color::RGB(0xC0392B));
    for (i, c) in cols.iter().enumerate() {
        ws.write_string_with_format(0, i as u16, *c, &h)?;
        ws.set_column_width(i as u16, *widths.get(i).unwrap_or(&14.0))?;
    }
    ws.set_freeze_panes(1, 0)?;
    Ok(())
}

fn rows_sheet(wb: &mut Workbook, name: &str, rows: &[Row], with_len: bool, with_type: bool) -> Result<(), XlsxError> {
    let ws = wb.add_worksheet();
    ws.set_name(name)?;
    let mut cols = vec!["Art", "Nennweite", "Material"];
    let mut widths = vec![30.0, 12.0, 26.0];
    if with_type { cols.push("Typ"); widths.push(40.0); }
    cols.push("Anzahl"); widths.push(10.0);
    if with_len { cols.push("Länge [m]"); widths.push(12.0); }
    header(ws, &cols, &widths)?;
    let num = Format::new().set_num_format("#,##0.00");
    let bold = Format::new().set_bold();
    let boldnum = Format::new().set_bold().set_num_format("#,##0.00");
    let mut r = 1u32;
    let (mut tc, mut tl) = (0usize, 0.0f64);
    for row in rows {
        let mut c = 0u16;
        ws.write_string(r, c, &row.kind)?; c += 1;
        ws.write_string(r, c, &row.dn)?; c += 1;
        ws.write_string(r, c, &row.material)?; c += 1;
        if with_type { ws.write_string(r, c, &row.type_name)?; c += 1; }
        ws.write_number(r, c, row.count as f64)?; c += 1;
        if with_len { ws.write_number_with_format(r, c, row.length_m, &num)?; }
        tc += row.count;
        tl += row.length_m;
        r += 1;
    }
    let mut c = 0u16;
    ws.write_string_with_format(r, c, "Summe", &bold)?;
    c += if with_type { 4 } else { 3 };
    ws.write_number_with_format(r, c, tc as f64, &bold)?;
    if with_len { ws.write_number_with_format(r, c + 1, tl, &boldnum)?; }
    ws.autofilter(0, 0, r.saturating_sub(1).max(1), (cols.len() - 1) as u16)?;
    Ok(())
}

pub fn export(model: &Model, t: &Takeoff, path: &str) -> Result<(), XlsxError> {
    let title = match t.anlage.as_str() {
        "Sprinkler" => "Mengenauszug Sprinkleranlage".to_string(),
        "Hydranten" => "Mengenauszug Hydrantenanlage".to_string(),
        "Trockensteigleitung" => "Mengenauszug Trockensteigleitung".to_string(),
        "Alle" => "Mengenauszug Löschanlagen (alle Anlagen)".to_string(),
        a => format!("Mengenauszug – {a}"),
    };
    let mut wb = Workbook::new();
    // Visão geral
    {
        let ws = wb.add_worksheet();
        ws.set_name("Übersicht")?;
        let tfmt = Format::new().set_bold().set_font_size(16.0);
        let bold = Format::new().set_bold();
        let num = Format::new().set_num_format("#,##0.00").set_align(FormatAlign::Left);
        ws.set_column_width(0, 34.0)?;
        ws.set_column_width(1, 60.0)?;
        ws.write_string_with_format(0, 0, &title, &tfmt)?;
        let note = Format::new().set_italic().set_font_color(Color::RGB(0xC62828));
        ws.write_string_with_format(1, 0, "Erstellt mit IFC Sprinkler Viewer – ALPHA 0.1.0 (Vorabversion, Mengen bitte prüfen) · Created by Josef Wagner", &note)?;
        let s = &model.summary;
        let lines: Vec<(&str, String)> = vec![
            ("Datei", s.file.clone()),
            ("IFC-Schema", s.schema.clone()),
            ("Dateigröße", format!("{:.1} MB", s.file_size as f64 / 1e6)),
        ];
        let mut r = 3;
        for (k, v) in lines { ws.write_string(r, 0, k)?; ws.write_string(r, 1, &v)?; r += 1; }
        r += 1;
        ws.write_string(r, 0, "Rohre gesamt [m]")?; ws.write_number_with_format(r, 1, t.total_pipe_m, &num)?; r += 1;
        ws.write_string(r, 0, "davon gedämmt [m]")?; ws.write_number_with_format(r, 1, t.insulated_m, &num)?; r += 1;
        ws.write_string(r, 0, "Rohrsegmente [Stk]")?; ws.write_number(r, 1, t.pipe_count as f64)?; r += 1;
        ws.write_string(r, 0, "Formstücke [Stk]")?; ws.write_number(r, 1, t.fitting_count as f64)?; r += 1;
        ws.write_string(r, 0, "Armaturen & Zubehör [Stk]")?; ws.write_number(r, 1, t.accessory_count as f64)?; r += 1;
        ws.write_string(r, 0, "davon Sprinkler [Stk]")?; ws.write_number(r, 1, t.sprinkler_count as f64)?; r += 1;
        ws.write_string(r, 0, "davon Hydranten [Stk]")?; ws.write_number(r, 1, t.hydrant_count as f64)?; r += 1;
        ws.write_string(r, 0, "davon Entnahmestellen [Stk]")?; ws.write_number(r, 1, t.outlet_count as f64)?; r += 1;
        ws.write_string(r, 0, "davon Einspeisungen [Stk]")?; ws.write_number(r, 1, t.feed_count as f64)?; r += 2;
        ws.write_string_with_format(r, 0, "Rohrlänge je Nennweite", &bold)?; r += 1;
        for (dn, l) in &t.pipe_by_dn { ws.write_string(r, 0, dn)?; ws.write_number_with_format(r, 1, *l, &num)?; r += 1; }
    }
    rows_sheet(&mut wb, "Rohre", &t.pipes, true, false)?;
    {
        let ws = wb.add_worksheet();
        ws.set_name("Dämmung")?;
        header(ws, &["Dämmung", "Stärke", "Nennweite", "Anzahl", "Länge [m]"], &[34.0, 16.0, 12.0, 10.0, 12.0])?;
        let num = Format::new().set_num_format("#,##0.00");
        let mut r = 1u32;
        if !t.insulation_in_model {
            ws.write_string(r, 0, "Keine Dämmung im Modell gefunden")?;
        }
        for row in &t.insulation {
            ws.write_string(r, 0, &row.kind)?;
            ws.write_string(r, 1, &row.material)?;
            ws.write_string(r, 2, &row.dn)?;
            ws.write_number(r, 3, row.count as f64)?;
            ws.write_number_with_format(r, 4, row.length_m, &num)?;
            r += 1;
        }
    }
    rows_sheet(&mut wb, "Formstücke", &t.fittings, false, false)?;
    rows_sheet(&mut wb, "Armaturen & Zubehör", &t.accessories, false, true)?;
    // Lista individual
    {
        let ws = wb.add_worksheet();
        ws.set_name("Einzelliste")?;
        let cols = ["Kategorie", "Art", "Name", "Typ", "IFC-Klasse", "DN", "Quelle DN", "Länge [m]", "Quelle Länge", "Material", "System", "Workset", "Geschoss", "GlobalId", "Dämmung", "Dämmstärke", "Anlage"];
        let widths = [14.0, 24.0, 30.0, 30.0, 22.0, 10.0, 30.0, 10.0, 24.0, 20.0, 24.0, 20.0, 14.0, 24.0, 24.0, 12.0, 18.0];
        header(ws, &cols, &widths)?;
        let num = Format::new().set_num_format("#,##0.000");
        let mut r = 1u32;
        for &i in &model.selection {
            let e: &Element = &model.elements[i];
            if t.anlage != "Alle" && e.anlage != t.anlage { continue; }
            ws.write_string(r, 0, e.cat.label())?;
            ws.write_string(r, 1, &e.kind)?;
            ws.write_string(r, 2, &e.name)?;
            ws.write_string(r, 3, &e.type_name)?;
            ws.write_string(r, 4, &e.class)?;
            ws.write_string(r, 5, &e.dn_label)?;
            ws.write_string(r, 6, &e.dn_src)?;
            if let Some(l) = e.length_m { ws.write_number_with_format(r, 7, l, &num)?; }
            ws.write_string(r, 8, &e.len_src)?;
            ws.write_string(r, 9, &e.material)?;
            ws.write_string(r, 10, &e.systems.join(", "))?;
            ws.write_string(r, 11, &e.worksets.join(", "))?;
            ws.write_string(r, 12, &e.storey)?;
            ws.write_string(r, 13, &e.guid)?;
            ws.write_string(r, 16, &e.anlage)?;
            if let Some(x) = &e.insulation {
                ws.write_string(r, 14, &x.kind)?;
                ws.write_string(r, 15, &x.label())?;
            }
            r += 1;
        }
        if r > 1 { ws.autofilter(0, 0, r - 1, 16)?; }
    }
    // Avisos
    {
        let ws = wb.add_worksheet();
        ws.set_name("Hinweise")?;
        header(ws, &["Hinweis", "Anzahl", "Beispiel-GlobalIds"], &[34.0, 10.0, 80.0])?;
        let mut r = 1u32;
        for n in &t.notes {
            ws.write_string(r, 0, &n.kind)?;
            ws.write_number(r, 1, n.count as f64)?;
            let ex: Vec<&str> = n.elements.iter().take(10).map(|&i| model.elements[i as usize].guid.as_str()).collect();
            ws.write_string(r, 2, &ex.join(", "))?;
            r += 1;
        }
    }
    wb.save(path)?;
    Ok(())
}
