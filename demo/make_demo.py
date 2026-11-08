import ifcopenshell, ifcopenshell.api as api, numpy as np, math
OD = {25: 33.7, 32: 42.4, 40: 48.3, 50: 60.3, 65: 76.1, 80: 88.9, 100: 114.3, 150: 168.3}
def base(name):
    f = api.run("project.create_file", version="IFC4")
    p = api.run("root.create_entity", f, ifc_class="IfcProject", name=name)
    api.run("unit.assign_unit", f)
    ctx = api.run("context.add_context", f, context_type="Model")
    body = api.run("context.add_context", f, context_type="Model", context_identifier="Body", target_view="MODEL_VIEW", parent=ctx)
    site = api.run("root.create_entity", f, ifc_class="IfcSite", name="Demo"); b = api.run("root.create_entity", f, ifc_class="IfcBuilding", name="Demo-Bürogebäude")
    api.run("aggregate.assign_object", f, relating_object=p, products=[site]); api.run("aggregate.assign_object", f, relating_object=site, products=[b])
    storeys = []
    for i, n in enumerate(["EG", "1.OG", "2.OG"]):
        s = api.run("root.create_entity", f, ifc_class="IfcBuildingStorey", name=n); s.Elevation = i * 3500.0
        api.run("aggregate.assign_object", f, relating_object=b, products=[s]); storeys.append(s)
    return f, body, storeys
def place(f, e, a, d):
    d = np.array(d, float); d /= np.linalg.norm(d)
    x = np.cross([0, 0, 1], d) if abs(d[2]) < 0.9 else np.cross([0, 1, 0], d)
    x /= np.linalg.norm(x); y = np.cross(d, x)
    m = np.eye(4); m[:3, 0] = x; m[:3, 1] = y; m[:3, 2] = d; m[:3, 3] = a
    api.run("geometry.edit_object_placement", f, product=e, matrix=m, is_si=True)
def cyl(f, body, e, a, b, r_mm):
    a = np.array(a, float); b = np.array(b, float); L = np.linalg.norm(b - a)
    prof = f.createIfcCircleProfileDef("AREA", None, None, r_mm)
    rep = api.run("geometry.add_profile_representation", f, context=body, profile=prof, depth=L)
    api.run("geometry.assign_representation", f, product=e, representation=rep); place(f, e, a, b - a)
def box(f, body, e, origin, dx, dy, dz):
    prof = f.createIfcRectangleProfileDef("AREA", None, f.createIfcAxis2Placement2D(f.createIfcCartesianPoint((dx * 500.0, dy * 500.0)), None), dx * 1000.0, dy * 1000.0)
    rep = api.run("geometry.add_profile_representation", f, context=body, profile=prof, depth=dz)
    api.run("geometry.assign_representation", f, product=e, representation=rep)
    m = np.eye(4); m[:3, 3] = origin; api.run("geometry.edit_object_placement", f, product=e, matrix=m, is_si=True)

# ---------------- Instalações (proteção contra incêndio)
f, body, storeys = base("Demo Löschanlagen")
def system(n):
    s = api.run("root.create_entity", f, ifc_class="IfcDistributionSystem", name=n); s.PredefinedType = "FIREPROTECTION"; return s
SPK, HYD, TSL = system("Sprinkleranlage Nass"), system("Wandhydranten nass"), system("Trockensteigleitung 1")
mats = {k: api.run("material.add_material", f, name=k) for k in ["Stahl schwarz", "Stahl verzinkt", "Gewinderohr"]}
cache = {}
def common(e, sysm, st, ws, mat, dn=None, fitting=False, extra=None):
    api.run("spatial.assign_container", f, relating_structure=st, products=[e])
    api.run("group.assign_group", f, products=[e], group=sysm)
    api.run("material.assign_material", f, products=[e], material=mats[mat])
    props = {"Workset": ws}
    if extra: props.update(extra)
    ps = api.run("pset.add_pset", f, product=e, name="Demo_Daten"); api.run("pset.edit_pset", f, pset=ps, properties=props)
    if dn:
        ps = api.run("pset.add_pset", f, product=e, name="Pset_PipeFittingTypeCommon" if fitting else "Pset_PipeSegmentTypeCommon")
        api.run("pset.edit_pset", f, pset=ps, properties={"NominalDiameter": float(dn)})
