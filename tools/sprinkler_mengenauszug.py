#!/usr/bin/env python3
"""
Levantamento de quantitativos de sistemas de sprinklers a partir de IFC  (IFC2x3 / IFC4 / IFC4x3)
-------------------------------------------------------------------------------------------------
Protótipo em Python (a aplicação principal é o app Tauri/Rust deste repositório).
Gera um arquivo Excel com:
  - Rohre        : comprimento [m] e quantidade por sistema / DN / material / tipo
  - Formstücke   : peças por tipo (curva, tê, redução …) / DN
  - Armaturen & Zubehör : sprinklers, válvulas, detectores de fluxo, manômetros …
  - Einzelliste  : cada componente com GUID, pavimento, valores e origem dos dados
  - Hinweise     : componentes sem comprimento/DN etc.

Uso:
  pip install ifcopenshell openpyxl
  python sprinkler_mengenauszug.py modelo.ifc
  python sprinkler_mengenauszug.py modelo.ifc -o Mengen.xlsx --system "Sprinkler|SPK"
  python sprinkler_mengenauszug.py modelo.ifc --alle     # sem filtro de sistema

Filtro de sistema (padrão): componentes atribuídos a um IfcSystem/IfcDistributionSystem
cujo nome/tipo corresponde à regex (padrão: Sprinkler|Feuer|Lösch|FIREPROTECTION|SPRINKLER)
– OU cujo próprio nome/tipo corresponde. Se o filtro não encontrar nada, o modelo inteiro é avaliado.
"""
import argparse
import re
import sys
from collections import defaultdict

import ifcopenshell
import ifcopenshell.util.element as uel
import ifcopenshell.util.unit as uunit

try:
    import ifcopenshell.geom as igeom
except Exception:  # pragma: no cover
    igeom = None

# ----------------------------------------------------------------- Classes
PIPE_CLASSES = ["IfcPipeSegment"]                 # IFC4+
FITTING_CLASSES = ["IfcPipeFitting"]              # IFC4+
ACCESSORY_CLASSES = [
    "IfcFireSuppressionTerminal", "IfcValve", "IfcFlowMeter", "IfcFlowInstrument",
    "IfcSensor", "IfcAlarm", "IfcPump", "IfcTank", "IfcActuator", "IfcController",
    "IfcInterceptor", "IfcFilter", "IfcDistributionChamberElement",
]
# Classes genéricas IFC2x3 -> atribuídas via objeto de tipo
GENERIC = {
    "IfcFlowSegment": "Rohr", "IfcFlowFitting": "Formstück",
    "IfcFlowController": "Zubehör", "IfcFlowTerminal": "Zubehör",
    "IfcFlowMovingDevice": "Zubehör", "IfcFlowStorageDevice": "Zubehör",
    "IfcFlowTreatmentDevice": "Zubehör", "IfcDistributionControlElement": "Zubehör",
}
EXCLUDE_TYPE_HINT = re.compile(r"Duct|Cable|Kanal|Kabel|Electric", re.I)