def pipe(a, b, dn, sysm, st, ws, mat="Stahl schwarz", extra=None):
    e = api.run("root.create_entity", f, ifc_class="IfcPipeSegment", name=f"Rohr DN{dn}", predefined_type="RIGIDSEGMENT")
    cyl(f, body, e, a, b, OD[dn] / 2); common(e, sysm, st, ws, mat, dn, extra=extra); return e
def fitting(at, d, dn, kind, sysm, st, ws, mat="Stahl schwarz"):
    names = {"BEND": "Bogen 90°", "JUNCTION": "T-Stück", "TRANSITION": "Reduzierung", "CONNECTOR": "Rillenkupplung"}
    e = api.run("root.create_entity", f, ifc_class="IfcPipeFitting", name=f"{names[kind]} DN{dn}", predefined_type=kind)
    a = np.array(at) - np.array(d) * 0.06; b = np.array(at) + np.array(d) * 0.06
    cyl(f, body, e, a, b, OD[dn] / 2 * 1.25); common(e, sysm, st, ws, mat, dn, fitting=True)
def device(cls, name, pt, at, size, sysm, st, ws, dn=None, mat="Stahl verzinkt", d=(0, 0, -1)):
    e = api.run("root.create_entity", f, ifc_class=cls, name=name, predefined_type=pt)
    a = np.array(at, float); cyl(f, body, e, a, a + np.array(d) * size[1], size[0]); common(e, sysm, st, ws, mat, dn, fitting=True)
    return e

H = 3.5
# Coluna de sprinklers + estação de válvula de alarme
pipe((2, 10, 0.3), (2, 10, 2 * H + 3.1), 150, SPK, storeys[0], "SPK_Steigleitung")
device("IfcValve", "Nassalarmventilstation DN150", "ISOLATING", (2.4, 10, 1.6), (120, 0.9), SPK, storeys[0], "SPK_Steigleitung", 150, d=(0, 0, 1))
device("IfcSensor", "Strömungsmelder DN100", "FLOWSENSOR", (2.6, 10, 3.1), (70, 0.25), SPK, storeys[0], "SPK_EG", 100, d=(1, 0, 0))
for si, st in enumerate(storeys):
    z = si * H + 3.1; ws = f"SPK_{st.Name.replace('.', '')}"
    pipe((2, 10, z), (38, 10, z), 100, SPK, st, ws)
    fitting((2, 10, z), (1, 0, 0), 100, "BEND", SPK, st, ws)
    for xi, x in enumerate(np.arange(3.5, 37, 3.0)):
        fitting((x, 10, z), (1, 0, 0), 100, "JUNCTION", SPK, st, ws)
        for sgn in (1, -1):
            pts = [10 + sgn * k for k in (0.0, 1.5, 4.5, 7.5)]
            dns = [50, 32, 25]
            for k in range(3):
                pipe((x, pts[k], z), (x, pts[k + 1], z), dns[k], SPK, st, ws, "Gewinderohr" if dns[k] == 25 else "Stahl schwarz")
                if k: fitting((x, pts[k], z), (0, sgn, 0), dns[k - 1], "TRANSITION", SPK, st, ws)
            for yk in (11.5, 14.5, 17.5) if sgn > 0 else (8.5, 5.5, 2.5):
                pipe((x, yk, z), (x, yk, z - 0.25), 25, SPK, st, ws, "Gewinderohr")
                fitting((x, yk, z), (0, 0, -1), 25, "JUNCTION", SPK, st, ws, "Gewinderohr")
                device("IfcFireSuppressionTerminal", "Sprinkler K80 68°C hängend", "SPRINKLER", (x, yk, z - 0.25), (16, 0.06), SPK, st, ws, mat="Gewinderohr")