FITTING_DE = {
    "BEND": "Bogen", "JUNCTION": "T-Stück / Abzweig", "TRANSITION": "Reduzierung / Übergang",
    "CONNECTOR": "Verbinder / Muffe / Kupplung", "ENTRY": "Eintritt", "EXIT": "Austritt / Kappe",
    "OBSTRUCTION": "Blende / Stopfen", "WYE": "Y-Stück", "NOTDEFINED": "nicht definiert",
    "USERDEFINED": "benutzerdefiniert",
}
FITTING_NAME_HINTS = [
    (r"bogen|elbow|bend|winkel|90°|45°", "Bogen"),
    (r"\bt-?st|tee|abzweig|junction|kreuz|cross", "T-Stück / Abzweig"),
    (r"reduz|reducer|transition|übergang", "Reduzierung / Übergang"),
    (r"muffe|coupling|kupplung|nippel|verbinder|connector|grooved|nut", "Verbinder / Muffe / Kupplung"),
    (r"kappe|cap|stopfen|plug|blind", "Blende / Stopfen"),
    (r"flansch|flange", "Flansch"),
]
ACC_NAME_HINTS = [
    (r"sprinkler|spk|düse|nozzle|head", "Sprinkler"),
    (r"alarm.?ventil|alarm valve|nassalarm|trockenalarm|vorgesteuert|deluge", "Alarmventilstation"),
    (r"strömungs|flow.?switch|flow.?detector|strömungsmelder", "Strömungsmelder"),
    (r"manometer|pressure gauge|druck", "Manometer / Druckschalter"),
    (r"prüf|test|entleer|drain", "Prüf-/Entleerungsventil"),
    (r"absperr|schieber|gate|butterfly|absperrklappe|kugelhahn|ball", "Absperrarmatur"),
    (r"rückschlag|check", "Rückschlagventil"),
    (r"entlüft|air vent", "Entlüftung"),
    (r"pumpe|pump", "Pumpe"),
    (r"halter|hanger|abhäng|befestig|support|schelle|clamp", "Befestigung"),
]

DN_KEYS = re.compile(r"^(NominalDiameter|Nominal\s?Diameter|Nennweite|Nenndurchmesser|DN|Size|Größe|"
                     r"Groesse|Diameter|Durchmesser|Rohrgröße|Rohrdurchmesser)$", re.I)
LEN_KEYS = re.compile(r"^(Length|Länge|Laenge|NetLength|GrossLength|Rohrlänge)$", re.I)
SYSTEM_KEYS = re.compile(r"^(System\s?Name|Systemname|System\s?Type|Systemtyp|System\s?Classification|"
                         r"Systemklassifizierung|Anlage|Gewerk)$", re.I)


# ----------------------------------------------------------------- Funções auxiliares
def num(v):
    if v is None:
        return None
    if isinstance(v, (int, float)):
        return float(v)
    m = re.search(r"(\d+(?:[.,]\d+)?)", str(v))
    return float(m.group(1).replace(",", ".")) if m else None


def flat_props(el):
    """Todos os Psets/Qsets da instância + tipo como {(pset, key): value}."""
    out = {}
    for src in (uel.get_type(el), el):
        if src is None:
            continue
        for pset, props in (uel.get_psets(src) or {}).items():
            for k, v in props.items():
                if k != "id":
                    out[(pset, k)] = v
    return out


def find_prop(props, rx, prefer=()):
    for p in prefer:
        for (ps, k), v in props.items():
            if ps == p and rx.match(k) and v not in (None, ""):
                return v, f"{ps}.{k}"
    for (ps, k), v in props.items():
        if rx.match(k) and v not in (None, ""):
            return v, f"{ps}.{k}"
    return None, None


def dn_from_text(txt):
    if not txt:
        return None
    m = re.search(r"DN\s?(\d{2,3})", txt, re.I)
    if m:
        return float(m.group(1))
    m = re.search(r"(\d{2,3})\s?mm", txt, re.I)
    return float(m.group(1)) if m else None


def fmt_dn(mm):
    return f"DN {int(round(mm))}" if mm else "ohne DN"


def predefined(el):
    try:
        return uel.get_predefined_type(el) or ""
    except Exception:
        return getattr(el, "PredefinedType", "") or ""


def storey(el):
    c = uel.get_container(el)
    while c is not None and not c.is_a("IfcBuildingStorey"):
        c = uel.get_aggregate(c) if not c.is_a("IfcBuilding") else None
    return c.Name if c is not None else ""


def material(el):
    try:
        mats = uel.get_materials(el) or []
        names = [m.Name for m in mats if getattr(m, "Name", None)]
        return ", ".join(dict.fromkeys(names))
    except Exception:
        return ""


def systems_of(el):
    names = []
    for rel in getattr(el, "HasAssignments", []) or []:
        if rel.is_a("IfcRelAssignsToGroup") and rel.RelatingGroup.is_a("IfcSystem"):
            g = rel.RelatingGroup
            pt = getattr(g, "PredefinedType", None)
            names.append(g.Name or (pt or "System"))
            if pt:
                names.append(pt)
    return names


# ------------------------------------------------------- Comprimento dos tubos
def length_from_qto(props, scale):
    for (ps, k), v in props.items():
        if ps.startswith("Qto_") and LEN_KEYS.match(k) and isinstance(v, (int, float)) and v > 0:
            return v * scale, f"{ps}.{k}"
    v, src = find_prop(props, LEN_KEYS)
    if num(v):
        return num(v) * scale, src
    return None, None


def length_from_extrusion(el, scale):
    rep = getattr(el, "Representation", None)
    if not rep:
        return None
    best = None
    for r in rep.Representations:
        for it in r.Items:
            stack = [it]
            while stack:
                x = stack.pop()
                if x.is_a("IfcExtrudedAreaSolid"):
                    best = max(best or 0, x.Depth)
                elif x.is_a("IfcSweptDiskSolid"):
                    try:
                        import ifcopenshell.util.shape  # noqa
                    except Exception:
                        pass
                elif x.is_a("IfcMappedItem"):
                    stack.extend(x.MappingSource.MappedRepresentation.Items)
                elif x.is_a("IfcBooleanResult"):
                    stack.append(x.FirstOperand)
    return best * scale if best else None


_geom_settings = None


def length_from_geometry(el):
    """Maior aresta da bounding box local (metros)."""
    global _geom_settings
    if igeom is None:
        return None
    try:
        if _geom_settings is None:
            _geom_settings = igeom.settings()
            _geom_settings.set("use-world-coords", False)
        sh = igeom.create_shape(_geom_settings, el)
        v = sh.geometry.verts
        xs, ys, zs = v[0::3], v[1::3], v[2::3]
        return max(max(xs) - min(xs), max(ys) - min(ys), max(zs) - min(zs))
    except Exception:
        return None


def dn_from_profile(el, scale_mm):
    rep = getattr(el, "Representation", None)
    if not rep:
        return None
    for r in rep.Representations:
        for it in r.Items:
            if it.is_a("IfcExtrudedAreaSolid"):
                p = it.SweptArea
                if p.is_a("IfcCircleHollowProfileDef") or p.is_a("IfcCircleProfileDef"):
                    return 2 * p.Radius * scale_mm      # diâmetro externo
            if it.is_a("IfcSweptDiskSolid"):
                return 2 * it.Radius * scale_mm
    return None


# ----------------------------------------------------------- Classificação
def category(el):
    if el.is_a() in PIPE_CLASSES:
        return "Rohr"
    if el.is_a() in FITTING_CLASSES:
        return "Formstück"
    if el.is_a() in ACCESSORY_CLASSES:
        return "Zubehör"
    t = uel.get_type(el)
    tcls = t.is_a() if t is not None else ""
    if "PipeSegment" in tcls:
        return "Rohr"
    if "PipeFitting" in tcls:
        return "Formstück"
    if EXCLUDE_TYPE_HINT.search(tcls):
        return None
    for g, cat in GENERIC.items():
        if el.is_a(g):
            return cat
    return None


def fitting_kind(el, typ_name):
    pt = predefined(el)
    if pt and pt not in ("NOTDEFINED", "USERDEFINED"):
        return FITTING_DE.get(pt, pt)
    txt = f"{el.Name or ''} {typ_name} {getattr(el, 'ObjectType', '') or ''}"
    for rx, label in FITTING_NAME_HINTS:
        if re.search(rx, txt, re.I):
            return label
    return FITTING_DE.get(pt, "sonstiges Formstück")