# Hidrantes de parede (com isolamento)
ins = {"Dämmung": "Mineralwolle alukaschiert", "Dämmstärke": "30 mm"}
pipe((38.5, 1.5, 0.3), (38.5, 1.5, 2 * H + 1.5), 80, HYD, storeys[0], "HYD_Steigleitung", "Stahl verzinkt", ins)
for si, st in enumerate(storeys):
    z = si * H + 1.2
    pipe((38.5, 1.5, z), (37.6, 1.5, z), 50, HYD, st, "HYD_Steigleitung", "Stahl verzinkt", ins)
    fitting((38.5, 1.5, z), (-1, 0, 0), 80, "JUNCTION", HYD, st, "HYD_Steigleitung", "Stahl verzinkt")
    device("IfcFireSuppressionTerminal", "Wandhydrant Typ F mit Schlauchhaspel", "HOSEREEL", (37.4, 1.5, z - 0.4), (300, 0.8), HYD, st, "HYD_Steigleitung", d=(0, 0, 1))
    device("IfcValve", "Absperrventil DN50", "ISOLATING", (37.9, 1.5, z), (40, 0.15), HYD, st, "HYD_Steigleitung", 50, d=(-1, 0, 0))
# Coluna seca
pipe((38.5, 18.5, 0.3), (38.5, 18.5, 2 * H + 2.5), 80, TSL, storeys[0], "TSL_1", "Stahl verzinkt")
device("IfcFireSuppressionTerminal", "Feuerwehr-Einspeisung 2B", "BREECHINGINLET", (39.2, 18.5, 0.9), (110, 0.4), TSL, storeys[0], "TSL_1", d=(1, 0, 0))
pipe((38.5, 18.5, 0.9), (39.2, 18.5, 0.9), 80, TSL, storeys[0], "TSL_1", "Stahl verzinkt")
for si, st in enumerate(storeys):
    z = si * H + 1.1
    fitting((38.5, 18.5, z), (0, 0, 1), 80, "JUNCTION", TSL, st, "TSL_1", "Stahl verzinkt")
    device("IfcValve", "Schlauchanschlussventil B", "ISOLATING", (38.0, 18.5, z), (55, 0.3), TSL, st, "TSL_1", 50, d=(-1, 0, 0))
device("IfcValve", "Be- und Entlüftungsventil DN25", "ISOLATING", (38.5, 18.5, 2 * H + 2.5), (35, 0.12), TSL, storeys[2], "TSL_1", 25, d=(0, 0, 1))
f.write("Demo_Loeschanlagen.ifc")

# ---------------- Arquitetura (referência)
f, body, storeys = base("Demo Architektur")
wallm = api.run("material.add_material", f, name="Stahlbeton")
def arch(cls, name, st, origin, dx, dy, dz):
    e = api.run("root.create_entity", f, ifc_class=cls, name=name)
    box(f, body, e, origin, dx, dy, dz); api.run("spatial.assign_container", f, relating_structure=st, products=[e]); return e
for si, st in enumerate(storeys):
    z = si * H
    arch("IfcSlab", f"Decke {st.Name}", st, (-0.3, -0.3, z - 0.3), 40.6, 20.6, 0.3)
    for (o, dx, dy) in [((-0.3, -0.3, z), 40.6, 0.3), ((-0.3, 20.0, z), 40.6, 0.3), ((-0.3, -0.3, z), 0.3, 20.6), ((40.0, -0.3, z), 0.3, 20.6)]:
        arch("IfcWall", f"Außenwand {st.Name}", st, o, dx, dy, H - 0.3)
    for x in (10, 20, 30):
        for y in (5, 15):
            arch("IfcColumn", "Stütze 30x30", st, (x - 0.15, y - 0.15, z), 0.3, 0.3, H - 0.3)
arch("IfcSlab", "Dach", storeys[2], (-0.3, -0.3, 3 * H - 0.3), 40.6, 20.6, 0.3)
f.write("Demo_Architektur.ifc")