def accessory_kind(el, typ_name):
    txt = f"{el.Name or ''} {typ_name} {getattr(el, 'ObjectType', '') or ''}"
    for rx, label in ACC_NAME_HINTS:
        if re.search(rx, txt, re.I):
            return label
    if el.is_a("IfcFireSuppressionTerminal"):
        pt = predefined(el)
        return {"SPRINKLER": "Sprinkler", "SPRINKLERDEFLECTOR": "Sprinkler-Deflektor",
                "BREECHINGINLET": "Einspeisung", "FIREHYDRANT": "Hydrant",
                "HOSEREEL": "Wandhydrant / Schlauchhaspel"}.get(pt, "Löschwasser-Endgerät")
    if el.is_a("IfcValve"):
        return "Ventil (" + (predefined(el) or "?") + ")"
    return el.is_a().replace("Ifc", "")


# ------------------------------------------------------------------ Principal
def run(path, out, system_rx, use_all, use_geom):
    f = ifcopenshell.open(path)
    scale = uunit.calculate_unit_scale(f)          # unidade do projeto -> m
    scale_mm = scale * 1000.0
    rx = re.compile(system_rx, re.I)

    candidates = []
    seen = set()
    classes = PIPE_CLASSES + FITTING_CLASSES + ACCESSORY_CLASSES + list(GENERIC)
    for cls in classes:
        try:
            items = f.by_type(cls)
        except RuntimeError:            # classe não existe no schema
            continue
        for el in items:
            if el.id() in seen or el.is_a("IfcElementType"):
                continue
            cat = category(el)
            if cat:
                seen.add(el.id())
                candidates.append((el, cat))

    def matches(el, props):
        txt = " ".join(systems_of(el))
        v, _ = find_prop(props, SYSTEM_KEYS)
        t = uel.get_type(el)
        txt += f" {v or ''} {el.Name or ''} {t.Name if t is not None else ''}"
        return bool(rx.search(txt))

    rows, notes = [], []
    prop_cache = {el.id(): flat_props(el) for el, _ in candidates}
    filtered = candidates if use_all else [(e, c) for e, c in candidates if matches(e, prop_cache[e.id()])]
    if not use_all and not filtered:
        notes.append(("Info", "", "Systemfilter fand keine Bauteile – ganzes Modell ausgewertet."))
        filtered = candidates

    for el, cat in filtered:
        props = prop_cache[el.id()]
        t = uel.get_type(el)
        typ_name = (t.Name if t is not None else "") or getattr(el, "ObjectType", "") or ""
        sysname = ", ".join(dict.fromkeys(systems_of(el))) or (find_prop(props, SYSTEM_KEYS)[0] or "")

        # Diâmetro nominal
        dn_raw, dn_src = find_prop(props, DN_KEYS,
                                   prefer=("Pset_PipeSegmentTypeCommon", "Pset_PipeFittingTypeCommon"))
        dn = num(dn_raw)
        if dn is not None and isinstance(dn_raw, (int, float)) and dn_src and \
                dn_src.split(".")[0].startswith("Pset_"):
            dn *= scale_mm                     # Psets padrão IFC na unidade do projeto
        if dn is not None and dn < 1:           # provavelmente em metros
            dn *= 1000
        if dn is None:
            dn = dn_from_text(f"{el.Name or ''} {typ_name}")
            dn_src = "Name/Typname" if dn else None
        if dn is None:
            d = dn_from_profile(el, scale_mm)
            if d:
                dn, dn_src = d, "Geometrie (Außendurchm.)"

        length, l_src = (None, None)
        if cat == "Rohr":
            length, l_src = length_from_qto(props, scale)
            if not length:
                length = length_from_extrusion(el, scale)
                l_src = "Extrusionstiefe" if length else None
            if not length and use_geom:
                length = length_from_geometry(el)
                l_src = "Geometrie (Bounding-Box)" if length else None
            if not length:
                notes.append(("Rohr ohne Länge", el.GlobalId, el.Name or ""))

        kind = typ_name or el.is_a()
        if cat == "Formstück":
            kind = fitting_kind(el, typ_name)
        elif cat == "Zubehör":
            kind = accessory_kind(el, typ_name)
        if dn is None and cat != "Zubehör":
            notes.append((f"{cat} ohne Nennweite", el.GlobalId, el.Name or ""))

        rows.append(dict(cat=cat, kind=kind, typ=typ_name, name=el.Name or "", cls=el.is_a(),
                         pt=predefined(el), dn=dn, dn_src=dn_src or "", length=length,
                         l_src=l_src or "", mat=material(el), sys=sysname, storey=storey(el),
                         guid=el.GlobalId))

    write_xlsx(out, rows, notes, path, f.schema)
    return rows, notes


# ------------------------------------------------------------------ Excel
def write_xlsx(out, rows, notes, src, schema):
    from openpyxl import Workbook
    from openpyxl.styles import Font, PatternFill, Alignment
    from openpyxl.utils import get_column_letter

    wb = Workbook()
    hdr_fill = PatternFill("solid", fgColor="C00000")
    hdr_font = Font(bold=True, color="FFFFFF")

    def sheet(title, header, data, num_fmt=None, total_cols=()):
        ws = wb.create_sheet(title)
        ws.append(header)
        for c in ws[1]:
            c.fill, c.font = hdr_fill, hdr_font
            c.alignment = Alignment(vertical="center")
        for r in data:
            ws.append(list(r))
        if data and total_cols:
            n = len(data) + 1
            tot = ["Summe"] + [""] * (len(header) - 1)
            ws.append(tot)
            for ci in total_cols:
                col = get_column_letter(ci)
                ws.cell(row=n + 1, column=ci, value=f"=SUBTOTAL(9,{col}2:{col}{n})")
            for c in ws[n + 1]:
                c.font = Font(bold=True)
        for ci, fmt in (num_fmt or {}).items():
            for row in ws.iter_rows(min_row=2, min_col=ci, max_col=ci):
                for c in row:
                    c.number_format = fmt
        for i, h in enumerate(header, 1):
            width = max([len(str(h))] + [len(str(r[i - 1])) for r in data if r[i - 1] is not None] + [8])
            ws.column_dimensions[get_column_letter(i)].width = min(width + 2, 60)
        ws.freeze_panes = "A2"
        if data:
            ws.auto_filter.ref = f"A1:{get_column_letter(len(header))}{len(data) + 1}"
        return ws

    # Tubos
    agg = defaultdict(lambda: [0, 0.0])
    for r in rows:
        if r["cat"] == "Rohr":
            k = (r["sys"], fmt_dn(r["dn"]), r["dn"] or 0, r["mat"], r["typ"])
            agg[k][0] += 1
            agg[k][1] += r["length"] or 0
    pipe_rows = [(s, d, m, t, n, round(l, 3)) for (s, d, _, m, t), (n, l)
                 in sorted(agg.items(), key=lambda kv: (kv[0][0], kv[0][2], kv[0][3]))]
    sheet("Rohre", ["System", "Nennweite", "Material", "Rohrtyp", "Anzahl Segmente", "Länge [m]"],
          pipe_rows, {6: "#,##0.00"}, total_cols=(5, 6))

    # Conexões / acessórios
    def count(cat):
        a = defaultdict(int)
        for r in rows:
            if r["cat"] == cat:
                a[(r["sys"], r["kind"], fmt_dn(r["dn"]), r["dn"] or 0, r["typ"])] += 1
        return [(s, k, d, t, n) for (s, k, d, _, t), n
                in sorted(a.items(), key=lambda kv: (kv[0][0], kv[0][1], kv[0][3]))]

    sheet("Formstücke", ["System", "Art", "Nennweite", "Typ", "Stück"], count("Formstück"), total_cols=(5,))
    sheet("Armaturen & Zubehör", ["System", "Art", "Nennweite", "Typ", "Stück"], count("Zubehör"),
          total_cols=(5,))

    # Lista individual
    det = [(r["cat"], r["kind"], r["name"], r["typ"], r["cls"], r["pt"],
            round(r["dn"], 1) if r["dn"] else None, r["dn_src"],
            round(r["length"], 3) if r["length"] else None, r["l_src"],
            r["mat"], r["sys"], r["storey"], r["guid"]) for r in rows]
    sheet("Einzelliste", ["Kategorie", "Art", "Name", "Typ", "IFC-Klasse", "PredefinedType", "DN/Ø [mm]",
                          "Quelle DN", "Länge [m]", "Quelle Länge", "Material", "System", "Geschoss",
                          "GlobalId"], det, {9: "#,##0.000"})

    # Visão geral
    ws = wb.active
    ws.title = "Übersicht"
    total_len = sum(r["length"] or 0 for r in rows if r["cat"] == "Rohr")
    lines = [("Mengenauszug Sprinkleranlage",), ("Quelle", src), ("IFC-Schema", schema), ("",),
             ("Rohre gesamt [m]", round(total_len, 2)),
             ("Rohrsegmente [Stk]", sum(r["cat"] == "Rohr" for r in rows)),
             ("Formstücke [Stk]", sum(r["cat"] == "Formstück" for r in rows)),
             ("Armaturen & Zubehör [Stk]", sum(r["cat"] == "Zubehör" for r in rows)),
             ("davon Sprinkler [Stk]", sum(r["cat"] == "Zubehör" and r["kind"] == "Sprinkler" for r in rows)),
             ("",), ("Rohrlänge je Nennweite",)]
    by_dn = defaultdict(float)
    for r in rows:
        if r["cat"] == "Rohr":
            by_dn[r["dn"] or 0] += r["length"] or 0
    lines += [(fmt_dn(d), round(l, 2)) for d, l in sorted(by_dn.items())]
    for l in lines:
        ws.append(list(l))
    ws["A1"].font = Font(bold=True, size=14)
    ws["A11"].font = Font(bold=True)
    ws.column_dimensions["A"].width = 30
    ws.column_dimensions["B"].width = 60

    sheet("Hinweise", ["Hinweis", "GlobalId", "Name"], notes)
    wb.save(out)


def main():
    ap = argparse.ArgumentParser(description="Mengenauszug Sprinkleranlage aus IFC")
    ap.add_argument("ifc")
    ap.add_argument("-o", "--out", default=None, help="Excel-Ausgabe (Standard: <ifc>_Mengen.xlsx)")
    ap.add_argument("--system", default=r"Sprinkler|SPK|Feuer|Lösch|Loesch|FIREPROTECTION|SPRINKLER|Fire",
                    help="Regex für System-/Bauteilnamen")
    ap.add_argument("--alle", action="store_true", help="kein Systemfilter, alle Rohrleitungsbauteile")
    ap.add_argument("--keine-geometrie", action="store_true",
                    help="keine Längenberechnung aus Geometrie (schneller)")
    a = ap.parse_args()
    out = a.out or re.sub(r"\.ifc$", "", a.ifc, flags=re.I) + "_Mengen.xlsx"
    rows, notes = run(a.ifc, out, a.system, a.alle, not a.keine_geometrie)
    pipes = [r for r in rows if r["cat"] == "Rohr"]
    print(f"Rohre: {len(pipes)} Segmente, {sum(r['length'] or 0 for r in pipes):.2f} m | "
          f"Formstücke: {sum(r['cat'] == 'Formstück' for r in rows)} | "
          f"Zubehör: {sum(r['cat'] == 'Zubehör' for r in rows)} | Hinweise: {len(notes)}")
    print(f"-> {out}")


if __name__ == "__main__":
    sys.exit(main())
